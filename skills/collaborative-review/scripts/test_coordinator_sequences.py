"""Independent cancellation/recovery sequences and conserved time budgets."""
from pathlib import Path
import tempfile
import unittest

from review_core.authorization import record_authorization
from review_core.coordinator import Coordinator
from review_core.delegation import plan_review
from review_core.recovery import handoff, resume_run, resolve_uncertain_task
from review_core.store import Store


class CoordinatorSequenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.temp.name) / 'run', create=True, metadata={
            'revision': 'captured', 'status': 'ready', 'budget_seconds': 100,
            'spent_seconds': 0, 'max_workers': 1})
        self.now = 1000
        self.coordinator = Coordinator(self.store, clock=lambda: self.now)
        record_authorization(self.store, 'captured', ['review', 'execute'], ['input.txt'])

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def enqueue(self, kind='review'):
        return self.coordinator.enqueue_task(kind, {'revision': 'captured', 'brief': 'Read input.txt',
            'scope_paths': ['input.txt'], 'timeout_seconds': 20},
            task_id='sequence-' + str(len(self.store.list_records('task'))))

    def budget(self, spent, reserved):
        checkpoint = handoff(self.store, now=self.now)
        self.assertEqual(checkpoint['spent_seconds'], spent)
        self.assertEqual(checkpoint['reserved_seconds'], reserved)
        self.assertEqual(checkpoint['remaining_seconds'], 100 - spent - reserved)

    def test_cancel_recover_reclaim_rejects_stale_ack_without_double_charge(self):
        cancelled = self.enqueue()
        next_task = self.enqueue()
        first = self.coordinator.claim_task('first', lease_seconds=5)
        self.coordinator.cancel_task(first['id'], 'operator stopped review')
        self.budget(0, 20)
        self.assertIsNone(self.coordinator.claim_task('second'))
        self.now += 6
        resume_run(self.store, self.coordinator)
        self.assertEqual(self.store.get_record('task', cancelled['id'])['data']['status'], 'cancelled')
        self.budget(6, 0)
        resume_run(self.store, self.coordinator)
        self.budget(6, 0)
        second = self.coordinator.claim_task('second')
        self.assertEqual(second['id'], next_task['id'])
        self.budget(6, 20)
        with self.assertRaises(ValueError):
            self.coordinator.complete_task(first['id'], 'first', first['data']['lease_id'], {})
        self.now += 3
        self.coordinator.complete_task(second['id'], 'second', second['data']['lease_id'], {})
        self.budget(9, 0)

    def test_expired_readonly_retry_preserves_attempt_and_fences_old_lease(self):
        task = self.enqueue()
        first = self.coordinator.claim_task('reader', lease_seconds=5)
        self.now += 6
        resume_run(self.store, self.coordinator)
        self.budget(6, 0)
        second = self.coordinator.claim_task('reader')
        self.assertEqual(second['id'], task['id'])
        self.assertEqual(second['data']['attempt'], 2)
        self.assertEqual(second['data']['attempts'][0]['status'], 'expired')
        self.assertNotEqual(first['data']['lease_id'], second['data']['lease_id'])
        with self.assertRaises(ValueError):
            self.coordinator.fail_task(task['id'], 'reader', first['data']['lease_id'], 'late error')
        self.now += 2
        self.coordinator.complete_task(task['id'], 'reader', second['data']['lease_id'], {})
        self.budget(8, 0)

    def test_fractional_remaining_budget_can_fund_its_own_review_plan(self):
        self.store.set_meta('budget_seconds', 120)
        self.store.set_meta('spent_seconds', 0.04648137092590332)
        self.store.set_meta('snapshot', {'revision': 'captured', 'files': [{'path': 'input.txt'}]})
        plan_review(self.store, self.coordinator)
        claim = self.coordinator.claim_task('reviewer-1')
        self.assertIsNotNone(claim, 'Dividing remaining budget must leave each planned task claimable')
        checkpoint = handoff(self.store, now=self.now)
        self.assertGreaterEqual(checkpoint['remaining_seconds'], 0)

    def test_execute_expiry_requires_evidence_and_operator_resolution(self):
        task = self.enqueue('execute')
        self.enqueue()
        first = self.coordinator.claim_task('executor', lease_seconds=5)
        self.now += 6
        checkpoint = resume_run(self.store, self.coordinator)
        self.assertEqual(checkpoint['status'], 'paused')
        self.assertIn(task['id'], checkpoint['operator_decisions'])
        self.assertEqual(self.store.get_record('task', task['id'])['data']['status'], 'uncertain')
        self.budget(20, 0)
        self.assertIsNone(self.coordinator.claim_task('reader'))
        with self.assertRaises(ValueError):
            resolve_uncertain_task(self.store, task['id'], 'retry', '', now=self.now)
        self.assertEqual(self.store.get_record('task', task['id'])['data']['status'], 'uncertain')
        resolve_uncertain_task(self.store, task['id'], 'cancelled', 'Operator verified no effects', now=self.now)
        resume_run(self.store, self.coordinator)
        self.budget(20, 0)
        second = self.coordinator.claim_task('reader')
        self.assertIsNotNone(second)
        with self.assertRaises(ValueError):
            self.coordinator.complete_task(task['id'], 'executor', first['data']['lease_id'], {})
        self.now += 2
        self.coordinator.fail_task(second['id'], 'reader', second['data']['lease_id'], 'observed failure')
        self.budget(22, 0)


if __name__ == '__main__':
    unittest.main()
