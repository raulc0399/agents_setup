"""Real local subprocesses exercise the worker transport and cleanup."""
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from review_core.aasm import BASE_DIALECT
from review_core.authorization import record_authorization
from review_core.dialects import acknowledge_dialect, propose_dialect, test_dialect
from review_core.coordinator import Coordinator
from review_core.runner import MAX_OUTPUT_BYTES, run_next_task
from review_core.snapshots import create_snapshot
from review_core.store import Store


RESULT_BODY = 'COV id=covered paths=["code.py"] status=reviewed evidence=["code.py:1"]\nVOTE verdict=APPROVED evidence=["covered"]'
RESULT_WORKER = 'print(' + repr(RESULT_BODY) + ')'


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        project = self.root / 'project'
        project.mkdir()
        (project / 'code.py').write_text('original')
        self.store = Store(self.root / 'runs' / 'run', create=True)
        manifest = create_snapshot(project, ['code.py'], self.root / 'runs', 'run')
        metadata = {
            'snapshot': manifest, 'project_root': str(project), 'paths': ['code.py'],
            'revision': manifest['revision'], 'status': 'ready', 'budget_seconds': 100,
            'spent_seconds': 0, 'max_workers': 2}
        for key, value in metadata.items():
            self.store.set_meta(key, value)
        self.coordinator = Coordinator(self.store)
        record_authorization(self.store, manifest['revision'], ['review'], ['code.py'])
        self.task = self.coordinator.enqueue_task('review', {
            'revision': manifest['revision'], 'scope_paths': ['code.py'], 'brief': 'Review code',
            'reviewer': 'reviewer-1', 'timeout_seconds': 10, 'peer_results': 'SECRET'})

    def tearDown(self):
        self.store.close()
        # Snapshot directories deliberately have no write bits.
        for directory, _, _ in os.walk(self.root):
            Path(directory).chmod(0o755)
        self.temp.cleanup()

    def run_worker(self, code, timeout=2):
        return run_next_task(self.coordinator, 'reviewer-1', [sys.executable, '-c', code], timeout)

    def test_aasm_blind_input_environment_and_writable_isolation(self):
        result = self.run_worker('''import json, os, pathlib, sys
request=json.load(sys.stdin)
assert 'SECRET' not in json.dumps(request)
assert set(request)=={'task','snapshot','language'}
assert request['language']['id']=='aasm-1'
assert request['language']['digest']
assert request['language']['definitions']==[]
assert 'VOTE' in request['language']['builtin_schemas']
assert request['language']['instructions']
assert os.environ['TASK_ID']==request['task']['id']
assert os.environ['LEASE_ID']
assert pathlib.Path('code.py').read_text()=='original'
pathlib.Path('code.py').write_text('worker edit')
print('COV id=covered paths=["code.py"] status=reviewed evidence=["code.py:1"]')
print('VOTE verdict=APPROVED evidence=["covered"]')
''')
        self.assertEqual(result['data']['status'], 'completed')
        self.assertTrue(result['data']['execution']['terminated'])
        self.assertTrue(Path(result['data']['execution']['workspace']).exists())
        self.assertEqual(result['data']['result']['dialect_id'], BASE_DIALECT)
        self.assertEqual(result['data']['result']['coverage'], ['covered'])
        manifest = self.store.get_meta('snapshot')
        self.assertEqual((Path(manifest['root']) / 'code.py').read_text(), 'original')
        self.assertIsNone(self.run_worker('raise Exception("duplicate")'))

    def test_targeted_dispatch_does_not_claim_earlier_task(self):
        selected = self.coordinator.enqueue_task('review', dict(revision=self.store.get_meta('revision'),
            scope_paths=['code.py'], brief='Selected review', reviewer='reviewer-1', timeout_seconds=10))
        result = run_next_task(self.coordinator, 'reviewer-1', [sys.executable, '-c', RESULT_WORKER],
                               task_id=selected['id'])
        self.assertEqual(result['id'], selected['id'])
        self.assertEqual(self.store.get_record('task', self.task['id'])['data']['status'], 'pending')

    def test_identity_mismatch_does_not_dispatch(self):
        self.assertIsNone(run_next_task(self.coordinator, 'reviewer-2', [sys.executable, '-c', 'raise Exception()']))
        self.assertEqual(self.store.get_record('task', self.task['id'])['data']['status'], 'pending')

    def test_invalid_result_and_exit_fail(self):
        result = self.run_worker('print("[]")')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('line 1', result['data']['error'])

    def test_json_worker_output_is_rejected(self):
        result = self.run_worker('print(' + repr(json.dumps({'evidence_ids': [], 'coverage': []})) + ')')
        self.assertEqual(result['data']['status'], 'failed')

    def test_invalid_utf8_output_is_rejected(self):
        result = self.run_worker('import os; os.write(1, bytes([255]))')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('UnicodeDecodeError', result['data']['error'])

    def test_unknown_dialect_rejected_before_claim(self):
        events = len(self.store.events())
        with self.assertRaises(ValueError):
            run_next_task(self.coordinator, 'reviewer-1', [sys.executable, '-c', RESULT_WORKER], dialect_id='unknown')
        self.assertEqual(len(self.store.events()), events)
        self.assertEqual(self.store.get_record('task', self.task['id'])['data']['status'], 'pending')

    def test_negotiated_dialect_is_pinned_in_worker_input(self):
        definitions = [{'op': 'OBS', 'meaning': 'Record an observation.',
                        'fields': {'text': {'type': 'text', 'required': True}},
                        'expands_to': {'op': 'NOTE', 'args': {'text': {'field': 'text'}}}}]
        examples = [{'body': 'OBS text=observed',
                     'expected': [{'op': 'NOTE', 'args': {'text': 'observed', 'refs': []}}]}]
        dialect = propose_dialect(self.store, 'reviewer-1', 'review', 1,
                                  ['reviewer-1', 'reviewer-2'], definitions, examples)['data']
        with self.assertRaises(ValueError):
            run_next_task(self.coordinator, 'reviewer-1', [sys.executable, '-c', RESULT_WORKER], dialect_id=dialect['id'])
        self.assertEqual(self.store.get_record('task', self.task['id'])['data']['status'], 'pending')
        for participant in dialect['participants']:
            test_dialect(self.store, dialect['id'], participant, examples)
            acknowledge_dialect(self.store, dialect['id'], participant)
        code = ('import json,sys; request=json.load(sys.stdin); '
                'assert request["language"]["id"]==' + repr(dialect['id']) + '; '
                'assert request["language"]["digest"]==' + repr(dialect['digest']) + '; '
                'assert request["language"]["definitions"][0]["op"]=="OBS"; '
                'print("OBS text=observed"); ' + RESULT_WORKER)
        result = run_next_task(self.coordinator, 'reviewer-1', [sys.executable, '-c', code], dialect_id=dialect['id'])
        self.assertEqual(result['data']['status'], 'completed')
        self.assertEqual(result['data']['result']['dialect_digest'], dialect['digest'])
        self.assertEqual(result['data']['result']['operations'][0]['op'], 'NOTE')

    def test_output_limit_includes_stderr(self):
        result = self.run_worker(f'import os; os.write(2, b"x" * {MAX_OUTPUT_BYTES + 1})')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('byte limit', result['data']['error'])

    def test_timeout_kills_descendant_holding_pipe(self):
        marker = self.root / 'descendant-alive'
        code = f'''import subprocess, sys
subprocess.Popen([sys.executable, '-c', {('import time; from pathlib import Path; time.sleep(.5); Path(' + repr(str(marker)) + ').write_text("alive")')!r}])
'''
        result = self.run_worker(code, timeout=.15)
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('timeout', result['data']['error'])
        time.sleep(.6)
        self.assertFalse(marker.exists())

    def test_cancellation_acknowledged_after_process_termination(self):
        def cancel():
            time.sleep(.15)
            with Store(self.store.run_dir) as separate:
                Coordinator(separate).cancel_task(self.task['id'], 'Stop review')
        thread = threading.Thread(target=cancel)
        thread.start()
        try:
            result = self.run_worker('import time; time.sleep(5)')
        finally:
            thread.join()
        self.assertEqual(result['data']['status'], 'cancelled')
        self.assertTrue(result['data']['execution']['terminated'])
        self.assertEqual(result['data']['reserved_seconds'], 0)

    def test_manifest_trusted_revision_anchor(self):
        self.store.set_meta('project_root', str(self.root / 'other'))
        result = self.run_worker('raise Exception("must not start")')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('trusted project', result['data']['error'])
        self.assertTrue(result['data']['execution']['terminated'])

    def test_lease_renewal_during_real_process(self):
        with patch('review_core.runner.LEASE_SECONDS', .2):
            result = self.run_worker('import time; time.sleep(.45); ' + RESULT_WORKER)
        self.assertEqual(result['data']['status'], 'completed')
        self.assertTrue(any(event['kind'] == 'task.renewed' for event in self.store.events()))

    def test_nonzero_exit(self):
        result = self.run_worker('raise SystemExit(7)')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('status 7', result['data']['error'])

    def test_missing_anchor_rejected(self):
        self.store.set_meta('paths', None)
        result = self.run_worker('raise Exception("must not start")')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('Missing trusted', result['data']['error'])

    def test_task_timeout_releases_reservation_after_reaping(self):
        task = self.store.get_record('task', self.task['id'])
        task['data']['payload']['timeout_seconds'] = .15
        self.store.put_record('task', task['id'], task['data'])
        result = self.run_worker('import time; time.sleep(5)', timeout=2)
        self.assertEqual(result['data']['status'], 'failed')
        self.assertEqual(result['data']['reserved_seconds'], 0)
        self.assertTrue(result['data']['execution']['terminated'])

    def test_retry_worker_receives_only_remaining_logical_task_budget(self):
        task = self.store.get_record('task', self.task['id'])
        task['data']['payload']['timeout_seconds'] = .6
        self.store.put_record('task', task['id'], task['data'])
        first = self.run_worker('import time; time.sleep(.15); raise SystemExit(4)')
        self.assertEqual(first['data']['status'], 'failed')
        self.assertGreater(first['data']['spent_seconds'], .15)
        self.coordinator.retry_task(self.task['id'], 'Retry stopped worker')
        from review_core.runner import _exchange
        def checked_exchange(coordinator, task, command, workspace, request, timeout):
            self.assertLessEqual(timeout, task['data']['attempt_budget_seconds'])
            self.assertLess(timeout, .45)
            return _exchange(coordinator, task, command, workspace, request, timeout)
        with patch('review_core.runner._exchange', side_effect=checked_exchange):
            second = self.run_worker('import time; time.sleep(2)')
        self.assertEqual(second['data']['status'], 'failed')
        self.assertIn('timeout', second['data']['error'])
        self.assertEqual(second['data']['reserved_seconds'], 0)
        self.assertLessEqual(second['data']['spent_seconds'], .6)

    def test_launch_intent_commits_before_locked_process_registration(self):
        import subprocess
        launch = subprocess.Popen
        def checked_launch(*args, **kwargs):
            self.assertTrue(self.store.connection.in_transaction)
            with Store(self.store.run_dir, readonly=True) as observer:
                execution = observer.get_record('task', self.task['id'])['data']['execution']
                self.assertTrue(execution['launch_intent'])
                self.assertFalse(execution['terminated'])
                self.assertTrue(Path(execution['workspace']).is_dir())
            return launch(*args, **kwargs)
        with patch('review_core.runner.subprocess.Popen', side_effect=checked_launch):
            result = self.run_worker(RESULT_WORKER)
        self.assertEqual(result['data']['status'], 'completed')
        self.assertTrue(Path(result['data']['execution']['workspace']).is_dir())

    def test_expired_preparation_never_launches_and_releases_budget(self):
        task = self.store.get_record('task', self.task['id'])
        task['data']['payload']['timeout_seconds'] = .1
        self.store.put_record('task', task['id'], task['data'])
        from review_core.runner import verify_snapshot
        def slow_verify(*args, **kwargs):
            time.sleep(.15)
            return verify_snapshot(*args, **kwargs)
        with patch('review_core.runner.verify_snapshot', side_effect=slow_verify):
            with patch('review_core.runner.subprocess.Popen') as launch:
                result = self.run_worker('raise Exception("must not start")')
        launch.assert_not_called()
        self.assertEqual(result['data']['status'], 'failed')
        self.assertEqual(result['data']['reserved_seconds'], 0)

    def test_stderr_diagnostic_preserved(self):
        result = self.run_worker('import sys; print("fixture failure detail", file=sys.stderr); sys.exit(3)')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('fixture failure detail', result['data']['error'])

    def test_invalid_aasm_preserves_stderr_diagnostic(self):
        result = self.run_worker('import sys; print("fixture syntax detail", file=sys.stderr); print("invalid")')
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('fixture syntax detail', result['data']['error'])

    def test_result_ingestion_failure_is_failed_atomically(self):
        def reject(task, result):
            raise ValueError('Invalid typed result')
        self.coordinator.result_handler = reject
        result = self.run_worker(RESULT_WORKER)
        self.assertEqual(result['data']['status'], 'failed')
        self.assertIn('Invalid typed result', result['data']['error'])


if __name__ == '__main__':
    unittest.main()
