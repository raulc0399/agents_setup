"""Recovery sequences use real stores and a controlled lease clock."""
import os
from pathlib import Path
import socket
import tempfile
import unittest
from unittest.mock import patch

from review_core.authorization import record_authorization, revoke_authorization
from review_core.coordinator import Coordinator
from review_core.recovery import confirm_process_terminated, handoff, resume_run, resolve_uncertain_task
from review_core.store import Store


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.temp.name) / 'run', create=True, metadata={
            'revision': 'rev', 'status': 'ready', 'budget_seconds': 200,
            'spent_seconds': 0, 'max_workers': 2})
        self.now = 1000
        self.coordinator = Coordinator(self.store, clock=lambda: self.now)
        record_authorization(self.store, 'rev', ['review', 'execute'], ['.'])

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def claim(self, kind='review'):
        task = self.coordinator.enqueue_task(kind, {'revision': 'rev', 'brief': 'Check',
            'timeout_seconds': 40, 'scope_paths': ['src']}, task_id='stable')
        return self.coordinator.claim_task('worker', lease_seconds=10)

    def test_handoff_distinguishes_pending_start_from_revoked_or_expired_permission(self):
        self.store.set_meta('paths', ['src'])
        initial_grant = self.store.list_records('authorization')[0]
        self.assertTrue(handoff(self.store, now=self.now)['start_approval_recorded'])
        self.assertTrue(handoff(self.store, now=self.now)['start_authorized'])
        revoke_authorization(self.store, initial_grant['id'])
        revoked = handoff(self.store, now=self.now)
        self.assertTrue(revoked['start_approval_recorded'])
        self.assertFalse(revoked['start_authorized'])
        record_authorization(self.store, 'rev', ['review'], ['src'], expires_at=self.now + 1)
        expired = handoff(self.store, now=self.now + 1)
        self.assertTrue(expired['start_approval_recorded'])
        self.assertFalse(expired['start_authorized'])

    def test_read_only_recovery_preserves_identity_history_and_budget(self):
        first = self.claim()
        self.now += 12
        checkpoint = resume_run(self.store, self.coordinator)
        self.assertEqual(checkpoint['spent_seconds'], 12)
        self.assertEqual(checkpoint['reserved_seconds'], 0)
        recovered = self.store.get_record('task', 'stable')['data']
        self.assertEqual(recovered['attempts'][0]['lease_id'], first['data']['lease_id'])
        self.assertEqual(recovered['attempts'][0]['status'], 'expired')
        resume_run(self.store, self.coordinator)
        self.assertEqual(self.store.get_meta('spent_seconds'), 12)
        second = self.coordinator.claim_task('worker')
        self.assertEqual(second['id'], 'stable')
        self.assertEqual(second['data']['attempt'], 2)
        with self.assertRaises(ValueError):
            self.coordinator.complete_task('stable', 'worker', first['data']['lease_id'], {})

    def test_uncertain_execute_blocks_until_explicit_outcome(self):
        self.claim('execute')
        self.now += 11
        checkpoint = resume_run(self.store, self.coordinator)
        self.assertEqual(checkpoint['operator_decisions'], ['stable'])
        self.assertEqual(checkpoint['spent_seconds'], 40)
        self.assertEqual(checkpoint['status'], 'paused')
        self.assertIsNone(self.coordinator.claim_task('other'))
        with self.assertRaises(ValueError):
            resolve_uncertain_task(self.store, 'stable', 'retry', '')
        with self.assertRaisesRegex(ValueError, 'budget'):
            resolve_uncertain_task(self.store, 'stable', 'retry', 'Verified no effects', now=self.now)
        resolve_uncertain_task(self.store, 'stable', 'failed', 'Allocation consumed', now=self.now)
        self.assertEqual(self.store.get_meta('status'), 'paused')
        resume_run(self.store, self.coordinator)
        self.assertIsNone(self.coordinator.claim_task('other'))

    def test_expired_cancellation_is_not_retried(self):
        self.claim()
        self.coordinator.cancel_task('stable', 'Stop')
        self.now += 11
        resume_run(self.store, self.coordinator)
        self.assertEqual(self.store.get_record('task', 'stable')['data']['status'], 'cancelled')
        self.assertIsNone(self.coordinator.claim_task('other'))

    def test_complete_and_unexpired_runs_not_reset(self):
        self.claim()
        self.store.set_meta('status', 'complete')
        before = handoff(self.store)
        self.assertEqual(resume_run(self.store, self.coordinator)['status'], 'complete')
        self.assertEqual(handoff(self.store), before)

    def test_live_process_blocks_read_only_requeue_and_execute_resolution(self):
        self.claim()
        task = self.store.get_record('task', 'stable')
        task['data']['execution'] = {'pid': os.getpid(), 'host': socket.gethostname(),
                                     'terminated': False}
        self.store.put_record('task', 'stable', task['data'])
        self.now += 11
        result = resume_run(self.store, self.coordinator)
        self.assertEqual(result['reserved_seconds'], 40)
        self.assertEqual(result['operator_decisions'], ['stable'])
        self.assertEqual(self.store.get_record('task', 'stable')['data']['status'], 'running')
        task = self.store.get_record('task', 'stable')
        task['data']['kind'] = 'execute'
        self.store.put_record('task', 'stable', task['data'])
        resume_run(self.store, self.coordinator)
        with self.assertRaises(ValueError):
            resolve_uncertain_task(self.store, 'stable', 'retry', 'Guess')
        with self.assertRaises(ValueError):
            resolve_uncertain_task(self.store, 'stable', 'retry', 'Guess',
                                   process_termination_evidence='Unsafe override')

    def test_readonly_checkpoint_and_healthy_worker(self):
        self.claim()
        task = self.store.get_record('task', 'stable')
        task['data']['execution'] = {'pid': os.getpid(), 'host': socket.gethostname(), 'terminated': False}
        self.store.put_record('task', 'stable', task['data'])
        with Store(self.store.run_dir, readonly=True) as reader:
            with patch('review_core.recovery.time.time', return_value=self.now):
                self.assertEqual(handoff(reader)['operator_decisions'], [])

    def test_launch_intent_blocks_expired_readonly_retry(self):
        self.claim()
        task = self.store.get_record('task', 'stable')
        task['data']['execution'] = {'launch_intent': True, 'terminated': False}
        self.store.put_record('task', 'stable', task['data'])
        self.now += 11
        resume_run(self.store, self.coordinator)
        self.assertEqual(self.store.get_record('task', 'stable')['data']['status'], 'running')
        confirm_process_terminated(self.store, 'stable', 'Operator verified no process launched', now=self.now)
        resume_run(self.store, self.coordinator)
        self.assertEqual(self.store.get_record('task', 'stable')['data']['status'], 'pending')

    def test_reused_pid_does_not_block_recovery(self):
        self.claim()
        task = self.store.get_record('task', 'stable')
        task['data']['execution'] = {'pid': os.getpid(), 'host': socket.gethostname(),
                                     'start_token': 'old-process', 'terminated': False}
        self.store.put_record('task', 'stable', task['data'])
        self.now += 11
        resume_run(self.store, self.coordinator)
        self.assertEqual(self.store.get_record('task', 'stable')['data']['status'], 'pending')

    def test_unknown_process_requires_evidenced_operator_confirmation(self):
        self.claim('execute')
        task = self.store.get_record('task', 'stable')
        task['data']['execution'] = {'host': 'unreachable-host', 'pid': 123, 'terminated': False}
        self.store.put_record('task', 'stable', task['data'])
        self.now += 11
        resume_run(self.store, self.coordinator)
        with self.assertRaises(ValueError):
            resolve_uncertain_task(self.store, 'stable', 'retry', 'No effects')
        result = resolve_uncertain_task(self.store, 'stable', 'failed', 'No effects',
            process_termination_evidence='Remote operator verified process group absent')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertTrue(result['data']['execution']['terminated'])

    def test_repeated_expirations_share_original_allocation(self):
        self.claim()
        for elapsed, expected in ((12, 12), (12, 24), (20, 40)):
            self.now += elapsed
            checkpoint = resume_run(self.store, self.coordinator)
            task = self.store.get_record('task', 'stable')['data']
            self.assertEqual(task['spent_seconds'], expected)
            self.assertEqual(checkpoint['spent_seconds'], expected)
            self.assertEqual(sum(a['spent_seconds'] for a in task['attempts']), expected)
            if expected < 40:
                claimed = self.coordinator.claim_task('worker', lease_seconds=10)
                self.assertEqual(claimed['data']['reserved_seconds'], 40 - expected)
            else:
                self.assertEqual(task['status'], 'failed')
                self.assertIn('budget', task['error'])
                self.assertIsNone(self.coordinator.claim_task('worker'))

    def test_transaction_rolls_back_on_recovery_failure(self):
        self.claim()
        self.now += 11
        original = self.store.put_record
        def fail(*args, **kwargs):
            raise RuntimeError('write failure')
        self.store.put_record = fail
        with self.assertRaises(RuntimeError):
            resume_run(self.store, self.coordinator)
        self.store.put_record = original
        self.assertEqual(self.store.get_meta('spent_seconds'), 0)
        self.assertEqual(self.store.get_record('task', 'stable')['data']['status'], 'running')


if __name__ == '__main__':
    unittest.main()
