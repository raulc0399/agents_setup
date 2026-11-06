import copy
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from review_core.snapshots import changed_paths, create_snapshot, verify_snapshot


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.project = Path(self.temporary.name) / 'project'
        self.project.mkdir()
        self.output = self.project / '.reviews'
        (self.project / 'src').mkdir()
        self.source = self.project / 'src' / 'tool.py'
        self.source.write_text('print(1)\n')
        self.source.chmod(0o755)

    def capture(self, paths=None):
        return create_snapshot(self.project, paths or ['.'], self.output, 'run')

    def test_capture_preserves_layout_permissions_and_source(self):
        manifest = self.capture()
        self.assertEqual(verify_snapshot(manifest), [])
        target = Path(manifest['root']) / 'src/tool.py'
        self.assertEqual(target.read_bytes(), self.source.read_bytes())
        self.assertEqual(target.stat().st_mode & 0o777, 0o555)
        self.assertEqual(manifest['files'][0]['mode'], 0o755)
        self.assertEqual(self.source.stat().st_mode & 0o777, 0o755)
        self.source.write_text('print(2)\n')
        self.assertEqual(target.read_text(), 'print(1)\n')

    def test_deterministic_overlapping_selections_and_nested_output(self):
        first = self.capture(['src/tool.py', 'src', 'src'])
        second = self.capture()
        self.assertEqual(first['revision'], second['revision'])
        self.assertEqual(first['root'], second['root'])
        self.assertEqual(len(first['files']), 1)
        self.assertEqual(list((self.output / 'run' / 'snapshots').iterdir()), [Path(first['root'])])

    def test_excludes_generated_directories_but_includes_explicit_file(self):
        for name in ('.git', '__pycache__', '.reviews'):
            folder = self.project / name
            folder.mkdir(exist_ok=True)
            (folder / 'state').write_text('generated')
        manifest = self.capture()
        self.assertEqual([entry['path'] for entry in manifest['files']], ['src/tool.py'])
        explicit = self.capture(['.git/state', '.reviews/state'])
        self.assertEqual([entry['path'] for entry in explicit['files']], ['.git/state', '.reviews/state'])

    def test_rejects_missing_empty_traversal_absolute_and_symlinks(self):
        (self.project / 'empty').mkdir()
        (self.project / 'link').symlink_to(self.source)
        for paths in ([], ['missing'], ['empty'], ['../outside'], [str(self.source)], ['link'], ['src/../src/tool.py']):
            with self.subTest(paths=paths), self.assertRaises(ValueError):
                create_snapshot(self.project, paths, self.output, 'run')
        with self.assertRaises(ValueError):
            self.capture()
        with self.assertRaises(ValueError):
            create_snapshot(self.project, ['src'], self.output, '../escape')

    def test_changed_paths_include_mode_content_added_and_removed(self):
        first = self.capture()
        self.source.chmod(0o644)
        second = self.capture()
        self.assertEqual(changed_paths(first, second), ['src/tool.py'])
        (self.project / 'new').write_text('new')
        self.source.unlink()
        third = self.capture()
        self.assertEqual(changed_paths(second, third), ['new', 'src/tool.py'])

    def test_tampering_and_hostile_manifest_are_detected(self):
        manifest = self.capture()
        target = Path(manifest['root']) / 'src/tool.py'
        target.chmod(0o755)
        target.write_text('tampered')
        self.assertTrue(verify_snapshot(manifest))
        with self.assertRaises(ValueError):
            self.capture()
        for path in ('../outside', '/etc/passwd', 'src/../../outside', '.'):
            hostile = copy.deepcopy(manifest)
            hostile['files'][0]['path'] = path
            self.assertTrue(verify_snapshot(hostile))
        hostile = copy.deepcopy(manifest)
        hostile['root'] = '/etc'
        self.assertTrue(verify_snapshot(hostile))

    def test_output_symlinks_and_generated_explicit_inputs_rejected(self):
        other = Path(self.temporary.name) / 'other'
        other.mkdir()
        self.output.symlink_to(other, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.capture(['src'])

    def test_manifest_project_and_scope_metadata_are_required_and_validated(self):
        manifest = self.capture()
        variants = []
        for key in ('project_root', 'paths'):
            missing = copy.deepcopy(manifest)
            del missing[key]
            variants.append(missing)
        for project in ('relative', str(self.project / '..'), None):
            invalid = copy.deepcopy(manifest)
            invalid['project_root'] = project
            variants.append(invalid)
        for paths in ([], 'src', ['../src'], ['unrelated'], ['src', 'src']):
            invalid = copy.deepcopy(manifest)
            invalid['paths'] = paths
            variants.append(invalid)
        for invalid in variants:
            with self.subTest(manifest=invalid):
                self.assertTrue(verify_snapshot(invalid))

    def test_trusted_anchors_reject_relocated_root_and_changed_metadata(self):
        manifest = self.capture()
        anchors = {'expected_run_dir': self.output / 'run',
                   'expected_project_root': self.project, 'expected_paths': ['.']}
        self.assertEqual(verify_snapshot(manifest, **anchors), [])
        other = create_snapshot(self.project, ['.'], Path(self.temporary.name) / 'elsewhere', 'run')
        self.assertTrue(verify_snapshot(other, **anchors))
        changed = copy.deepcopy(manifest)
        changed['project_root'] = str(Path(self.temporary.name))
        self.assertTrue(verify_snapshot(changed, **anchors))
        changed = copy.deepcopy(manifest)
        changed['paths'] = ['src']
        self.assertTrue(verify_snapshot(changed, **anchors))
        self.assertTrue(verify_snapshot(manifest, expected_revision='0' * 64))

    def test_verification_reads_large_files_in_bounded_chunks(self):
        self.source.write_bytes(b'x' * (3 * 1024 * 1024))
        manifest = self.capture()
        original_fdopen = os.fdopen
        reads = []

        class BoundedReader:
            def __init__(self, handle):
                self.handle = handle

            def __enter__(self):
                self.handle.__enter__()
                return self

            def __exit__(self, *args):
                return self.handle.__exit__(*args)

            def fileno(self):
                return self.handle.fileno()

            def read(self, size=-1):
                if not 0 < size <= 1024 * 1024:
                    raise AssertionError('verification attempted an unbounded read')
                reads.append(size)
                return self.handle.read(size)

        def bounded_fdopen(*args, **kwargs):
            return BoundedReader(original_fdopen(*args, **kwargs))

        with mock.patch('review_core.snapshots.os.fdopen', side_effect=bounded_fdopen):
            self.assertEqual(verify_snapshot(manifest), [])
        self.assertGreater(len(reads), 3)

    def test_unmanifested_file_and_symlink_detected(self):
        manifest = self.capture()
        root = Path(manifest['root'])
        root.chmod(0o755)
        (root / 'extra').write_text('extra')
        (root / 'link').symlink_to('/etc/passwd')
        self.assertTrue(verify_snapshot(manifest))


if __name__ == '__main__':
    unittest.main()
