"""Review planning uses real transactional storage and coordinator leases."""
from pathlib import Path
import tempfile
import unittest

from review_core.authorization import record_authorization
from review_core.coordinator import Coordinator
from review_core.delegation import plan_review
from review_core.store import Store


class DelegationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.temp.name) / 'run', create=True, metadata=dict(
            revision='rev', snapshot={'revision': 'rev', 'files': [{'path': 'a.py'}, {'path': 'b.py'}]},
            status='ready', budget_seconds=90, spent_seconds=0, max_workers=2))
        self.coordinator = Coordinator(self.store, clock=lambda: 100)
        record_authorization(self.store, 'rev', ['review'], ['.'])

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def test_blind_full_scope_claims_and_shared_budget(self):
        self.store.put_record('finding', 'peer-secret', {'claim': 'Never reveal this'})
        tasks = plan_review(self.store, self.coordinator)
        self.assertEqual(len(tasks), 2)
        for index, task in enumerate(tasks, 1):
            payload = task['data']['payload']
            self.assertEqual(payload['scope_paths'], ['a.py', 'b.py'])
            self.assertEqual(payload['timeout_seconds'], 30)
            self.assertNotIn('Never reveal this', str(payload))
            claimed = self.coordinator.claim_task(f'reviewer-{index}')
            self.assertEqual(claimed['id'], task['id'])
        self.assertIsNone(self.coordinator.claim_task('unassigned'))
        self.assertEqual(self.store.get_meta('budget_seconds'), 90)
        self.assertEqual(plan_review(self.store, self.coordinator),
                         [self.store.get_record('task', task['id']) for task in tasks])

    def test_only_requested_specialists_and_stable_identities(self):
        tasks = plan_review(self.store, self.coordinator, risks=['security', 'security'], specialists=['database'])
        self.assertEqual([task['data']['kind'] for task in tasks], ['review', 'review', 'specialist', 'specialist'])
        self.assertEqual({task['data']['payload'].get('focus') for task in tasks[2:]},
                         {'risk: security', 'specialist: database'})
        self.assertEqual(tasks, plan_review(self.store, self.coordinator, risks=['security'], specialists=['database']))
        with self.assertRaises(ValueError):
            plan_review(self.store, self.coordinator)
        self.assertEqual(len(self.store.list_records('task')), 4)

    def test_invalid_inputs_leave_no_partial_plan(self):
        for kwargs in ({'max_workers': 0}, {'max_workers': 3}, {'risks': ['']}, {'specialists': 'security'}):
            with self.assertRaises(ValueError):
                plan_review(self.store, self.coordinator, **kwargs)
            self.assertEqual(self.store.list_records('task'), [])
        self.store.set_meta('budget_seconds', 2)
        with self.assertRaises(ValueError):
            plan_review(self.store, self.coordinator)
        self.assertEqual(self.store.list_records('task'), [])

    def test_enqueue_error_rolls_back_whole_plan(self):
        original = self.coordinator.enqueue_task
        count = 0

        def failing_enqueue(*args, **kwargs):
            nonlocal count
            count += 1
            if count == 2:
                raise ValueError('Injected second enqueue rejection')
            return original(*args, **kwargs)

        self.coordinator.enqueue_task = failing_enqueue
        with self.assertRaises(ValueError):
            plan_review(self.store, self.coordinator)
        self.assertEqual(self.store.list_records('task'), [])
        self.assertEqual(self.store.list_records('review_plan'), [])

    def test_serial_cap_and_snapshot_validation(self):
        tasks = plan_review(self.store, self.coordinator, max_workers=1)
        self.assertEqual(self.store.get_meta('max_workers'), 1)
        self.assertEqual(tasks, plan_review(self.store, self.coordinator))
        self.assertIsNotNone(self.coordinator.claim_task('reviewer-1'))
        self.assertIsNone(self.coordinator.claim_task('reviewer-2'))

    def test_malformed_snapshot_is_rejected(self):
        for path in ('../a.py', '/a.py', './a.py'):
            self.store.set_meta('snapshot', {'revision': 'rev', 'files': [{'path': path}]})
            with self.assertRaises(ValueError):
                plan_review(self.store, self.coordinator)
            self.assertEqual(self.store.list_records('task'), [])


if __name__ == '__main__':
    unittest.main()
