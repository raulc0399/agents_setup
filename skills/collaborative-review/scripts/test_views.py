"""Operator status reflects one read snapshot and never writes review state."""
import tempfile
import os
import socket
import unittest
from pathlib import Path
from unittest.mock import patch

from review_core.store import Store
from review_core.views import status_view, format_status


class ViewTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'run'
        self.store = Store(self.path, create=True, metadata={
            'revision': 'rev', 'status': 'paused', 'budget_seconds': 100,
            'spent_seconds': 12, 'snapshot': {'files': [{'path': 'a'}, {'path': 'b'}]}})
        self.addCleanup(self.store.close)
        self.approval = {'result': 'BLOCKED', 'blockers': ['challenge missing']}

    def task(self, identity, status, revision='rev', **fields):
        return self.store.put_record('task', identity, {
            'kind': 'review', 'payload': {'revision': revision, 'reviewer': identity,
                                        'scope_paths': ['a', 'b']},
            'status': status, 'reserved_seconds': 0, **fields})

    def view(self, store=None):
        with patch('review_core.cli.readiness', return_value=self.approval) as readiness:
            result = status_view(store or self.store)
        readiness.assert_called_once_with(store or self.store, None)
        return result

    def test_current_coverage_decisions_and_actual_budget(self):
        self.task('reviewer-1', 'pending')
        self.task('reviewer-2', 'running', reserved_seconds=20, lease_expires=99999999999)
        self.task('failed', 'failed', error='process failed')
        self.task('uncertain', 'uncertain', revision='old')
        self.store.put_record('question', 'q', {'revision': 'rev', 'status': 'unanswered', 'text': 'Choose scope'})
        for identity, revision, status, evidence, paths in (
                ('good', 'rev', 'reviewed', ['proof'], ['a']),
                ('old', 'old', 'reviewed', ['proof'], ['b']),
                ('partial', 'rev', 'partial', ['proof'], ['b']),
                ('empty', 'rev', 'reviewed', [], ['b'])):
            self.store.put_record('coverage', identity, dict(author='reviewer-1', revision=revision,
                                                           status=status, evidence=evidence, paths=paths))
        view = self.view()
        self.assertEqual(view['uncovered_paths']['reviewer-1'], ['b'])
        self.assertEqual(view['budget']['remaining_seconds'], 68)
        self.assertEqual(view['budget']['reserved_seconds'], 20)
        actions = {item['action'] for item in view['operator_decisions']}
        self.assertTrue({'respond', 'recover', 'run', 'authorize', 'retry', 'resume', 'resolve gaps'} <= actions)
        text = format_status(view)
        for expected in ('Run paused', 'reviewer-1', 'uncertain', 'Approval: BLOCKED', 'challenge missing', '68 remaining'):
            self.assertIn(expected, text)

    def test_readonly_has_no_mutation(self):
        self.task('worker', 'pending')
        before = self.store.events()
        with Store(self.path, readonly=True) as readonly:
            self.view(readonly)
            self.assertEqual(readonly.events(), before)
            self.assertEqual(readonly.connection.total_changes, 0)

    def test_readiness_and_fields_share_snapshot_during_concurrent_write(self):
        def readiness(store, coordinator):
            with Store(self.path) as writer:
                writer.set_meta('revision', 'new')
                writer.set_meta('spent_seconds', 99)
            self.assertEqual(store.get_meta('revision'), 'rev')
            return {'result': 'BLOCKED', 'revision': store.get_meta('revision'), 'blockers': []}
        with patch('review_core.cli.readiness', side_effect=readiness):
            view = status_view(self.store)
        self.assertEqual(view['revision'], view['approval']['revision'])
        self.assertEqual(view['budget']['spent_seconds'], 12)
        self.assertEqual(self.store.get_meta('revision'), 'new')

    def test_expired_remote_worker_requires_termination_confirmation_before_resume(self):
        self.task('remote', 'running', lease_expires=1,
                  execution={'host': 'unreachable-worker', 'pid': 123, 'terminated': False})
        decisions = [item for item in self.view()['operator_decisions'] if item['id'] == 'remote' and item['action'] != 'resolve gaps']
        self.assertEqual(len(decisions), 1)
        self.assertIn('recover --confirm-termination', decisions[0]['reason'])
        self.assertIn('then resume', decisions[0]['reason'])
        self.assertNotEqual(decisions[0]['action'], 'resume')

    def test_local_live_worker_cannot_be_overridden_and_terminated_can_resume(self):
        self.task('local', 'running', lease_expires=1,
                  execution={'host': socket.gethostname(), 'pid': os.getpid(), 'terminated': False})
        decisions = self.view()['operator_decisions']
        local = next(item for item in decisions if item['action'] == 'inspect termination')
        self.assertIn('cannot be overridden', local['reason'])
        self.assertNotIn('recover --confirm-termination', local['reason'])
        self.task('local', 'running', lease_expires=1, execution={'terminated': True})
        self.assertTrue(any(item['id'] == 'local' and item['action'] == 'resume'
                            for item in self.view()['operator_decisions']))

    def test_cancelled_unrequired_reviewer_has_no_coverage_gap(self):
        self.task('retired', 'cancelled')
        view = self.view()
        self.assertNotIn('retired', view['uncovered_paths'])
        self.assertFalse(any(item['id'] == 'retired' for item in view['operator_decisions']))
        self.store.put_record('review_plan', 'plan', {
            'request': {'revision': 'rev'}, 'task_ids': ['retired']})
        view = self.view()
        self.assertEqual(view['uncovered_paths']['retired'], ['a', 'b'])
        self.assertTrue(any(item['action'] == 'retry' and item['id'] == 'retired'
                            for item in view['operator_decisions']))

    def test_terminal_controls_are_visible_without_destroying_unicode(self):
        self.task('worker\x1b[2J\nspoof', 'failed', error='bad\rresult')
        self.approval['blockers'] = ['unsafe\x9b31m\u202e']
        text = format_status(self.view())
        self.assertNotIn('\x1b', text)
        self.assertNotIn('\x9b', text)
        self.assertNotIn('\u202e', text)
        self.assertIn('worker\\u001b[2J\\u000aspoof', text)
        self.assertIn('—', text)


if __name__ == '__main__':
    unittest.main()
