import json
import os
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from unittest.mock import patch
from pathlib import Path
import tempfile
import unittest

from review_core.protocol import freeze_protocol, verify_protocol, propose_change


class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        (self.source / 'review.py').write_text('from review_core import main\nmain()\n')
        package = self.source / 'review_core'
        package.mkdir()
        (package / '__init__.py').write_text('def main():\n    print("review")\n')
        (package / 'test_ignored.py').write_text('ignored')
        (package / '__pycache__').mkdir()
        (package / '__pycache__' / 'cached.py').write_text('ignored')
        (self.source / 'unrelated.py').write_text('ignored')
        self.run = self.root / 'run'

    def test_copy_pin_and_idempotency(self):
        policies = {'challenge_required': True}
        manifest = freeze_protocol(self.run, self.source, policies)
        policies['challenge_required'] = False
        self.assertTrue(manifest['policies']['challenge_required'])
        self.assertEqual([], verify_protocol(self.run))
        self.assertEqual({'review.py', 'review_core/__init__.py'}, {item['path'] for item in manifest['files']})
        self.assertEqual(manifest, freeze_protocol(self.run, self.source, {'challenge_required': True}))
        self.assertEqual((self.source / 'review.py').read_bytes(), (self.run / 'protocol/review.py').read_bytes())
        with self.assertRaises(ValueError):
            freeze_protocol(self.run, self.source, {'challenge_required': False})
        (self.source / 'review.py').write_text('changed')
        with self.assertRaises(ValueError):
            freeze_protocol(self.run, self.source, {'challenge_required': True})
        self.assertEqual([], verify_protocol(self.run))

    def test_detect_content_and_extra_file_tampering(self):
        freeze_protocol(self.run, self.source, {})
        path = self.run / 'protocol/review.py'
        path.chmod(0o644)
        path.write_text('tampered')
        self.assertTrue(verify_protocol(self.run))
        path.write_bytes((self.source / 'review.py').read_bytes())
        (self.run / 'protocol/extra.py').write_text('extra')
        self.assertTrue(verify_protocol(self.run))

    def test_detect_policy_tampering(self):
        freeze_protocol(self.run, self.source, {'locked': True})
        path = self.run / 'protocol/manifest.json'
        manifest = json.loads(path.read_text())
        manifest['policies']['locked'] = False
        path.chmod(0o644)
        path.write_text(json.dumps(manifest))
        self.assertTrue(verify_protocol(self.run))

    def test_reject_symlink_source_and_run(self):
        (self.source / 'review_core/link.py').symlink_to(self.source / 'review.py')
        with self.assertRaises(ValueError):
            freeze_protocol(self.run, self.source, {})
        linked = self.root / 'linked'
        linked.symlink_to(self.source, target_is_directory=True)
        with self.assertRaises(ValueError):
            freeze_protocol(linked / 'run', self.source, {})

    def test_fifo_is_rejected_without_blocking(self):
        freeze_protocol(self.run, self.source, {})
        for name in ('review.py', 'manifest.json'):
            path = self.run / 'protocol' / name
            original = path.read_bytes()
            path.unlink()
            os.mkfifo(path)
            result = subprocess.run(
                [sys.executable, '-c',
                 'import sys; from review_core.protocol import verify_protocol; '
                 'sys.exit(0 if verify_protocol(sys.argv[1]) else 1)', str(self.run)],
                cwd=Path(__file__).parent, timeout=2, capture_output=True)
            self.assertEqual(0, result.returncode, result.stderr)
            path.unlink()
            path.write_bytes(original)

    def test_concurrent_identical_freezes(self):
        barrier = threading.Barrier(2)
        original = Path.rename

        def rename(path, destination):
            if path.name.startswith('.protocol-'):
                barrier.wait(timeout=5)
            return original(path, destination)

        with patch.object(Path, 'rename', rename), ThreadPoolExecutor(2) as pool:
            futures = [pool.submit(freeze_protocol, self.run, self.source, {}) for _ in range(2)]
            results = [future.result(timeout=10) for future in futures]
        self.assertEqual(results[0], results[1])
        self.assertEqual([], verify_protocol(self.run))

    def test_concurrent_different_policies_reject_loser(self):
        barrier = threading.Barrier(2)
        original = Path.rename

        def rename(path, destination):
            if path.name.startswith('.protocol-'):
                barrier.wait(timeout=5)
            return original(path, destination)

        with patch.object(Path, 'rename', rename), ThreadPoolExecutor(2) as pool:
            futures = [pool.submit(freeze_protocol, self.run, self.source, {'choice': choice})
                       for choice in (True, False)]
            results = []
            errors = []
            for future in futures:
                try:
                    results.append(future.result(timeout=10))
                except ValueError as error:
                    errors.append(str(error))
        self.assertEqual(1, len(results))
        self.assertEqual(1, len(errors))
        self.assertIn('create a new run', errors[0])
        self.assertEqual([], verify_protocol(self.run))

    def test_execute_frozen_bundle_without_bytecode(self):
        freeze_protocol(self.run, self.source, {})
        result = subprocess.run([sys.executable, '-B', str(self.run / 'protocol/review.py')],
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertEqual('review\n', result.stdout)
        self.assertEqual([], verify_protocol(self.run))

    def test_proposal_preserves_protocol(self):
        from review_core.store import Store
        with Store(self.run, create=True) as store:
            freeze_protocol(self.run, self.source, {'locked': True})
            store.set_meta('policies', {'locked': True})
            result = propose_change(store, 'Change next run', evidence=['review observation'])
            self.assertEqual('protocol_proposal', result['kind'])
            self.assertEqual({'locked': True}, store.get_meta('policies'))
            self.assertEqual(1, len(store.list_records('protocol_proposal')))
        self.assertEqual([], verify_protocol(self.run))


if __name__ == '__main__':
    unittest.main()
