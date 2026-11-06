"""Evidence and revision regressions for assumptions."""
import tempfile
import unittest
from pathlib import Path

from review_core.store import Store
from review_core.assumptions import add_assumption, verify_assumption, invalidate_assumptions, assumption_gaps


class AssumptionTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.store = Store(Path(temp.name) / 'run', create=True, metadata={'revision': 'a', 'snapshot': {'files': [{'path': 'src/a.py'}, {'path': 'lib/b.py'}]}})
        self.addCleanup(self.store.close)

    def add(self, **kwargs):
        values = dict(author='reviewer', revision='a', statement='API preserves ordering', affected_paths=['src'], assumption_id='assumption-order')
        values.update(kwargs)
        return add_assumption(self.store, **values)

    def verify(self, **kwargs):
        values = dict(assumption_id='assumption-order', author='checker', revision='a', evidence=['tests/order.py:10 passed'])
        values.update(kwargs)
        return verify_assumption(self.store, **values)

    def test_explicit_evidence_and_idempotence(self):
        row = self.add(evidence=['source.py:5'])
        self.assertEqual(row, self.add(evidence=['source.py:5']))
        self.assertEqual(1, len(assumption_gaps(self.store)))
        for evidence in ([], 'reference', [True], [' ']):
            with self.subTest(evidence=evidence), self.assertRaises(ValueError):
                self.verify(evidence=evidence)
        verified = self.verify()
        self.assertEqual(verified, self.verify())
        self.assertEqual(verified, self.add(evidence=['source.py:5']))
        with self.assertRaises(ValueError):
            self.add(evidence=['different proof'])
        self.assertEqual([], assumption_gaps(self.store))
        self.assertEqual(2, len(self.store.events()))
        with self.assertRaises(ValueError):
            self.add(statement='Different')

    def test_validation(self):
        for paths in ([], ['missing'], ['../src'], ['/src'], ['src//a.py'], ['src/'], ['src/./a.py'], ['src\\a.py']):
            with self.subTest(paths=paths), self.assertRaises(ValueError):
                self.add(affected_paths=paths)
        for values in ({'blocks_approval': 1}, {'revision': 'old'}, {'statement': ''}, {'author': ''}, {'assumption_id': '../bad'}):
            with self.subTest(values=values), self.assertRaises(ValueError):
                self.add(**values)
        self.assertEqual([], self.store.list_records('assumption'))

    def test_deleted_path_invalidates_parent_and_carries_unaffected(self):
        self.add()
        self.verify()
        self.add(assumption_id='lib', affected_paths=['lib/b.py'])
        self.verify(assumption_id='lib')
        self.store.set_meta('revision', 'b')
        self.store.set_meta('snapshot', {'files': [{'path': 'lib/b.py'}]})
        invalidate_assumptions(self.store, ['src/a.py'], 'b')
        changed = self.store.get_record('assumption', 'assumption-order')['data']
        carried = self.store.get_record('assumption', 'lib')['data']
        self.assertEqual('invalidated', changed['status'])
        self.assertEqual([], changed['evidence'])
        self.assertEqual('verified', carried['status'])
        self.assertEqual('a', carried['history'][0]['source_revision'])
        self.assertTrue(carried['history'][0]['evidence'])
        self.assertEqual('b', carried['revision'])
        self.assertEqual(['assumption-order'], [gap['id'] for gap in assumption_gaps(self.store)])
        self.assertEqual([], invalidate_assumptions(self.store, ['src/a.py'], 'b'))
        self.verify(revision='b')
        self.assertEqual([], assumption_gaps(self.store))

    def test_changed_directory_invalidates_descendant_and_stale_blocks(self):
        self.add(affected_paths=['src/a.py'])
        self.verify()
        self.store.set_meta('revision', 'b')
        self.assertEqual(1, len(assumption_gaps(self.store)))
        with self.assertRaises(ValueError):
            self.verify()
        with self.assertRaises(ValueError):
            invalidate_assumptions(self.store, ['../src'], 'b')
        self.assertEqual('a', self.store.get_record('assumption', 'assumption-order')['data']['revision'])
        invalidate_assumptions(self.store, ['src'], 'b')
        self.assertEqual('invalidated', self.store.get_record('assumption', 'assumption-order')['data']['status'])

    def test_nonblocking_does_not_gate(self):
        self.add(blocks_approval=False)
        self.assertEqual([], assumption_gaps(self.store))


if __name__ == '__main__':
    unittest.main()
