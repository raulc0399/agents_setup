"""AASM transport transaction, identity, visibility and replay regressions."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import tempfile
import time
import unittest

from review_core.aasm import expand_operations, parse_document
from review_core.authorization import record_authorization
from review_core.dialects import acknowledge_dialect, propose_dialect, test_dialect as readback
from review_core.messages import read_messages, send_message
from review_core.store import Store


class MessageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name) / 'run'
        self.store = Store(self.path, create=True, metadata=dict(
            revision='rev', participants=['alice', 'bob', 'carol'], paths=['src']))
        record_authorization(self.store, 'rev', ['review'], ['src'])

    def tearDown(self):
        self.store.close()
        self.temp.cleanup()

    def task(self, worker='alice', status='running', blind=False, revision='rev'):
        return self.store.put_record('task', 'task-' + worker, dict(
            kind='review', payload=dict(revision=revision, scope_paths=['src'],
                                       reviewer=worker, blind=blind),
            worker=worker, lease_id='lease-' + worker, status=status,
            lease_expires=time.time() + 1000))

    def send(self, **kwargs):
        return send_message(self.store, 'alice', 'bob', 'NOTE text="Review src"', **kwargs)

    def test_cursor_replay_and_read_only_inbox(self):
        calls = []
        first = self.send(message_id='m1', apply=lambda ops, data: calls.append(ops))
        self.assertEqual(first, self.send(message_id='m1', apply=lambda ops, data: self.fail('reapplied')))
        self.assertEqual(len(calls), 1)
        with self.assertRaises(ValueError):
            send_message(self.store, 'alice', 'bob', 'NOTE text=changed', message_id='m1')
        second = self.send(message_id='m2')
        self.assertGreater(second['sequence'], first['sequence'])
        events = self.store.events()
        self.assertEqual(read_messages(self.store, 'bob', first['sequence']), [second])
        self.assertEqual(events, self.store.events())
        with Store(self.path, readonly=True) as readonly:
            self.assertEqual(read_messages(readonly, 'bob'), [first, second])

    def test_callback_failure_rolls_back_records_and_publication(self):
        before = self.store.events()
        def fail(operations, data):
            self.store.put_record('question', 'q1', dict(revision='rev'))
            raise ValueError('application failed')
        with self.assertRaisesRegex(ValueError, 'application failed'):
            self.send(message_id='failed', apply=fail)
        self.assertIsNone(self.store.get_record('question', 'q1'))
        self.assertIsNone(self.store.get_record('message', 'failed'))
        self.assertEqual(before, self.store.events())

    def test_unknown_recipient_and_missing_authorization_write_nothing(self):
        before = self.store.events()
        with self.assertRaises(ValueError):
            send_message(self.store, 'alice', 'unknown', 'NOTE text=hello')
        self.store.set_meta('revision', 'other')
        with self.assertRaises(PermissionError):
            self.send()
        self.assertEqual(before, self.store.events())

    def test_task_binding_expiry_and_completed_result(self):
        self.task()
        with self.assertRaises(PermissionError):
            self.send(task_id='task-alice', lease_id='wrong')
        task = self.store.get_record('task', 'task-alice')['data']
        task['lease_expires'] = 0
        self.store.put_record('task', 'task-alice', task)
        with self.assertRaises(PermissionError):
            self.send(task_id='task-alice', lease_id='lease-alice')
        self.task(status='completed')
        message = send_message(self.store, 'alice', 'coordinator', 'NOTE text=finished',
                               task_id='task-alice', lease_id='lease-alice')
        self.assertEqual(read_messages(self.store, 'coordinator'), [message])
        self.store.set_meta('revision', 'new')
        with self.assertRaises(ValueError):
            self.send(task_id='task-alice', lease_id='lease-alice')

    def test_blind_delivery_and_invisible_thread_references(self):
        self.task(blind=True)
        self.task(worker='bob', blind=True)
        message = self.send(task_id='task-alice', lease_id='lease-alice')
        self.assertEqual(read_messages(self.store, 'bob'), [])
        with self.assertRaises(ValueError):
            send_message(self.store, 'bob', 'alice', 'NOTE text=reply', reply_to=message['id'],
                         task_id='task-bob', lease_id='lease-bob')
        with self.assertRaises(ValueError):
            send_message(self.store, 'bob', 'alice', 'NOTE text=ref refs=["' + message['id'] + '"]',
                         task_id='task-bob', lease_id='lease-bob')
        self.task(status='completed', blind=True)
        self.assertEqual(read_messages(self.store, 'bob'), [])
        self.task(worker='bob', status='completed', blind=True)
        self.assertEqual(read_messages(self.store, 'bob'), [message])
        reply = send_message(self.store, 'bob', 'alice', 'NOTE text=reply', reply_to=message['id'],
                             task_id='task-bob', lease_id='lease-bob')
        self.assertEqual(reply['data']['reply_to'], message['id'])

    def test_foreign_and_cross_revision_threads(self):
        message = self.send()
        with self.assertRaises(ValueError):
            send_message(self.store, 'carol', 'bob', 'NOTE text=hello', reply_to=message['id'])
        self.store.set_meta('revision', 'next')
        record_authorization(self.store, 'next', ['review'], ['src'])
        with self.assertRaises(ValueError):
            self.send(reply_to=message['id'])
        self.assertEqual(read_messages(self.store, 'bob'), [])

    def test_unknown_and_new_batch_references(self):
        with self.assertRaises(ValueError):
            send_message(self.store, 'alice', 'bob', 'NOTE text=hello refs=["missing"]')
        message = send_message(self.store, 'alice', 'bob',
                               'Q id=q1 text="Which path?"\nNOTE text=context refs=["q1"]')
        self.assertEqual(len(message['data']['operations']), 2)
        self.assertIsNone(self.store.get_record('question', 'q1'))

    def test_improvement_and_existing_task_references_match_application(self):
        from review_core.aasm_records import apply_operations
        self.task()
        body = 'IMP proposal=p1 text="Clarify the task brief"\nNOTE text=context refs=["p1","task-alice"]'
        message = send_message(self.store, 'alice', 'bob', body,
            apply=lambda ops, data: apply_operations(self.store, 'alice', 'rev', ops))
        self.assertEqual(message['data']['body'], body)
        self.assertEqual(self.store.get_record('protocol_proposal', 'p1')['data']['status'], 'proposed')

    def test_concurrent_retry_publishes_and_applies_once(self):
        def send_once(_):
            with Store(self.path) as store:
                def apply(operations, data):
                    store.put_record('question', 'once', dict(revision='rev'), expected_version=0)
                return send_message(store, 'alice', 'bob', 'NOTE text=hello',
                                    message_id='concurrent', apply=apply)
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(send_once, range(2)))
        self.assertEqual(results[0], results[1])
        self.assertEqual(len(read_messages(self.store, 'bob')), 1)
        self.assertEqual(self.store.get_record('question', 'once')['version'], 1)

    def test_negotiated_dialect_preserves_expansion_and_membership(self):
        definitions = [{'op': 'OBS', 'meaning': 'An observation recorded as a note',
                        'fields': {'text': {'type': 'text', 'required': True}},
                        'expands_to': {'op': 'NOTE', 'args': {'text': {'field': 'text'}}}}]
        body = 'OBS text=observed'
        expected = expand_operations(parse_document(body, definitions), definitions)
        examples = [{'body': body, 'expected': expected}]
        dialect = propose_dialect(self.store, 'alice', 'observations', 1,
                                  ['alice', 'bob'], definitions, examples)
        with self.assertRaises(ValueError):
            send_message(self.store, 'alice', 'bob', body, dialect_id=dialect['id'])
        for participant in ('alice', 'bob'):
            readback(self.store, dialect['id'], participant, examples)
            acknowledge_dialect(self.store, dialect['id'], participant)
        message = send_message(self.store, 'alice', 'bob', body, dialect_id=dialect['id'])
        self.assertEqual(message['data']['operations'], expected)
        self.assertEqual(message['data']['body'], body)
        self.assertEqual(message['data']['dialect_digest'], dialect['data']['digest'])
        with self.assertRaises(ValueError):
            send_message(self.store, 'alice', 'carol', body, dialect_id=dialect['id'])

    def test_blind_references_hide_peer_records_and_unknown_provenance(self):
        self.task(blind=True)
        self.task(worker='bob', blind=True)
        fixtures = [('finding', 'f-private', {'author': 'alice'}),
                    ('coverage', 'c-private', {'reviewer': 'alice'}),
                    ('source', 's-private', {'author': 'alice'}),
                    ('aasm_domain', 'd-private', {'author': 'alice'}),
                    ('aasm_domain', 'd-unknown', {})]
        for kind, record_id, provenance in fixtures:
            self.store.put_record(kind, record_id, dict(revision='rev', **provenance))
            with self.subTest(kind=kind, id=record_id):
                with self.assertRaises(ValueError):
                    send_message(self.store, 'bob', 'alice',
                                 'NOTE text=reference refs=["' + record_id + '"]',
                                 task_id='task-bob', lease_id='lease-bob')
        with self.assertRaises(ValueError):
            send_message(self.store, 'bob', 'alice', 'FIX id=f-private evidence=["checked"]',
                         task_id='task-bob', lease_id='lease-bob')
        self.store.put_record('source', 's-own', dict(revision='rev', author='bob'))
        own = send_message(self.store, 'bob', 'alice', 'NOTE text=own refs=["s-own"]',
                           task_id='task-bob', lease_id='lease-bob')
        self.assertEqual(own['data']['operations'][0]['args']['refs'], ['s-own'])

    def test_hidden_inbound_message_blocks_cursor_until_blind_gate_opens(self):
        self.store.set_meta('participants', ['alice', 'bob', 'carol', 'coordinator'])
        self.task(blind=True)
        self.task(worker='bob', blind=True)
        hidden = self.send(task_id='task-alice', lease_id='lease-alice')
        later = send_message(self.store, 'coordinator', 'bob', 'NOTE text=administrative')
        self.assertEqual(read_messages(self.store, 'bob'), [])
        self.task(status='completed', blind=True)
        self.task(worker='bob', status='completed', blind=True)
        self.assertEqual(read_messages(self.store, 'bob'), [hidden, later])
        self.assertEqual(read_messages(self.store, 'bob', hidden['sequence']), [later])

    def test_completed_nonreview_result_uses_own_action_grant(self):
        self.store.set_meta('revision', 'inspection')
        for action in ('inspect', 'execute'):
            with self.subTest(action=action):
                record_authorization(self.store, 'inspection', [action], ['src'])
                task = self.task(status='completed', revision='inspection')
                data = task['data']
                data['kind'] = action
                data['payload']['required_action'] = action
                self.store.put_record('task', task['id'], data)
                result = send_message(self.store, 'alice', 'coordinator', 'NOTE text=finished',
                                      task_id=task['id'], lease_id='lease-alice')
                self.assertEqual(result['data']['recipient'], 'coordinator')
                with self.assertRaises(PermissionError):
                    self.send(task_id=task['id'], lease_id='lease-alice')
                data['status'] = 'running'
                self.store.put_record('task', task['id'], data)
                with self.assertRaises(PermissionError):
                    send_message(self.store, 'alice', 'coordinator', 'NOTE text=progress',
                                 task_id=task['id'], lease_id='lease-alice')

    def test_self_reference_and_invalid_cursor(self):
        with self.assertRaises(ValueError):
            self.send(message_id='self', reply_to='self')
        for after in (-1, True, 1.2):
            with self.assertRaises(ValueError):
                read_messages(self.store, 'bob', after)


if __name__ == '__main__':
    unittest.main()
