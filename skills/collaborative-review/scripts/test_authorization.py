import tempfile
import unittest
from pathlib import Path

from review_core.authorization import authorize_task, canonical_paths, record_authorization, revoke_authorization
from review_core.store import Store


class AuthorizationTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.store = Store(Path(self.directory.name) / 'run', create=True)
        self.addCleanup(self.store.close)

    def grant(self, **kwargs):
        return record_authorization(self.store, 'revision', ['review'], ['src'], **kwargs)

    def test_exact_revision_action_and_descendant_scope(self):
        grant = self.grant()
        self.assertEqual(authorize_task(self.store, 'review', 'revision', ['src/a.py'], now=10), grant['id'])
        for action, revision, paths in [('execute', 'revision', ['src']), ('review', 'other', ['src']),
                                        ('review', 'revision', ['src-other']), ('review', 'revision', ['.']),
                                        ('review', 'revision', ['src/a', 'other'])]:
            with self.assertRaises(PermissionError):
                authorize_task(self.store, action, revision, paths, now=10)

    def test_no_grant_denied_expired_and_revoked(self):
        with self.assertRaises(PermissionError):
            authorize_task(self.store, 'review', 'revision', ['src'], now=10)
        self.grant(decision='denied')
        expiring = self.grant(expires_at=10)
        self.assertEqual(authorize_task(self.store, 'review', 'revision', ['src'], now=9), expiring['id'])
        with self.assertRaises(PermissionError):
            authorize_task(self.store, 'review', 'revision', ['src'], now=10)
        grant = self.grant()
        before = self.store.get_record('authorization', grant['id'])
        revocation = revoke_authorization(self.store, grant['id'], reason='withdrawn')
        event_count = len(self.store.events())
        self.assertEqual(revoke_authorization(self.store, grant['id']), revocation)
        self.assertEqual(len(self.store.events()), event_count)
        self.assertEqual(self.store.get_record('authorization', grant['id']), before)
        with self.assertRaises(PermissionError):
            authorize_task(self.store, 'review', 'revision', ['src'], now=10)
        with self.assertRaises(ValueError):
            revoke_authorization(self.store, 'unknown')

    def test_canonical_scopes_root_and_rejections(self):
        self.assertEqual(canonical_paths(['src/b', 'src/a', 'src/a']), ['src/a', 'src/b'])
        grant = record_authorization(self.store, 'revision', ['review'], ['.'])
        self.assertEqual(authorize_task(self.store, 'review', 'revision', ['any/file'], now=0), grant['id'])
        for path in ['../src', 'src/../other', '/src', 'src//a', 'src/./a', 'src/', 'C:/src', 'src\\a', 'src\x00a', ' src']:
            with self.subTest(path=path), self.assertRaises(ValueError):
                record_authorization(self.store, 'revision', ['review'], [path])
            with self.subTest(path=path), self.assertRaises(ValueError):
                authorize_task(self.store, 'review', 'revision', [path], now=0)

    def test_clock_and_input_validation(self):
        for value in [True, '10', float('nan'), float('inf'), -1, 10 ** 1000]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.grant(expires_at=value)
            with self.subTest(value=value), self.assertRaises(ValueError):
                authorize_task(self.store, 'review', 'revision', ['src'], now=value)
        for actions, paths in [([], ['src']), (['*'], ['src']), (['review'], []), ('review', ['src'])]:
            with self.assertRaises(ValueError):
                record_authorization(self.store, 'revision', actions, paths)
        with self.assertRaises(ValueError):
            self.grant(decision='maybe')
        self.assertEqual(self.store.list_records('authorization'), [])


if __name__ == '__main__':
    unittest.main()
