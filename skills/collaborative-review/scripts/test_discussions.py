"""Explicit peer questions schedule once without bypassing the blind gate."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tempfile
import unittest

from review_core.aasm_records import apply_operations
from review_core.authorization import record_authorization
from review_core.coordinator import Coordinator
from review_core.discussions import schedule_discussions
from review_core.messages import send_message
from review_core.store import Store


class DiscussionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name) / 'run'
        self.store = Store(self.path, create=True, metadata=dict(
            revision='rev', participants=['alice', 'bob'], status='ready', max_workers=2,
            budget_seconds=100, snapshot=dict(revision='rev', files=[dict(path='a.py')]), paths=['a.py']))
        self.coordinator = Coordinator(self.store)
        record_authorization(self.store, 'rev', ['review'], ['a.py'])

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def send(self, body, **kwargs):
        return send_message(self.store, 'alice', 'bob', body,
                            apply=lambda ops, data: apply_operations(self.store, 'alice', 'rev', ops), **kwargs)

    def test_note_prose_and_answers_do_not_schedule(self):
        self.send('NOTE text="Please review this?"')
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [])
        self.send('Q id=q1 text="Which caller fails?"', message_id='q')
        tasks = schedule_discussions(self.store, self.coordinator, 12)
        self.assertEqual(len(tasks), 1)
        payload = tasks[0]['data']['payload']
        self.assertEqual(payload['question_ids'], ['q1'])
        self.assertEqual(payload['reply_recipient'], 'alice')
        self.assertEqual(payload['worker'], 'bob')
        self.assertEqual(payload['scope_paths'], ['a.py'])
        self.assertEqual(payload['timeout_seconds'], 12)
        self.assertIn('Which caller fails?', payload['brief'])
        self.assertEqual(tasks, schedule_discussions(self.store, self.coordinator, 20))
        data = tasks[0]['data']
        data['status'] = 'failed'
        existing = self.store.put_record('task', tasks[0]['id'], data)
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [existing])

    def test_answered_questions_and_previous_revisions_skip(self):
        self.send('Q id=q1 text="Which caller?"')
        self.store.put_record('question', 'q1', dict(revision='rev', status='answered'))
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [])
        self.store.set_meta('revision', 'next')
        self.store.set_meta('snapshot', dict(revision='next', files=[dict(path='b.py')]))
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [])

    def test_blind_gate_and_concurrent_idempotence(self):
        self.send('GAP id=g1 text="Missing failure evidence"')
        self.store.put_record('task', 'blind', dict(kind='review', status='pending', payload=dict(
            revision='rev', blind=True, reviewer='alice')))
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [])
        blind = self.store.get_record('task', 'blind')['data']
        blind['status'] = 'completed'
        self.store.put_record('task', 'blind', blind)
        def schedule(_):
            with Store(self.path) as store:
                return schedule_discussions(store, Coordinator(store))
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(schedule, range(2)))
        self.assertEqual(results[0], results[1])
        self.assertEqual(len(results[0]), 1)
        self.assertEqual(len(self.store.list_records('task')), 2)

    def test_applied_answer_prevents_followup_and_unapplied_question_errors(self):
        self.send('Q id=q1 text="Which caller?"', message_id='source')
        send_message(self.store, 'bob', 'alice',
                     'A id=a1 question=q1 text="The import caller" evidence=["a.py:1"]',
                     reply_to='source',
                     apply=lambda ops, data: apply_operations(self.store, 'bob', 'rev', ops))
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [])
        send_message(self.store, 'alice', 'bob', 'Q id=raw text="Unapplied?"')
        before = self.store.events()
        with self.assertRaisesRegex(ValueError, 'question has not been applied: raw'):
            schedule_discussions(self.store, self.coordinator)
        self.assertEqual(before, self.store.events())

    def test_dispute_context_and_scheduler_failure_roll_back_batch(self):
        self.store.put_record('finding', 'f1', dict(revision='rev', author='alice'))
        self.send('DISPUTE id=d1 finding=f1 reason="Which failure path supports this?" evidence=["a.py:1"]', message_id='a')
        self.send('GAP id=g1 text="Where is the failure evidence?"', message_id='b')
        original = self.coordinator.enqueue_task
        calls = []
        def enqueue(*args, **kwargs):
            calls.append(args)
            if len(calls) == 2:
                raise ValueError('second enqueue failed')
            return original(*args, **kwargs)
        self.coordinator.enqueue_task = enqueue
        before = self.store.events()
        with self.assertRaisesRegex(ValueError, 'second enqueue failed'):
            schedule_discussions(self.store, self.coordinator)
        self.assertEqual(self.store.list_records('task'), [])
        self.assertEqual(before, self.store.events())
        self.coordinator.enqueue_task = original
        tasks = schedule_discussions(self.store, self.coordinator)
        self.assertEqual({tuple(task['data']['payload']['question_ids']) for task in tasks},
                         {('d1',), ('g1',)})
        self.assertTrue(any('Which failure path supports this?' in task['data']['payload']['brief']
                            for task in tasks))

    def answer(self, question_id):
        send_message(self.store, 'alice', 'bob',
                     f'A id=answer-{question_id} question={question_id} text="Observed caller" evidence=["a.py:1"]',
                     apply=lambda ops, data: apply_operations(self.store, 'alice', 'rev', ops))

    def test_pending_assignment_reconciles_remaining_questions(self):
        self.send('Q id=q1 text="First?"\nQ id=q2 text="Second?"', message_id='source')
        old = schedule_discussions(self.store, self.coordinator)[0]
        self.answer('q1')
        replacement = schedule_discussions(self.store, self.coordinator)[0]
        superseded = self.store.get_record('task', old['id'])
        self.assertEqual(superseded['data']['status'], 'cancelled')
        self.assertTrue(superseded['data']['discussion_superseded'])
        self.assertNotEqual(old['id'], replacement['id'])
        self.assertEqual(replacement['data']['payload']['question_ids'], ['q2'])
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [replacement])
        self.answer('q2')
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [])
        last = self.store.get_record('task', replacement['id'])
        self.assertEqual(last['data']['status'], 'cancelled')
        self.assertTrue(last['data']['discussion_superseded'])

    def test_running_assignment_waits_before_subset_replacement(self):
        self.send('Q id=q1 text="First?"\nQ id=q2 text="Second?"', message_id='source')
        old = schedule_discussions(self.store, self.coordinator)[0]
        running = self.coordinator.claim_task('bob')
        self.assertEqual(running['id'], old['id'])
        self.answer('q1')
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [running])
        self.assertEqual(len(self.store.list_records('task')), 1)
        data = running['data']
        data.update(status='cancelled', discussion_superseded=True)
        self.store.put_record('task', old['id'], data)
        replacement = schedule_discussions(self.store, self.coordinator)[0]
        self.assertEqual(replacement['data']['payload']['question_ids'], ['q2'])

    def test_discussion_gap_reply_does_not_schedule_loop_but_question_does(self):
        self.send('Q id=q1 text="Evidence?"', message_id='source')
        task = schedule_discussions(self.store, self.coordinator)[0]
        data = task['data']
        data.update(status='completed', worker='bob', lease_id='lease')
        self.store.put_record('task', task['id'], data)
        send_message(self.store, 'bob', 'alice', 'GAP id=g1 text="Need operator evidence"',
                     reply_to='source', task_id=task['id'], lease_id='lease',
                     apply=lambda ops, data: apply_operations(self.store, 'bob', 'rev', ops))
        self.assertEqual(schedule_discussions(self.store, self.coordinator),
                         [self.store.get_record('task', task['id'])])
        send_message(self.store, 'bob', 'alice', 'Q id=q2 text="Which operator evidence?"',
                     reply_to='source', task_id=task['id'], lease_id='lease',
                     apply=lambda ops, data: apply_operations(self.store, 'bob', 'rev', ops))
        tasks = schedule_discussions(self.store, self.coordinator)
        self.assertEqual(len(tasks), 2)
        self.assertTrue(any(t['data']['payload']['question_ids'] == ['q2'] for t in tasks))

    def test_failed_assignment_never_retries_for_smaller_subset(self):
        self.send('Q id=q1 text="First?"\nQ id=q2 text="Second?"', message_id='source')
        old = schedule_discussions(self.store, self.coordinator)[0]
        failed = self.store.put_record('task', old['id'], {**old['data'], 'status': 'failed'})
        self.answer('q1')
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [failed])
        self.assertEqual(len(self.store.list_records('task')), 1)

    def test_pending_text_edit_supersedes_same_question_ids(self):
        from review_core.findings import update_question
        self.send('Q id=q1 text="Original caller?"', message_id='source')
        original = schedule_discussions(self.store, self.coordinator)[0]
        update_question(self.store, question_id='q1', actor='alice', text='Which retry caller fails?')
        replacement = schedule_discussions(self.store, self.coordinator)[0]
        self.assertNotEqual(original['id'], replacement['id'])
        self.assertEqual(replacement['data']['status'], 'pending')
        self.assertEqual(replacement['data']['payload']['question_versions'], {'q1': 2})
        self.assertIn('Which retry caller fails?', replacement['data']['payload']['brief'])
        old = self.store.get_record('task', original['id'])
        self.assertEqual(old['data']['status'], 'cancelled')
        self.assertTrue(old['data']['discussion_superseded'])

    def test_reopened_question_does_not_reuse_cancelled_assignment(self):
        from review_core.findings import update_question
        self.send('Q id=q1 text="First?"\nQ id=q2 text="Second?"', message_id='source')
        original = schedule_discussions(self.store, self.coordinator)[0]
        self.answer('q1')
        subset = schedule_discussions(self.store, self.coordinator)[0]
        update_question(self.store, question_id='q1', actor='alice', text='First with retry?')
        replacement = schedule_discussions(self.store, self.coordinator)[0]
        self.assertNotIn(replacement['id'], (original['id'], subset['id']))
        self.assertEqual(replacement['data']['status'], 'pending')
        self.assertEqual(replacement['data']['payload']['question_versions'], {'q1': 2, 'q2': 1})
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [replacement])
        self.assertEqual([t['id'] for t in self.store.list_records('task')
                          if t['data']['status'] == 'pending'], [replacement['id']])

    def test_invalid_timeout_writes_nothing(self):
        self.send('Q id=q1 text="What happens?"')
        before = self.store.events()
        for timeout in (0, True, float('inf')):
            with self.assertRaises(ValueError):
                schedule_discussions(self.store, self.coordinator, timeout)
        self.assertEqual(before, self.store.events())


if __name__ == '__main__':
    unittest.main()
