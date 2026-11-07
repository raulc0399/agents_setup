"""Coordinator lifecycle tests against transactional on-disk stores."""
import concurrent.futures
from pathlib import Path
import tempfile
import unittest

from review_core.authorization import record_authorization
from review_core.coordinator import Coordinator
from review_core.store import Store


class CoordinatorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name) / 'run'
        self.store = Store(self.path, create=True, metadata=dict(revision='rev', status='ready',
            budget_seconds=100, spent_seconds=0, max_workers=2))
        self.now = 1000
        self.coordinator = Coordinator(self.store, clock=lambda: self.now)
        record_authorization(self.store, 'rev', ['review', 'execute', 'inspect'], ['.'])

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def enqueue(self, kind='review', **kwargs):
        payload = dict(revision='rev', brief='Read pinned files', timeout_seconds=40, scope_paths=['src'])
        payload.update(kwargs)
        return self.coordinator.enqueue_task(kind, payload)

    def test_availability_matches_targeted_claim_and_is_readonly(self):
        task = self.enqueue(reviewer='alice')
        before = self.store.events()
        available = self.coordinator.availability()
        self.assertEqual(available['runnable_task_ids'], [task['id']])
        self.assertEqual(before, self.store.events())
        self.assertEqual(self.coordinator.claim_task('alice', task_id=task['id'])['id'], task['id'])
        self.now += 41
        available = self.coordinator.availability()
        self.assertTrue(available['stalled'])
        self.assertIn('recovery', [b['code'] for t in available['tasks'] for b in t['blockers']])

    def test_progress_is_explicit_idempotent_and_lease_bound(self):
        task = self.enqueue()
        claimed = self.coordinator.claim_task('alice')
        lease = claimed['data']['lease_id']
        first = self.coordinator.record_progress(task['id'], 'alice', lease, 'Traced callers', ['src:1'])
        self.now += 2
        self.coordinator.renew_task(task['id'], 'alice', lease)
        again = self.coordinator.record_progress(task['id'], 'alice', lease, 'Traced callers', ['src:1'])
        self.assertEqual(first['data']['progress'], again['data']['progress'])
        with self.assertRaises(ValueError):
            self.coordinator.record_progress(task['id'], 'bob', lease, 'Wrong worker')

    def test_discussion_uses_only_unprotected_remaining_budget(self):
        self.store.set_meta('budget_seconds', 50)
        review = self.enqueue(timeout_seconds=20)
        self.store.put_record('review_plan', 'plan', {'request': {'revision': 'rev'},
            'task_ids': [review['id']], 'challenge_budget_seconds': 20})
        discussion = self.enqueue('discussion', timeout_seconds=40, worker='peer')
        claim = self.coordinator.claim_task('peer', task_id=discussion['id'])
        self.assertEqual(claim['data']['attempt_budget_seconds'], 10)

    def test_availability_reports_actual_failed_dependency_and_budget(self):
        first = self.enqueue(reviewer='alice')
        dependent = self.coordinator.enqueue_task('review', first['data']['payload'], [first['id']])
        claim = self.coordinator.claim_task('alice', task_id=first['id'])
        self.coordinator.fail_task(first['id'], 'alice', claim['data']['lease_id'], 'bad output')
        available = self.coordinator.availability()
        row = next(row for row in available['tasks'] if row['task_id'] == dependent['id'])
        self.assertIn('is failed', row['blockers'][0]['reason'])
        self.assertIsNone(self.coordinator.claim_task('alice', task_id=dependent['id']))
        self.store.set_meta('budget_seconds', 1)
        fresh = self.enqueue(reviewer='bob')
        row = next(row for row in self.coordinator.availability()['tasks'] if row['task_id'] == fresh['id'])
        self.assertIn('budget', [blocker['code'] for blocker in row['blockers']])
        self.assertIsNone(self.coordinator.claim_task('bob', task_id=fresh['id']))

    def test_progress_rejects_string_evidence_and_old_revision(self):
        task = self.enqueue()
        claim = self.coordinator.claim_task('alice')
        with self.assertRaises(ValueError):
            self.coordinator.record_progress(task['id'], 'alice', claim['data']['lease_id'], 'Read files', 'src:1')
        self.store.set_meta('revision', 'new')
        with self.assertRaises(ValueError):
            self.coordinator.record_progress(task['id'], 'alice', claim['data']['lease_id'], 'Read files')

    def test_running_availability_does_not_report_claim_budget_or_capacity(self):
        task = self.enqueue()
        self.coordinator.claim_task('alice')
        row = self.coordinator.availability()['tasks'][0]
        self.assertEqual([blocker['code'] for blocker in row['blockers']], ['running'])

    def test_targeted_availability_does_not_expose_unrelated_runnable_tasks(self):
        task = self.enqueue()
        missing = self.coordinator.availability(task_id='missing')
        self.assertEqual(missing['runnable_task_ids'], [])
        self.assertEqual([row['task_id'] for row in missing['tasks']], ['missing'])
        self.assertEqual(missing['tasks'][0]['blockers'][0]['code'], 'missing')
        claimed = self.coordinator.claim_task('alice', task_id=task['id'])
        self.coordinator.complete_task(task['id'], 'alice', claimed['data']['lease_id'], {})
        completed = self.coordinator.availability(task_id=task['id'])
        self.assertEqual(completed['tasks'][0]['blockers'][0]['code'], 'terminal')
        stale = self.enqueue()
        self.store.set_meta('revision', 'new')
        old = self.coordinator.availability(task_id=stale['id'])
        self.assertEqual(old['runnable_task_ids'], [])
        self.assertIn('revision', [blocker['code'] for blocker in old['tasks'][0]['blockers']])

    def test_identity_dependencies_and_idempotence(self):
        first = self.enqueue(reviewer='alice')
        dependent = self.coordinator.enqueue_task('review', first['data']['payload'], [first['id']])
        self.assertIsNone(self.coordinator.claim_task('bob'))
        claimed = self.coordinator.claim_task('alice')
        with self.assertRaises(ValueError):
            self.coordinator.complete_task(first['id'], 'bob', claimed['data']['lease_id'], {})
        result = {'evidence': ['src:1'], 'coverage': ['src']}
        self.now += 5
        done = self.coordinator.complete_task(first['id'], 'alice', claimed['data']['lease_id'], result)
        self.assertEqual(done, self.coordinator.complete_task(first['id'], 'alice', claimed['data']['lease_id'], result))
        self.assertEqual(self.store.get_meta('spent_seconds'), 5)
        self.assertEqual(self.coordinator.claim_task('alice')['id'], dependent['id'])
        self.assertEqual(done['data']['attempts'][0]['status'], 'completed')

    def test_expiry_renewal_and_cancellation(self):
        task = self.enqueue()
        claimed = self.coordinator.claim_task('alice', lease_seconds=10)
        lease = claimed['data']['lease_id']
        self.now += 5
        renewed = self.coordinator.renew_task(task['id'], 'alice', lease, 100)
        self.assertEqual(renewed['data']['lease_expires'], 1040)
        self.now = 1040
        for method, value in ((self.coordinator.fail_task, 'failed'), (self.coordinator.complete_task, {})):
            with self.assertRaises(ValueError):
                method(task['id'], 'alice', lease, value)
        cancelled = self.coordinator.cancel_task(task['id'], 'operator stopped')
        self.assertEqual(cancelled['data']['status'], 'running')
        self.assertEqual(cancelled['data']['reserved_seconds'], 40)
        self.assertEqual(self.store.get_meta('spent_seconds'), 0)

    def test_execute_exclusion_pause_and_action_spoof(self):
        with self.assertRaises(ValueError):
            self.enqueue('execute', required_action='review')
        self.enqueue('execute')
        self.enqueue()
        self.coordinator.pause('check scope')
        self.assertIsNone(self.coordinator.claim_task('alice'))
        self.coordinator.resume()
        claimed = self.coordinator.claim_task('alice', capabilities=['execute'])
        self.assertEqual(claimed['data']['kind'], 'execute')
        self.assertIsNone(self.coordinator.claim_task('bob'))

    def test_revision_json_and_review_gate(self):
        with self.assertRaises(ValueError):
            self.enqueue(revision='old')
        task = self.enqueue()
        claim = self.coordinator.claim_task('alice')
        with self.assertRaises(ValueError):
            self.coordinator.complete_task(task['id'], 'alice', claim['data']['lease_id'], [])
        self.coordinator.complete_task(task['id'], 'alice', claim['data']['lease_id'], {'verdict': 'APPROVED'})
        self.assertEqual(self.coordinator.readiness()['result'], 'BLOCKED')
        with self.assertRaises(ValueError):
            self.coordinator.record_review('bob', 'rev', 'APPROVED', ['attestation'], task['id'])
        self.coordinator.record_review('alice', 'rev', 'APPROVED', ['src:1'], task['id'])
        self.coordinator.record_review('alice', 'rev', 'APPROVED', ['src:2'])
        self.assertEqual(self.coordinator.readiness()['result'], 'BLOCKED')
        self.coordinator.record_review('bob', 'rev', 'APPROVED', ['src:3'])
        self.assertEqual(self.coordinator.readiness()['result'], 'APPROVED')
        self.store.set_meta('revision', 'new')
        self.assertEqual(self.coordinator.readiness()['result'], 'BLOCKED')

    def test_parallel_claims_cannot_oversubscribe_budget(self):
        self.store.set_meta('budget_seconds', 50)
        self.enqueue()
        self.enqueue()
        def claim(worker):
            with Store(self.path) as store:
                return Coordinator(store, clock=lambda: 1000).claim_task(worker)
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(claim, ['alice', 'bob']))
        self.assertEqual(sum(result is not None for result in results), 1)

    def test_result_handler_atomicity_and_duplicate_completion(self):
        task = self.enqueue()
        claim = self.coordinator.claim_task('alice')
        calls = []
        def handler(completed, result):
            calls.append(completed['data']['status'])
            self.store.set_meta('imported', True)
            raise ValueError('Invalid finding')
        self.coordinator.result_handler = handler
        self.now += 3
        with self.assertRaises(ValueError):
            self.coordinator.complete_task(task['id'], 'alice', claim['data']['lease_id'], {})
        self.assertEqual(self.store.get_record('task', task['id'])['data']['status'], 'running')
        self.assertEqual(self.store.get_meta('spent_seconds'), 0)
        self.assertIsNone(self.store.get_meta('imported'))
        self.coordinator.result_handler = lambda completed, result: calls.append(completed['data']['status'])
        self.coordinator.complete_task(task['id'], 'alice', claim['data']['lease_id'], {})
        self.coordinator.complete_task(task['id'], 'alice', claim['data']['lease_id'], {})
        self.assertEqual(calls, ['completed', 'completed'])

    def test_cancel_retains_live_capacity_until_acknowledged(self):
        task = self.enqueue('execute')
        claim = self.coordinator.claim_task('alice')
        self.enqueue()
        requested = self.coordinator.cancel_task(task['id'], 'stop process')
        self.assertEqual(requested['data']['status'], 'running')
        self.assertEqual(requested['data']['reserved_seconds'], 40)
        self.assertEqual(requested['data']['cancellation_requested'], 'stop process')
        self.assertIsNone(self.coordinator.claim_task('bob'))
        calls = []
        self.coordinator.result_handler = lambda *args: calls.append(args)
        done = self.coordinator.complete_task(task['id'], 'alice', claim['data']['lease_id'], {'verdict': 'APPROVED'})
        self.assertEqual(done['data']['status'], 'cancelled')
        self.assertIsNone(done['data']['result'])
        self.assertEqual(calls, [])
        self.assertIsNotNone(self.coordinator.claim_task('bob'))

    def test_cancel_failure_acknowledges_without_accepting_error(self):
        task = self.enqueue()
        claim = self.coordinator.claim_task('alice')
        self.coordinator.cancel_task(task['id'], 'operator cancelled')
        done = self.coordinator.fail_task(task['id'], 'alice', claim['data']['lease_id'], 'process stopped')
        self.assertEqual(done['data']['status'], 'cancelled')
        self.assertEqual(done['data']['error'], 'operator cancelled')

    def test_old_running_execute_blocks_new_revision_approval(self):
        self.enqueue('execute')
        self.coordinator.claim_task('alice')
        self.store.set_meta('revision', 'new')
        for reviewer in ('one', 'two'):
            self.coordinator.record_review(reviewer, 'new', 'APPROVED', ['evidence'])
        self.assertEqual(self.coordinator.readiness()['result'], 'BLOCKED')

    def test_invalid_scope_is_rejected_before_queueing(self):
        for paths in ([], ['../escape'], ['/absolute'], ['a/../b']):
            with self.subTest(paths=paths):
                with self.assertRaises(ValueError):
                    self.enqueue(scope_paths=paths)
        self.assertEqual(self.store.list_records('task'), [])

    def test_review_identity_tuple_cannot_collide(self):
        self.store.set_meta('revision', 'b-c')
        first = self.coordinator.record_review('a', 'b-c', 'APPROVED', ['evidence'])
        self.store.set_meta('revision', 'c')
        second = self.coordinator.record_review('a-b', 'c', 'BLOCKED', ['evidence'])
        self.assertNotEqual(first['id'], second['id'])
        self.assertEqual(len(self.store.list_records('review')), 2)

    def test_review_plan_budget_cannot_be_taken_by_inspection(self):
        first = self.enqueue(timeout_seconds=30, reviewer='one')
        second = self.enqueue(timeout_seconds=30, reviewer='two')
        self.store.put_record('review_plan', 'plan-test', {
            'request': {'revision': 'rev'}, 'task_ids': [first['id'], second['id']],
            'challenge_budget_seconds': 30})
        self.enqueue('inspect', timeout_seconds=20)
        self.assertIsNone(self.coordinator.claim_task('inspector', capabilities=['inspect']))
        for worker in ('one', 'two'):
            claimed = self.coordinator.claim_task(worker, capabilities=['review'])
            self.assertIsNotNone(claimed)
            self.coordinator.complete_task(claimed['id'], worker, claimed['data']['lease_id'], {})
        self.enqueue('challenge', timeout_seconds=30, budget_plan_id='plan-test')
        challenge = self.coordinator.claim_task('challenger', capabilities=['challenge'])
        self.assertIsNotNone(challenge)
        self.assertIsNotNone(self.coordinator.claim_task('inspector', capabilities=['inspect']))

    def test_uncertain_execution_blocks_claims_and_approval_across_revisions(self):
        task = self.enqueue('execute')
        data = task['data']
        data['status'] = 'uncertain'
        self.store.put_record('task', task['id'], data)
        self.store.set_meta('revision', 'new')
        self.enqueue(revision='new')
        record_authorization(self.store, 'new', ['review'], ['.'])
        for reviewer in ('one', 'two'):
            self.coordinator.record_review(reviewer, 'new', 'APPROVED', ['evidence'])
        with self.assertRaises(ValueError):
            self.coordinator.cancel_task(task['id'], 'skip evidence')
        self.assertIsNone(self.coordinator.claim_task('worker'))
        self.assertIn('Uncertain task effects require operator resolution', self.coordinator.readiness()['blockers'])

    def test_expired_stopped_worker_can_acknowledge_failure_or_cancellation(self):
        for cancelled in (False, True):
            with self.subTest(cancelled=cancelled):
                task = self.enqueue()
                claim = self.coordinator.claim_task('alice', lease_seconds=2)
                lease = claim['data']['lease_id']
                if cancelled:
                    self.coordinator.cancel_task(task['id'], 'stop requested')
                self.now += 3
                with self.assertRaises(ValueError):
                    self.coordinator.fail_task(task['id'], 'alice', lease, 'timeout')
                current = self.store.get_record('task', task['id'])
                current['data']['execution'] = {'terminated': True}
                self.store.put_record('task', task['id'], current['data'])
                with self.assertRaises(ValueError):
                    self.coordinator.fail_task(task['id'], 'bob', lease, 'timeout')
                with self.assertRaises(ValueError):
                    self.coordinator.fail_task(task['id'], 'alice', 'stale', 'timeout')
                with self.assertRaises(ValueError):
                    self.coordinator.complete_task(task['id'], 'alice', lease, {})
                done = self.coordinator.fail_task(task['id'], 'alice', lease, 'timeout')
                self.assertEqual(done['data']['status'], 'cancelled' if cancelled else 'failed')
                self.assertEqual(done['data']['reserved_seconds'], 0)

    def test_failed_challenge_retry_preserves_history_and_issues_new_lease(self):
        task = self.enqueue('challenge')
        first = self.coordinator.claim_task('challenger')
        self.coordinator.fail_task(task['id'], 'challenger', first['data']['lease_id'], 'transient failure')
        retried = self.coordinator.retry_task(task['id'], 'adapter recovered')
        self.assertEqual(retried['id'], task['id'])
        self.assertEqual(retried['data']['attempts'][0]['status'], 'failed')
        second = self.coordinator.claim_task('challenger')
        self.assertEqual(second['data']['attempt'], 2)
        self.assertNotEqual(second['data']['lease_id'], first['data']['lease_id'])
        with self.assertRaises(ValueError):
            self.coordinator.complete_task(task['id'], 'challenger', first['data']['lease_id'], {})
        self.coordinator.complete_task(task['id'], 'challenger', second['data']['lease_id'], {})
        with self.assertRaises(ValueError):
            self.coordinator.retry_task(task['id'], 'already completed')

    def test_retry_rejects_uncertain_old_revision_and_unstopped_execution(self):
        task = self.enqueue('execute')
        claim = self.coordinator.claim_task('worker')
        self.coordinator.fail_task(task['id'], 'worker', claim['data']['lease_id'], 'failed')
        data = self.store.get_record('task', task['id'])['data']
        data['execution'] = {'launch_intent': True, 'terminated': False}
        self.store.put_record('task', task['id'], data)
        with self.assertRaises(ValueError):
            self.coordinator.retry_task(task['id'], 'unsafe')
        data['execution']['terminated'] = True
        self.store.put_record('task', task['id'], data)
        self.store.set_meta('revision', 'new')
        with self.assertRaises(ValueError):
            self.coordinator.retry_task(task['id'], 'stale')
        self.store.set_meta('revision', 'rev')
        data['status'] = 'uncertain'
        self.store.put_record('task', task['id'], data)
        with self.assertRaises(ValueError):
            self.coordinator.retry_task(task['id'], 'uncertain')

    def test_retry_uses_remaining_logical_allocation(self):
        self.store.set_meta('budget_seconds', 120)
        first = self.enqueue(reviewer='one')
        second = self.enqueue(reviewer='two')
        self.store.put_record('review_plan', 'plan-test', {
            'request': {'revision': 'rev'}, 'task_ids': [first['id'], second['id']],
            'challenge_budget_seconds': 40})
        claim = self.coordinator.claim_task('one')
        self.now += 0.125
        self.coordinator.fail_task(first['id'], 'one', claim['data']['lease_id'], 'transient')
        self.coordinator.retry_task(first['id'], 'retry adapter')
        retried = self.coordinator.claim_task('one')
        self.assertIsNotNone(retried)
        self.assertEqual(retried['data']['attempt_budget_seconds'], 39.875)
        self.assertEqual(retried['data']['payload']['timeout_seconds'], 40)
        self.assertEqual(retried['data']['spent_seconds'], 0.125)
        self.assertEqual(retried['data']['attempts'][0]['spent_seconds'], 0.125)
        self.assertLessEqual(self.coordinator.renew_task(first['id'], 'one',
            retried['data']['lease_id'], 300)['data']['lease_expires'], self.now + 39.875)
        self.assertIsNotNone(self.coordinator.claim_task('two'))

    def test_fractional_plan_claim_clamps_roundoff_without_oversubscription(self):
        budget = 100.0
        self.store.set_meta('budget_seconds', budget)
        share = budget / 3
        first = self.enqueue(timeout_seconds=share, reviewer='one')
        second = self.enqueue(timeout_seconds=share, reviewer='two')
        self.store.put_record('review_plan', 'plan-test', {
            'request': {'revision': 'rev'}, 'task_ids': [first['id'], second['id']],
            'challenge_budget_seconds': share})
        claimed = self.coordinator.claim_task('one')
        self.assertIsNotNone(claimed)
        self.assertLessEqual(claimed['data']['reserved_seconds'], budget - 2 * share)

    def test_exhausted_task_retry_rejects_without_mutation(self):
        task = self.enqueue(timeout_seconds=1)
        claim = self.coordinator.claim_task('worker')
        self.now += 2
        current = self.store.get_record('task', task['id'])
        current['data']['execution'] = {'terminated': True}
        self.store.put_record('task', task['id'], current['data'])
        failed = self.coordinator.fail_task(task['id'], 'worker', claim['data']['lease_id'], 'timeout')
        with self.assertRaisesRegex(ValueError, 'task budget exhausted; new task/run needed'):
            self.coordinator.retry_task(task['id'], 'try again')
        self.assertEqual(self.store.get_record('task', task['id']), failed)

    def test_completed_challenge_does_not_unprotect_pending_replacement(self):
        self.store.put_record('review_plan', 'plan-test', {
            'request': {'revision': 'rev'}, 'task_ids': [],
            'challenge_budget_seconds': 30})
        old = self.enqueue('challenge', timeout_seconds=30, budget_plan_id='plan-test')
        claimed = self.coordinator.claim_task('challenger', capabilities=['challenge'])
        self.now += 5
        self.coordinator.complete_task(old['id'], 'challenger', claimed['data']['lease_id'], {})
        replacement = self.enqueue('challenge', timeout_seconds=25, budget_plan_id='plan-test')
        self.enqueue('inspect', timeout_seconds=80)
        self.assertIsNone(self.coordinator.claim_task('inspector', capabilities=['inspect']))
        next_claim = self.coordinator.claim_task('challenger', capabilities=['challenge'])
        self.assertEqual(next_claim['id'], replacement['id'])
        self.assertEqual(next_claim['data']['reserved_seconds'], 25)

    def test_superseded_challenge_cannot_retry_or_claim(self):
        from review_core.challenge import current_context_fingerprint
        self.store.put_record('review_plan', 'plan-test', {
            'request': {'revision': 'rev'}, 'task_ids': [], 'challenge_budget_seconds': 30})
        fingerprint = current_context_fingerprint(self.store)
        old = self.enqueue('challenge', timeout_seconds=30, budget_plan_id='plan-test',
                           context_fingerprint=fingerprint, reviewer='old-challenger')
        claim = self.coordinator.claim_task('old-challenger', capabilities=['challenge'])
        self.now += 5
        self.coordinator.fail_task(old['id'], 'old-challenger', claim['data']['lease_id'], 'transient')
        self.store.set_meta('integration_surfaces', ['changed API'])
        replacement = self.enqueue('challenge', timeout_seconds=25, budget_plan_id='plan-test',
                                   context_fingerprint=current_context_fingerprint(self.store), reviewer='challenger')
        with self.assertRaisesRegex(ValueError, 'Challenge context is stale'):
            self.coordinator.retry_task(old['id'], 'superseded')
        old_data = self.store.get_record('task', old['id'])['data']
        old_data['status'] = 'pending'
        self.store.put_record('task', old['id'], old_data)
        self.assertIsNone(self.coordinator.claim_task('old-challenger', capabilities=['challenge']))
        current = self.coordinator.claim_task('challenger', capabilities=['challenge'])
        self.assertEqual(current['id'], replacement['id'])
        self.assertEqual(current['data']['attempt_budget_seconds'], 25)

    def test_bound_challenge_attempt_cannot_exceed_shared_plan_remainder(self):
        self.store.put_record('review_plan', 'plan-test', {
            'request': {'revision': 'rev'}, 'task_ids': [], 'challenge_budget_seconds': 30})
        old = self.enqueue('challenge', timeout_seconds=30, budget_plan_id='plan-test', reviewer='first')
        claim = self.coordinator.claim_task('first', capabilities=['challenge'])
        self.now += 5
        self.coordinator.fail_task(old['id'], 'first', claim['data']['lease_id'], 'transient')
        replacement = self.enqueue('challenge', timeout_seconds=25, budget_plan_id='plan-test', reviewer='second')
        self.coordinator.retry_task(old['id'], 'same-context retry')
        retry = self.coordinator.claim_task('first', capabilities=['challenge'])
        self.assertIsNone(self.coordinator.claim_task('second', capabilities=['challenge']))
        self.now += 24
        self.coordinator.complete_task(retry['id'], 'first', retry['data']['lease_id'], {})
        next_claim = self.coordinator.claim_task('second', capabilities=['challenge'])
        self.assertEqual(next_claim['id'], replacement['id'])
        self.assertEqual(next_claim['data']['attempt_budget_seconds'], 1)
        self.assertEqual(next_claim['data']['reserved_seconds'], 1)

    def test_authorization_required(self):
        self.enqueue()
        self.store.set_meta('revision', 'unapproved')
        self.enqueue(revision='unapproved')
        self.assertIsNone(self.coordinator.claim_task('alice'))


if __name__ == '__main__':
    unittest.main()
