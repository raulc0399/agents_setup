"""Operator status reflects one read snapshot and never writes review state."""
import tempfile
import json
import subprocess
import sys
import os
import socket
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

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
                                        'scope_paths': ['a', 'b'], 'timeout_seconds': 10},
            'status': status, 'reserved_seconds': 0, 'worker': identity,
            'dependencies': [], 'spent_seconds': 0, 'attempt': 0, **fields})

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
        self.assertTrue({'respond', 'recover', 'retry', 'resume', 'resolve gaps',
                         'Provide an explicit current scope authorization'} <= actions)
        text = format_status(view)
        for expected in ('Run paused', 'reviewer-1', 'uncertain', 'Approval: BLOCKED', 'challenge missing', '68 remaining'):
            self.assertIn(expected, text)

    def test_pending_and_reported_progress(self):
        self.task('ready', 'pending')
        self.task('waiting', 'pending')
        self.task('working', 'running', lease_expires=99999999999,
                  progress={'summary': 'Traced caller', 'at': 10, 'attempt': 1}, attempt=1)
        self.task('heartbeat', 'running', lease_expires=99999999999, renewed_at=20)
        coordinator = Mock()
        coordinator.availability.return_value = {'tasks': [
            {'task_id': 'ready', 'worker': 'ready', 'ready': True, 'blockers': []},
            {'task_id': 'waiting', 'worker': 'waiting', 'ready': False, 'blockers': [
                {'code': 'dependency', 'reason': 'Await working', 'action': 'Complete working', 'owner': 'working'}]}]}
        with patch('review_core.cli.readiness', return_value=self.approval), patch('review_core.views.time.time', return_value=30):
            view = status_view(self.store, coordinator)
        self.assertEqual({row['id'] for row in view['active_tasks']}, {'working', 'heartbeat'})
        self.assertEqual([row['id'] for row in view['queued_tasks']], ['ready'])
        self.assertEqual([row['id'] for row in view['blocked_tasks']], ['waiting'])
        details = {item['id']: item for item in view['task_details']}
        self.assertEqual(details['waiting']['wait_owner'], 'working')
        self.assertEqual(details['working']['reported_progress']['age_seconds'], 20)
        self.assertIsNone(details['heartbeat']['reported_progress'])
        for expected in ('queued', 'blocked', 'Await working', 'Complete working', 'Traced caller', 'No progress reported'):
            self.assertIn(expected, format_status(view))

    def test_expired_running_is_recovery_and_renewal_is_not_transition(self):
        self.task('expired', 'running', lease_expires=1, execution={'terminated': True})
        transition = self.store.events()[-1]
        self.task('expired', 'running', lease_expires=1, execution={'terminated': True})
        view = self.view()
        detail = next(item for item in view['task_details'] if item['id'] == 'expired')
        self.assertEqual(detail['state'], 'recovery required')
        self.assertEqual(detail['wait_owner'], 'operator')
        self.assertEqual(detail['last_transition']['at'], transition['at'])
        self.assertIn('0 working', format_status(view))
        self.assertIn('Recover expired task lease', format_status(view))

    def test_short_ids_terminate_when_one_task_id_prefixes_another(self):
        parent = 'task-' + 25 * 'a'
        child = parent + '-child'
        self.task(parent, 'completed')
        self.task(child, 'completed')
        script = (
            'import json, sys; from unittest.mock import patch; '
            'from review_core.store import Store; from review_core.views import status_view; '
            'store = Store(sys.argv[1], readonly=True); '
            'mock = patch("review_core.cli.readiness", return_value={"result": "BLOCKED", "blockers": []}); '
            'mock.start(); print(json.dumps(status_view(store)["task_details"]))'
        )
        result = subprocess.run([sys.executable, '-c', script, str(self.path)],
                                capture_output=True, text=True, timeout=5, check=True, cwd=Path(__file__).resolve().parent)
        details = {item['id']: item for item in json.loads(result.stdout)}
        self.assertEqual(details[parent]['short_id'], parent)
        self.assertTrue(child.startswith(details[child]['short_id']))
        self.assertNotEqual(details[parent]['short_id'], details[child]['short_id'])

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
            'request': {'revision': 'rev'}, 'task_ids': ['retired'], 'challenge_budget_seconds': 10})
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
