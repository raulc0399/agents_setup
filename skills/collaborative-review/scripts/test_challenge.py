"""Current-context challenge gates using real storage, leases and evidence."""
from pathlib import Path
import tempfile
import unittest

from review_core.authorization import record_authorization
from review_core.challenge import challenge_gaps, current_context_fingerprint, schedule_challenge
from review_core.coordinator import Coordinator
from review_core.delegation import plan_review
from review_core.findings import create_coverage
from review_core.store import Store


class ChallengeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.temp.name) / 'run', create=True, metadata=dict(
            revision='rev', snapshot={'revision': 'rev', 'files': [{'path': 'a.py'}]},
            status='ready', challenge_required=True, budget_seconds=90, spent_seconds=0, max_workers=2))
        self.coordinator = Coordinator(self.store, clock=lambda: 100)
        record_authorization(self.store, 'rev', ['review'], ['.'])

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def reviews(self):
        from review_core.aasm_records import prepare_result
        from review_core.cli import _coordinator
        from review_core.discussions import schedule_discussions
        coordinator = _coordinator(self.store)
        coordinator.clock = self.coordinator.clock
        tasks = plan_review(self.store, self.coordinator)
        for index, task in enumerate(tasks, 1):
            worker = f'reviewer-{index}'
            claimed = coordinator.claim_task(worker)
            body = (f'COV id=coverage-{worker} paths=["a.py"] status=reviewed evidence=["read:a.py"]\n'
                    f'VOTE verdict=APPROVED evidence=["coverage-{worker}"]')
            coordinator.complete_task(task['id'], worker, claimed['data']['lease_id'],
                                      prepare_result(self.store, worker, body))
        for task in schedule_discussions(self.store, coordinator):
            worker = task['data']['payload']['worker']
            claimed = coordinator.claim_task(worker, task_id=task['id'])
            question = task['data']['payload']['question_ids'][0]
            body = f'A id=answer-{worker} question={question} text="Checked peer evidence" evidence=["read:a.py"]'
            coordinator.complete_task(task['id'], worker, claimed['data']['lease_id'],
                                      prepare_result(self.store, worker, body))
        return tasks

    def complete(self, task, coverage=True, verdict='APPROVED'):
        claimed = self.coordinator.claim_task('challenger')
        self.assertEqual(claimed['id'], task['id'])
        evidence = ['read:a.py']
        if coverage:
            proof = create_coverage(self.store, author='challenger', revision='rev',
                                    paths=['a.py'], status='reviewed', evidence=['read:a.py'])
            evidence.append(proof['id'])
        return self.coordinator.complete_task(task['id'], 'challenger', claimed['data']['lease_id'],
                                              dict(verdict=verdict, evidence_ids=evidence))

    def test_plan_and_two_completed_current_review_records_required(self):
        with self.assertRaises(ValueError):
            schedule_challenge(self.store, self.coordinator)
        tasks = plan_review(self.store, self.coordinator)
        with self.assertRaises(ValueError):
            schedule_challenge(self.store, self.coordinator)
        for index, task in enumerate(tasks, 1):
            worker = f'reviewer-{index}'
            claimed = self.coordinator.claim_task(worker)
            self.coordinator.complete_task(task['id'], worker, claimed['data']['lease_id'], {})
        with self.assertRaises(ValueError):
            schedule_challenge(self.store, self.coordinator)
        self.assertTrue(challenge_gaps(self.store))

    def test_idempotent_current_context_and_preserved_blind_payloads(self):
        tasks = self.reviews()
        original = [task['data']['payload'] for task in tasks]
        self.store.put_record('finding', 'f', dict(revision='rev', claim='integration issue'))
        self.store.put_record('question', 'q', dict(revision='rev', text='Recovery?'))
        task = schedule_challenge(self.store, self.coordinator)
        self.assertEqual(task, schedule_challenge(self.store, self.coordinator))
        self.assertIn('integration issue', task['data']['payload']['brief'])
        self.assertIn('Recovery?', task['data']['payload']['brief'])
        self.assertEqual(original, [self.store.get_record('task', entry['id'])['data']['payload'] for entry in tasks])
        self.assertIsNone(self.coordinator.claim_task('reviewer-1'))
        self.complete(task)
        self.assertEqual(challenge_gaps(self.store), [])
        self.assertEqual(schedule_challenge(self.store, self.coordinator)['id'], task['id'])

    def test_changed_evidence_requires_fresh_challenge(self):
        self.reviews()
        self.store.put_record('assumption', 'a', dict(revision='rev', statement='old'))
        old = schedule_challenge(self.store, self.coordinator)
        self.complete(old)
        self.store.put_record('assumption', 'a', dict(revision='rev', statement='changed'))
        self.assertTrue(challenge_gaps(self.store))
        new = schedule_challenge(self.store, self.coordinator)
        self.assertNotEqual(old['id'], new['id'])
        self.assertIn('changed', new['data']['payload']['brief'])
        self.complete(new)
        self.assertEqual(challenge_gaps(self.store), [])

    def test_completed_process_without_approved_explicit_coverage_blocks(self):
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        self.complete(task, coverage=False)
        self.assertIn('coverage', challenge_gaps(self.store)[0]['reason'])

    def test_blocked_verdict_blocks_even_with_coverage(self):
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        self.complete(task, verdict='BLOCKED')
        self.assertIn('APPROVED', challenge_gaps(self.store)[0]['reason'])

    def test_challenge_uses_protected_budget(self):
        self.reviews()
        self.store.set_meta('spent_seconds', 72)
        self.coordinator.enqueue_task('inspect', dict(revision='rev', brief='Other work',
            scope_paths=['a.py'], timeout_seconds=1), task_id='aaa-other')
        task = schedule_challenge(self.store, self.coordinator)
        self.assertIsNone(self.coordinator.claim_task('unassigned'))
        self.assertEqual(task['data']['payload']['timeout_seconds'], 18)
        self.assertEqual(task['data']['payload']['budget_plan_id'], self.store.list_records('review_plan')[0]['id'])
        self.complete(task)
        self.assertEqual(challenge_gaps(self.store), [])

    def test_stale_revision_and_reused_reviewer_do_not_approve(self):
        tasks = self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        self.complete(task)
        self.store.set_meta('revision', 'new')
        self.assertTrue(challenge_gaps(self.store))
        with self.assertRaises(ValueError):
            schedule_challenge(self.store, self.coordinator)
        self.store.set_meta('revision', 'rev')
        first = self.store.get_record('task', tasks[0]['id'])
        first['data']['worker'] = 'challenger'
        self.store.put_record('task', first['id'], first['data'])
        self.coordinator.record_review('challenger', 'rev', 'APPROVED', ['read:a.py'], first['id'])
        with self.assertRaises(ValueError):
            schedule_challenge(self.store, self.coordinator)
        self.assertTrue(challenge_gaps(self.store))

    def test_changed_pending_context_cancels_obsolete_task(self):
        self.reviews()
        old = schedule_challenge(self.store, self.coordinator)
        self.store.put_record('claim', 'claim-new', dict(revision='rev', statement='new support'))
        new = schedule_challenge(self.store, self.coordinator)
        self.assertNotEqual(old['id'], new['id'])
        self.assertEqual(self.store.get_record('task', old['id'])['data']['status'], 'cancelled')
        self.complete(new)
        self.assertEqual(challenge_gaps(self.store), [])

    def test_readonly_store_can_check_current_challenge(self):
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        self.complete(task)
        with Store(self.store.run_dir, readonly=True) as reader:
            before = reader.events()
            self.assertEqual(current_context_fingerprint(reader), task['data']['payload']['context_fingerprint'])
            self.assertEqual(challenge_gaps(reader), [])
            self.assertEqual(reader.events(), before)

    def test_failed_challenge_explicit_retry_keeps_identity_and_history(self):
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        claimed = self.coordinator.claim_task('challenger')
        self.coordinator.fail_task(task['id'], 'challenger', claimed['data']['lease_id'], 'Worker stopped')
        self.assertTrue(challenge_gaps(self.store))
        self.assertEqual(schedule_challenge(self.store, self.coordinator)['id'], task['id'])
        self.coordinator.retry_task(task['id'], 'Worker available again')
        completed = self.complete(task)
        self.assertEqual(completed['data']['attempt'], 2)
        self.assertEqual([attempt['status'] for attempt in completed['data']['attempts']],
                         ['failed', 'completed'])
        self.assertEqual(challenge_gaps(self.store), [])

    def test_replacement_challenge_uses_remaining_plan_reserve(self):
        self.reviews()
        old = schedule_challenge(self.store, self.coordinator)
        claimed = self.coordinator.claim_task('challenger')
        self.coordinator.clock = lambda: 105
        self.coordinator.fail_task(old['id'], 'challenger', claimed['data']['lease_id'], 'Stopped after five seconds')
        self.store.put_record('claim', 'new-context', dict(revision='rev', statement='Changed support'))
        replacement = schedule_challenge(self.store, self.coordinator)
        self.assertEqual(replacement['data']['payload']['timeout_seconds'], 13)
        self.assertEqual(schedule_challenge(self.store, self.coordinator)['id'], replacement['id'])
        claimed = self.coordinator.claim_task('challenger')
        self.assertEqual(claimed['id'], replacement['id'])
        self.assertEqual(claimed['data']['reserved_seconds'], 13)

    def test_running_challenge_allows_only_same_context_schedule(self):
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        self.coordinator.claim_task('challenger')
        self.assertEqual(schedule_challenge(self.store, self.coordinator)['id'], task['id'])
        self.store.put_record('claim', 'new-context', dict(revision='rev', statement='Changed support'))
        with self.assertRaisesRegex(ValueError, 'running'):
            schedule_challenge(self.store, self.coordinator)

    def test_exhausted_plan_challenge_reserve_rejects_replacement(self):
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        claimed = self.coordinator.claim_task('challenger')
        self.coordinator.clock = lambda: 130
        claimed['data']['execution'] = {'terminated': True}
        self.store.put_record('task', task['id'], claimed['data'])
        self.coordinator.fail_task(task['id'], 'challenger', claimed['data']['lease_id'], 'Timed out')
        self.store.put_record('claim', 'new-context', dict(revision='rev', statement='Changed support'))
        with self.assertRaisesRegex(ValueError, 'exhausted'):
            schedule_challenge(self.store, self.coordinator)

    def test_optional_challenge(self):
        self.store.set_meta('challenge_required', False)
        self.assertEqual(challenge_gaps(self.store), [])

    def test_language_agreement_and_peer_messages_invalidate_challenge(self):
        self.reviews()
        for kind, record_id, data in (
                ('dialect', 'terms', {'definitions': [{'op': 'CHECK'}]}),
                ('dialect_test', 'readback', {'passed': True}),
                ('dialect_ack', 'ack', {'participant': 'reviewer-1'}),
                ('message', 'peer', {'revision': 'rev', 'sender': 'reviewer-1', 'body': 'NOTE text=changed'}),
                ('aasm_domain', 'domain', {'revision': 'rev', 'author': 'reviewer-2', 'operation': 'CHECK'})):
            with self.subTest(kind=kind):
                task = schedule_challenge(self.store, self.coordinator)
                self.complete(task)
                self.assertEqual(challenge_gaps(self.store), [])
                self.store.put_record(kind, record_id, data)
                self.assertTrue(challenge_gaps(self.store))

    def test_challenger_messages_do_not_invalidate_their_own_result(self):
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        self.complete(task)
        self.store.put_record('message', 'result', dict(revision='rev', sender='challenger', body='VOTE verdict=APPROVED'))
        self.store.put_record('aasm_domain', 'domain', dict(revision='rev', author='challenger', operation='CHECK'))
        self.assertEqual(current_context_fingerprint(self.store), task['data']['payload']['context_fingerprint'])
        self.assertEqual(challenge_gaps(self.store), [])

    def test_task_result_binds_its_own_effects_but_later_edits_require_review(self):
        from review_core.aasm_records import prepare_result
        from review_core.cli import _coordinator
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        coordinator = _coordinator(self.store)
        claimed = coordinator.claim_task('challenger')
        body = ('ASSUME id=observed text="Nonblocking observation" paths=["a.py"] blocking=false\n'
                'COV id=proof paths=["a.py"] status=reviewed evidence=["read:a.py"]\n'
                'VOTE verdict=APPROVED evidence=["proof"]')
        result = prepare_result(self.store, 'challenger', body)
        completed = coordinator.complete_task(task['id'], 'challenger', claimed['data']['lease_id'], result)
        self.assertEqual(challenge_gaps(self.store), [])
        self.assertEqual(schedule_challenge(self.store, coordinator)['id'], task['id'])
        self.assertEqual(completed, self.store.get_record('task', task['id']))
        assumption = self.store.get_record('assumption', 'observed')
        assumption['data']['statement'] = 'Changed by a later reviewer'
        self.store.put_record('assumption', 'observed', assumption['data'], actor='reviewer-1')
        self.assertTrue(challenge_gaps(self.store))
        self.assertNotEqual(schedule_challenge(self.store, coordinator)['id'], task['id'])

    def test_changed_context_during_challenge_cannot_be_bound_as_reviewed(self):
        from review_core.aasm_records import prepare_result
        from review_core.cli import _coordinator
        self.reviews()
        task = schedule_challenge(self.store, self.coordinator)
        coordinator = _coordinator(self.store)
        claimed = coordinator.claim_task('challenger')
        self.store.put_record('question', 'late', dict(revision='rev', text='New evidence'))
        result = prepare_result(self.store, 'challenger',
            'COV id=proof paths=["a.py"] status=reviewed evidence=["read:a.py"]\n'
            'VOTE verdict=APPROVED evidence=["proof"]')
        with self.assertRaisesRegex(ValueError, 'context'):
            coordinator.complete_task(task['id'], 'challenger', claimed['data']['lease_id'], result)
        self.assertIsNone(self.store.get_record('coverage', 'proof'))
        self.assertEqual(self.store.get_record('task', task['id'])['data']['status'], 'running')


if __name__ == '__main__':
    unittest.main()
