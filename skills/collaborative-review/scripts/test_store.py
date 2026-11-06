"""Durability, transaction and concurrent writer tests using temporary runs."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import sqlite3
import tempfile
import threading
import unittest

from review_core.store import ConflictError, Store, StoreError


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.run = Path(self.temp.name) / 'run'
        self.store = Store(self.run, create=True, metadata={'revision': 'abc', 'policies': {'blind': True}})
        self.addCleanup(self.store.close)

    def test_durable_records_metadata_and_events(self):
        first = self.store.put_record('finding', 'b', {'claim': 'Unicode ✓'}, actor='reviewer', expected_version=0)
        self.store.put_record('finding', 'a', {'claim': 'earlier'})
        updated = self.store.put_record('finding', 'b', {'claim': 'verified'}, expected_version=1)
        self.assertEqual(first['version'], 1)
        self.assertEqual(updated['version'], 2)
        self.assertIsInstance(updated['updated_at'], float)
        with Store(self.run) as reopened:
            self.assertEqual(reopened.get_meta('revision'), 'abc')
            self.assertEqual(reopened.get_meta('absent', 7), 7)
            self.assertEqual([record['id'] for record in reopened.list_records('finding')], ['a', 'b'])
            self.assertEqual(reopened.get_record('finding', 'b'), updated)
            events = reopened.events()
            self.assertEqual([event['seq'] for event in events], [1, 2, 3])
            self.assertEqual(events[0]['actor'], 'reviewer')
            self.assertEqual(events[0]['payload']['data'], first['data'])
            self.assertEqual(reopened.events(after=2), events[2:])

    def test_outer_rollback_includes_all_nested_updates(self):
        with self.assertRaisesRegex(RuntimeError, 'abort'):
            with self.store.transaction():
                self.store.set_meta('revision', 'changed')
                self.store.put_record('task', 'one', {'status': 'running'})
                with self.store.transaction():
                    self.store.append_event('manual', 'one', {'note': 'nested'})
                raise RuntimeError('abort')
        self.assertEqual(self.store.get_meta('revision'), 'abc')
        self.assertIsNone(self.store.get_record('task', 'one'))
        self.assertEqual(self.store.events(), [])

    def test_nested_failure_can_be_caught_without_aborting_outer(self):
        with self.store.transaction():
            self.store.put_record('task', 'before', {})
            try:
                with self.store.transaction():
                    self.store.put_record('task', 'discard', {})
                    self.store.set_meta('revision', 'discard')
                    raise ValueError('nested failure')
            except ValueError:
                pass
            self.store.put_record('task', 'after', {})
        self.assertEqual([item['id'] for item in self.store.list_records('task')], ['after', 'before'])
        self.assertEqual(self.store.get_meta('revision'), 'abc')
        self.assertEqual(len(self.store.events()), 2)

    def test_record_and_event_are_atomic(self):
        self.store.connection.execute('''CREATE TRIGGER reject_event BEFORE INSERT ON events
            BEGIN SELECT RAISE(ABORT, 'event unavailable'); END''')
        with self.assertRaises(sqlite3.IntegrityError):
            self.store.put_record('task', 'one', {})
        self.assertIsNone(self.store.get_record('task', 'one'))
        self.assertEqual(self.store.events(), [])

    def test_optimistic_versions_reject_absence_and_stale_updates(self):
        with self.assertRaises(ConflictError):
            self.store.put_record('task', 'one', {}, expected_version=1)
        self.store.put_record('task', 'one', {'value': 1}, expected_version=0)
        for version in (0, 2):
            with self.assertRaises(ConflictError):
                self.store.put_record('task', 'one', {'value': 2}, expected_version=version)
        self.assertEqual(self.store.get_record('task', 'one')['data'], {'value': 1})
        self.assertEqual(len(self.store.events()), 1)

    def test_concurrent_transactions_serialize_read_modify_write(self):
        self.store.put_record('counter', 'one', {'value': 0})
        barrier = threading.Barrier(4)

        def increment():
            with Store(self.run) as store:
                barrier.wait(timeout=5)
                for _ in range(15):
                    with store.transaction():
                        record = store.get_record('counter', 'one')
                        store.put_record('counter', 'one', {'value': record['data']['value'] + 1},
                                         expected_version=record['version'])

        with ThreadPoolExecutor(max_workers=4) as executor:
            futures = [executor.submit(increment) for _ in range(4)]
            for future in futures:
                future.result(timeout=15)
        self.assertEqual(self.store.get_record('counter', 'one')['data']['value'], 60)
        self.assertEqual(len(self.store.events()), 61)

    def test_concurrent_optimistic_writes_have_one_winner(self):
        barrier = threading.Barrier(2)

        def write():
            with Store(self.run) as store:
                barrier.wait(timeout=5)
                try:
                    store.put_record('task', 'one', {}, expected_version=0)
                    return True
                except ConflictError:
                    return False

        with ThreadPoolExecutor(max_workers=2) as executor:
            futures = [executor.submit(write) for _ in range(2)]
            self.assertEqual(sorted(future.result(timeout=10) for future in futures), [False, True])
        self.assertEqual(len(self.store.events()), 1)

    def test_readonly_open_rejects_all_writes(self):
        self.store.put_record('task', 'one', {})
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.get_meta('revision'), 'abc')
            self.assertEqual(len(store.list_records('task')), 1)
            self.assertEqual(len(store.events()), 1)
            for operation in (lambda: store.set_meta('x', 1),
                              lambda: store.put_record('task', 'two', {}),
                              lambda: store.append_event('note', 'one', {})):
                with self.assertRaises(StoreError):
                    operation()
            with self.assertRaises(sqlite3.OperationalError):
                store.connection.execute('DELETE FROM records')

    def test_open_does_not_create_or_reinitialize(self):
        absent = self.run.parent / 'absent'
        with self.assertRaises(StoreError):
            Store(absent)
        self.assertFalse(absent.exists())
        for readonly in (False, True):
            with self.assertRaises(StoreError):
                Store(absent, readonly=readonly)
        with self.assertRaises(StoreError):
            Store(self.run, create=True, metadata={'revision': 'replacement'})
        self.assertEqual(self.store.get_meta('revision'), 'abc')
        legacy = self.run.parent / 'legacy'
        legacy.mkdir()
        (legacy / 'state.json').write_text('{}')
        with self.assertRaises(StoreError):
            Store(legacy, create=True)
        self.assertFalse((legacy / 'run.db').exists())

    def test_history_cannot_be_updated_or_deleted(self):
        event = self.store.append_event('note', 'one', {'text': 'original'})
        for sql in ('UPDATE events SET actor = "someone"', 'DELETE FROM events',
                    'INSERT OR REPLACE INTO events SELECT seq, kind, record_id, \'replacement\', payload, at FROM events'):
            with self.assertRaises(sqlite3.IntegrityError):
                self.store.connection.execute(sql)
        self.assertEqual(self.store.events(), [event])

    def test_json_validation_and_independent_returned_values(self):
        for data in ([], {'value': float('nan')}, {'value': object()}):
            with self.assertRaises(ValueError):
                self.store.put_record('task', 'one', data)
        for version in (-1, True, 1.5):
            with self.assertRaises(ValueError):
                self.store.put_record('task', 'one', {}, expected_version=version)
        original = {'nested': [1]}
        result = self.store.put_record('task', 'one', original)
        original['nested'].append(2)
        result['data']['nested'].append(3)
        self.assertEqual(self.store.get_record('task', 'one')['data'], {'nested': [1]})

    def test_policies_are_immutable(self):
        self.store.set_meta('policies', {'blind': True})
        with self.assertRaises(StoreError):
            self.store.set_meta('policies', {'blind': False})
        self.assertEqual(self.store.get_meta('policies'), {'blind': True})

    def test_protocol_is_immutable_after_first_write(self):
        self.store.set_meta('protocol', {'digest': 'first'})
        self.store.set_meta('protocol', {'digest': 'first'})
        with self.assertRaises(StoreError):
            self.store.set_meta('protocol', {'digest': 'changed'})
        self.assertEqual(self.store.get_meta('protocol'), {'digest': 'first'})

    def test_malformed_schema_constraints_rejected_on_open(self):
        replacements = (
            ('metadata', 'TEXT PRIMARY KEY', 'TEXT'),
            ('records', ', PRIMARY KEY(kind, id)', ''),
            ('records', 'CHECK(version > 0)', ''),
            ('records', 'data TEXT NOT NULL', 'data TEXT'),
            ('events', 'INTEGER PRIMARY KEY AUTOINCREMENT', 'INTEGER'),
        )
        for index, (table, before, after) in enumerate(replacements):
            with self.subTest(table=table, constraint=before):
                run = self.run.parent / f'malformed-{index}'
                with Store(run, create=True) as store:
                    sql = store.connection.execute(
                        'SELECT sql FROM sqlite_schema WHERE type = ? AND name = ?',
                        ('table', table)).fetchone()[0]
                    store.connection.execute(f'DROP TABLE {table}')
                    store.connection.execute(sql.replace(before, after))
                for readonly in (False, True):
                    with self.assertRaisesRegex(StoreError, 'schema'):
                        with Store(run, readonly=readonly):
                            pass

    def test_missing_or_modified_history_triggers_rejected_on_open(self):
        for index, trigger in enumerate(('events_no_update', 'events_no_delete', 'events_no_replace')):
            for modified in (False, True):
                with self.subTest(trigger=trigger, modified=modified):
                    run = self.run.parent / f'trigger-{index}-{modified}'
                    with Store(run, create=True) as store:
                        store.connection.execute(f'DROP TRIGGER {trigger}')
                        if modified:
                            store.connection.execute(
                                f'CREATE TRIGGER {trigger} BEFORE DELETE ON events BEGIN SELECT 1; END')
                    for readonly in (False, True):
                        with self.assertRaisesRegex(StoreError, 'schema'):
                            with Store(run, readonly=readonly):
                                pass

    def test_read_transaction_keeps_consistent_snapshot_during_writer_commit(self):
        for readonly in (False, True):
            with self.subTest(readonly=readonly):
                self.store.set_meta('revision', 'before')
                self.store.put_record('task', 'snapshot', {'revision': 'before'})
                with Store(self.run, readonly=readonly) as reader:
                    with reader.read_transaction() as current:
                        self.assertIs(current, reader)
                        self.assertEqual(reader.get_meta('revision'), 'before')
                        before_events = reader.events()
                        # WAL permits this independent writer to commit while the reader remains open.
                        with self.store.transaction():
                            self.store.set_meta('revision', 'after')
                            self.store.put_record('task', 'snapshot', {'revision': 'after'})
                        self.assertEqual(reader.get_meta('revision'), 'before')
                        self.assertEqual(reader.get_record('task', 'snapshot')['data']['revision'], 'before')
                        self.assertEqual(reader.events(), before_events)
                    self.assertFalse(reader.connection.in_transaction)
                    self.assertEqual(reader.get_meta('revision'), 'after')
                    self.assertEqual(reader.get_record('task', 'snapshot')['data']['revision'], 'after')

    def test_nested_read_transactions_preserve_outer_write_and_rollback(self):
        with self.assertRaisesRegex(ValueError, 'outer rollback'):
            with self.store.transaction():
                self.store.set_meta('revision', 'pending')
                with self.store.read_transaction():
                    with self.store.read_transaction():
                        self.assertEqual(self.store.get_meta('revision'), 'pending')
                self.assertTrue(self.store.connection.in_transaction)
                raise ValueError('outer rollback')
        self.assertEqual(self.store.get_meta('revision'), 'abc')
        self.assertFalse(self.store.connection.in_transaction)

    def test_read_transaction_nested_exception_does_not_end_outer_snapshot(self):
        with Store(self.run, readonly=True) as reader:
            with reader.read_transaction():
                self.assertEqual(reader.get_meta('revision'), 'abc')
                with self.assertRaisesRegex(ValueError, 'nested'):
                    with reader.read_transaction():
                        raise ValueError('nested')
                self.assertTrue(reader.connection.in_transaction)
                with self.assertRaises(StoreError):
                    reader.set_meta('revision', 'forbidden')
                self.assertTrue(reader.connection.in_transaction)
            with self.assertRaisesRegex(ValueError, 'own rollback'):
                with reader.read_transaction():
                    raise ValueError('own rollback')
            self.assertFalse(reader.connection.in_transaction)
            with reader.read_transaction():
                self.assertEqual(reader.get_meta('revision'), 'abc')

    def test_read_transaction_preserves_externally_started_transaction(self):
        self.store.connection.execute('BEGIN')
        try:
            with self.store.read_transaction():
                self.assertEqual(self.store.get_meta('revision'), 'abc')
            self.assertTrue(self.store.connection.in_transaction)
        finally:
            self.store.connection.execute('ROLLBACK')
        self.store.close()
        with self.assertRaises(StoreError):
            with self.store.read_transaction():
                pass

    def test_unknown_schema_rejected(self):
        self.store.connection.execute('PRAGMA user_version = 99')
        with self.assertRaisesRegex(StoreError, 'schema'):
            Store(self.run)


if __name__ == '__main__':
    unittest.main()
