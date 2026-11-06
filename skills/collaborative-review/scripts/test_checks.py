"""Integration tests for pinned read-only inspections."""

import copy
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from review_core.authorization import record_authorization
from review_core.checks import inspect_snapshot, verify_checks
from review_core.snapshots import create_snapshot
from review_core.store import Store


class InspectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project = self.root / 'project'
        self.project.mkdir()
        (self.project / 'a.txt').write_text('first\nneedle.* literal\nlast\n')
        (self.project / 'b.txt').write_bytes(b'other\x00data')
        self.store = Store(self.root / 'runs' / 'one', create=True)
        self.manifest = create_snapshot(self.project, ['a.txt', 'b.txt'], self.root / 'runs', 'one')
        for key, value in {'snapshot': self.manifest, 'revision': self.manifest['revision'],
                           'project_root': str(self.project), 'paths': ['a.txt', 'b.txt']}.items():
            self.store.set_meta(key, value)
        self.addCleanup(self.store.close)
        self.addCleanup(self.make_writable)

    def make_writable(self):
        for directory, directories, filenames in os.walk(self.root):
            Path(directory).chmod(0o755)
            for filename in filenames:
                path = Path(directory) / filename
                if not path.is_symlink():
                    path.chmod(0o644)

    def grant(self, paths=None, revision=None):
        return record_authorization(self.store, revision or self.manifest['revision'], ['inspect'], paths or ['.'])

    def test_reads_pinned_content_and_literal_search_without_mutation(self):
        grant = self.grant(['a.txt', 'b.txt'])
        (self.project / 'a.txt').write_text('working tree changed')
        before = self.store.events()
        result = inspect_snapshot(self.store, ['a.txt'], 'needle.*')
        self.assertEqual(result['authorization_id'], grant['id'])
        self.assertEqual(result['files'][0]['lines'], [{'number': 2, 'text': 'needle.* literal', 'truncated': False}])
        self.assertFalse(result['truncated'])
        self.assertEqual(before, self.store.events())
        self.assertEqual((self.project / 'a.txt').read_text(), 'working tree changed')

    def test_denies_missing_wrong_revision_and_partial_grants(self):
        with self.assertRaises(PermissionError):
            inspect_snapshot(self.store)
        self.grant(revision='old')
        with self.assertRaises(PermissionError):
            inspect_snapshot(self.store)
        self.grant(['a.txt'])
        with self.assertRaises(PermissionError):
            inspect_snapshot(self.store, ['b.txt'])
        with self.assertRaises(PermissionError):
            verify_checks(self.store)

    def test_partial_grant_denied_before_any_snapshot_reads(self):
        self.grant(['a.txt'])
        with patch('review_core.checks._verify', return_value=[]) as verify:
            with self.assertRaises(PermissionError):
                inspect_snapshot(self.store, ['a.txt'])
            verify.assert_not_called()

    def test_rejects_traversal_and_outside_scope(self):
        self.grant()
        for paths in (['../a.txt'], ['/a.txt'], ['a.txt/../b.txt'], ['.'], ['missing']):
            with self.subTest(paths=paths), self.assertRaises(ValueError):
                inspect_snapshot(self.store, paths)

    def test_verifier_uses_every_trusted_anchor(self):
        self.grant()
        for field, value in (('revision', '0' * 64), ('project_root', str(self.root)), ('paths', ['a.txt']), ('root', str(self.root))):
            manifest = copy.deepcopy(self.manifest)
            manifest[field] = value
            self.store.set_meta('snapshot', manifest)
            self.assertFalse(verify_checks(self.store)['valid'])
            with self.assertRaises(ValueError):
                inspect_snapshot(self.store)
        self.store.set_meta('snapshot', self.manifest)
        self.assertTrue(verify_checks(self.store)['valid'])

    def test_detects_content_and_permission_tampering(self):
        self.grant()
        path = Path(self.manifest['root']) / 'a.txt'
        path.chmod(0o644)
        self.assertFalse(verify_checks(self.store)['valid'])
        path.write_text('tampered')
        path.chmod(0o444)
        with self.assertRaises(ValueError):
            inspect_snapshot(self.store)

    def test_rejects_symlink_and_fifo_without_blocking(self):
        self.grant()
        root = Path(self.manifest['root'])
        root.chmod(0o755)
        path = root / 'a.txt'
        path.unlink()
        path.symlink_to(self.project / 'a.txt')
        with self.assertRaises(ValueError):
            inspect_snapshot(self.store)
        path.unlink()
        os.mkfifo(path)
        with self.assertRaises(ValueError):
            inspect_snapshot(self.store)

    def test_explicit_line_and_total_truncation(self):
        self.grant()
        with patch('review_core.checks.MAX_LINE_BYTES', 3):
            result = inspect_snapshot(self.store, ['a.txt'])
        self.assertTrue(result['truncated'])
        self.assertEqual(result['files'][0]['lines'][0], {'number': 1, 'text': 'fir', 'truncated': True})
        with patch('review_core.checks.MAX_OUTPUT_LINES', 1):
            result = inspect_snapshot(self.store)
        self.assertTrue(result['truncated'])
        self.assertEqual(len(result['files'][0]['lines']), 1)

    def test_file_limit_and_binary_are_explicit(self):
        self.grant()
        with patch('review_core.checks.MAX_FILE_BYTES', 4):
            result = inspect_snapshot(self.store, ['a.txt'], 'needle')
        self.assertTrue(result['files'][0]['truncated'])
        self.assertEqual(result['files'][0]['lines'], [])
        with patch('review_core.checks.MAX_FILES', 1):
            result = inspect_snapshot(self.store)
        self.assertEqual(len(result['files']), 1)
        self.assertTrue(result['truncated'])
        result = inspect_snapshot(self.store, ['b.txt'])
        self.assertTrue(result['files'][0]['binary'])
        self.assertEqual(result['files'][0]['lines'], [])

    def test_invalid_patterns_rejected(self):
        self.grant()
        for pattern in ('', '\n', 4, 'x' * 4097):
            with self.subTest(pattern=str(pattern)[:20]), self.assertRaises(ValueError):
                inspect_snapshot(self.store, pattern=pattern)


if __name__ == '__main__':
    unittest.main()
