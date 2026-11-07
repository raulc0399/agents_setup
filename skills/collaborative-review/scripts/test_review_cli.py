"""Real subprocess CLI sequences against temporary projects and local adapters."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from review_core.authorization import record_authorization
from review_core.store import Store

SCRIPTS = Path(__file__).resolve().parent
ADAPTER = '''import hashlib, json, pathlib, sys
request = json.load(sys.stdin)
proofs, paths = [], []
for entry in request['snapshot']['files']:
    path = entry['path']
    content = (pathlib.Path(request['snapshot']['root']) / path).read_bytes()
    assert hashlib.sha256(content).hexdigest() == entry['sha256']
    paths.append(path)
    proofs.append(path + ':sha256:' + hashlib.sha256(content).hexdigest())
identity = 'coverage-' + request['task']['id']
print('COV id=' + identity + ' paths=' + json.dumps(paths) +
      ' status=reviewed evidence=' + json.dumps(proofs))
print('VOTE verdict=APPROVED evidence=' + json.dumps([identity]))
'''


class ReviewCliTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.project = self.base / 'project'
        self.project.mkdir()
        (self.project / 'input.txt').write_text('captured evidence\n')
        self.root = self.base / 'runs'
        self.run = self.root / 'sequence'
        self.entry = SCRIPTS / 'review.py'
        self.adapter = self.base / 'adapter.py'
        self.adapter.write_text(ADAPTER)
        self.cli('create', '--project', str(self.project), '--paths', 'input.txt',
                 '--budget-seconds', '120', '--max-workers', '1')

    def tearDown(self):
        self.temp.cleanup()

    def invoke(self, *args, input=None):
        return subprocess.run([sys.executable, '-B', str(self.entry), '--root', str(self.root),
                               '--run', 'sequence', *args], cwd=self.project, input=input,
                              text=True, capture_output=True, timeout=30)

    def cli(self, *args, expected=0, input=None):
        result = self.invoke(*args, input=input)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return json.loads(result.stdout if result.stdout else result.stderr)

    def grant(self):
        with Store(self.run) as store:
            record_authorization(store, store.get_meta('revision'), ['review', 'inspect'], ['input.txt'])

    def work(self, worker):
        return self.cli('work', '--worker', worker, '--timeout-seconds', '5',
                        '--', sys.executable, str(self.adapter))

    def reviews(self):
        self.grant()
        self.cli('plan')
        for worker in ('reviewer-1', 'reviewer-2'):
            self.assertEqual(self.work(worker)['data']['status'], 'completed')

    def approved(self):
        self.reviews()
        self.cli('check', expected=1)
        self.cli('challenge')
        self.assertEqual(self.work('challenger')['data']['status'], 'completed')
        self.assertEqual(self.cli('check')['result'], 'APPROVED')

    def test_full_review_challenge_finish_and_exports(self):
        self.approved()
        self.assertEqual(self.cli('finish')['result'], 'APPROVED')
        self.assertEqual(self.cli('status', '--json')['status'], 'complete')
        with Store(self.run, readonly=True) as store:
            before = store.events()
        self.cli('export')
        for name in ('OPTIONS.md', 'DECISIONS.md', 'SUMMARY.md', 'TRANSCRIPT.html'):
            self.assertTrue((self.run / 'exports' / name).is_file(), name)
        self.assertIn('APPROVED', (self.run / 'exports' / 'SUMMARY.md').read_text())
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.events(), before)
        self.assertEqual((self.project / 'input.txt').read_text(), 'captured evidence\n')

    def test_revision_invalidates_reviews_and_authorization(self):
        self.approved()
        previous = self.cli('status', '--json')['revision']
        (self.project / 'input.txt').write_text('new evidence\n')
        self.assertEqual(self.cli('check')['result'], 'APPROVED')
        changed = self.cli('revise')
        self.assertNotEqual(changed['revision'], previous)
        self.assertEqual(changed['changed_paths'], ['input.txt'])
        self.cli('check', expected=1)
        self.cli('plan')
        blocked = self.work('reviewer-1')
        self.assertEqual(blocked['state'], 'blocked')
        self.assertTrue(any(reason['code'] == 'authorization' for task in blocked['availability']['tasks']
                            for reason in task['blockers']))
        self.grant()
        self.assertEqual(self.work('reviewer-1')['data']['status'], 'completed')
        self.assertEqual(self.work('reviewer-2')['data']['status'], 'completed')
        challenge = self.cli('challenge')
        context = json.loads(challenge['data']['payload']['brief'].split('Context: ', 1)[1])
        self.assertEqual(context['changed_paths'], ['input.txt'])

    def test_unattended_authorization_and_recovery_do_not_mutate(self):
        with Store(self.run, readonly=True) as store:
            before = store.events()
        denied = self.cli('authorize', '--paths', 'input.txt', expected=2, input='APPROVE\n')
        self.assertIn('terminal', denied['error'])
        denied = self.cli('recover', '--data', '-', expected=2,
                          input=json.dumps({'task_id': 'missing', 'outcome': 'retry', 'evidence': 'stopped'}))
        self.assertIn('terminal', denied['error'])
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.events(), before)
            self.assertEqual(store.list_records('authorization'), [])

    def test_failed_adapter_retry_preserves_identity_and_history(self):
        self.grant()
        self.cli('plan')
        self.adapter.write_text('raise SystemExit(7)\n')
        failed = self.work('reviewer-1')
        self.assertEqual(failed['data']['status'], 'failed')
        self.cli('task', 'retry', '--data', '-', input=json.dumps({
            'task_id': failed['id'], 'reason': 'adapter repaired'}))
        self.adapter.write_text(ADAPTER)
        completed = self.work('reviewer-1')
        self.assertIsNotNone(completed, 'An explicit retry must remain claimable within its planned budget')
        self.assertEqual(completed['id'], failed['id'])
        self.assertEqual(completed['data']['status'], 'completed')
        self.assertEqual(completed['data']['attempt'], 2)
        self.assertEqual(completed['data']['attempts'][0]['status'], 'failed')
        self.assertNotEqual(completed['data']['lease_id'], failed['data']['lease_id'])
        self.assertEqual(self.work('reviewer-2')['data']['status'], 'completed')
        self.cli('challenge')
        self.assertEqual(self.work('challenger')['data']['status'], 'completed')
        self.assertEqual(self.cli('finish')['result'], 'APPROVED')

    def test_status_and_check_are_consistent_readonly_views(self):
        self.reviews()
        with Store(self.run, readonly=True) as store:
            before = store.events()
        status = self.cli('status', '--json')
        self.assertEqual(status['approval'], self.cli('check', expected=1))
        terminal = self.invoke('status')
        self.assertEqual(terminal.returncode, 0, terminal.stderr)
        self.assertIn('Approval: BLOCKED', terminal.stdout)
        self.assertIn(status['revision'], terminal.stdout)
        self.assertEqual(status['budget']['reserved_seconds'], 0)
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.events(), before)

    def test_doctor_failure_is_distinct_from_invalid_cli_request(self):
        before = sorted(path.name for path in self.project.iterdir())
        diagnosis = self.cli('doctor', '--project', str(self.project), '--',
                             str(self.base / 'missing-executable'), expected=1)
        self.assertFalse(diagnosis['ok'])
        self.assertTrue(any(check['name'] == 'worker_command' and not check['ok']
                            for check in diagnosis['checks']))
        self.assertEqual(sorted(path.name for path in self.project.iterdir()), before)
        self.cli('record', 'review', '--data', '-', input='[]', expected=2)

    def test_existing_run_uses_pinned_runtime_after_current_code_changes(self):
        runtime = self.base / 'runtime'
        shutil.copytree(SCRIPTS, runtime, ignore=shutil.ignore_patterns('__pycache__', 'test_*', 'fixtures'))
        self.entry = runtime / 'review.py'
        (runtime / 'review_core' / 'views.py').write_text("raise RuntimeError('current runtime changed')\n")
        self.assertEqual(self.cli('status', '--json')['status'], 'ready')
        current_cli = runtime / 'review_core' / 'cli.py'
        source = current_cli.read_text()
        self.assertIn("commands.add_parser('status'", source)
        current_cli.write_text(source.replace("commands.add_parser('status'", "commands.add_parser('state'"))
        self.assertEqual(self.cli('status', '--json')['status'], 'ready')
        self.assertFalse(any((self.run / 'protocol').rglob('*.pyc')))


if __name__ == '__main__':
    unittest.main()
