from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch

from review_core.doctor import diagnose


class DoctorTests(unittest.TestCase):
    def test_healthy_real_subprocess_locking_and_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = diagnose(root, [sys.executable, '--version'])
            self.assertTrue(result['ok'], result)
            self.assertTrue(next(row for row in result['checks'] if row['name'] == 'sqlite_locking')['ok'])
            self.assertEqual(list(root.iterdir()), [])

    def test_missing_directory_does_not_create_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'missing'
            self.assertFalse(diagnose(root)['ok'])
            self.assertFalse(root.exists())

    def test_missing_command(self):
        with tempfile.TemporaryDirectory() as temporary, patch('review_core.doctor.shutil.which', return_value=None):
            checks = diagnose(temporary, ['absent-worker'])['checks']
            self.assertEqual([row['name'] for row in checks if not row['ok']], ['worker_command'])

    def test_storage_failure_is_structured(self):
        with tempfile.TemporaryDirectory() as temporary, patch('review_core.doctor.sqlite3.connect', side_effect=sqlite3.OperationalError('disk unavailable')):
            self.assertFalse(diagnose(temporary)['ok'])
            self.assertEqual(list(Path(temporary).iterdir()), [])

    def test_broken_locking_is_not_reported_healthy(self):
        with tempfile.TemporaryDirectory() as temporary, patch('review_core.doctor._probe_lock', return_value=0):
            result = diagnose(temporary)
            self.assertFalse(next(row for row in result['checks'] if row['name'] == 'sqlite_locking')['ok'])
            self.assertFalse(result['ok'])

    def test_timeout_is_bounded_and_cleans_up(self):
        import subprocess
        with tempfile.TemporaryDirectory() as temporary, patch('review_core.doctor._probe_lock', side_effect=subprocess.TimeoutExpired('probe', 5)):
            self.assertFalse(diagnose(temporary)['ok'])
            self.assertEqual(list(Path(temporary).iterdir()), [])

    def test_known_network_mount_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary, patch('review_core.doctor._filesystem_type', return_value='nfs4'):
            with patch('review_core.doctor.tempfile.TemporaryDirectory') as temporary_probe, patch('review_core.doctor.sqlite3.connect') as connect, patch.object(Path, 'write_bytes') as write:
                result = diagnose(temporary)
            self.assertFalse(result['ok'])
            temporary_probe.assert_not_called()
            connect.assert_not_called()
            write.assert_not_called()
            self.assertIn('skipped', next(row for row in result['checks'] if row['name'] == 'storage_probe')['detail'])

    def test_worker_is_checked_without_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            marker = Path(temporary) / 'executed'
            result = diagnose(temporary, [sys.executable, '-c', f'open({str(marker)!r}, "w").close()'])
            self.assertTrue(result['ok'], result)
            self.assertFalse(marker.exists())


if __name__ == '__main__':
    unittest.main()
