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
if request['task']['kind'] == 'discussion':
    context = json.loads(request['task']['brief'].split('Source context: ', 1)[1])
    for question in context['question_records']:
        print('A id=answer-' + question['id'] + ' question=' + question['id'] +
              ' text="Compared captured evidence" evidence=' + json.dumps(proofs))
    raise SystemExit(0)
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
        self.cross_check()
        self.cli('challenge')
        self.assertEqual(self.work('challenger')['data']['status'], 'completed')
        self.assertEqual(self.cli('check')['result'], 'APPROVED')

    def cross_check(self):
        for worker in ('reviewer-1', 'reviewer-2'):
            result = self.cli('poll', '--worker', worker, '--wait-seconds', '0.01')
            self.assertEqual(result['state'], 'ready')
            self.assertEqual(self.work(worker)['data']['status'], 'completed')

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
        self.cross_check()
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

    def chat_approval(self, **changes):
        approval = {'run_id': 'sequence', 'revision': self.cli('status', '--json')['revision'],
                    'approval_scope': 'technical-plan', 'actions': ['review', 'inspect'],
                    'paths': ['input.txt'], 'message': 'Go ahead with this review.',
                    'reference': 'lead chat: user reply to the displayed review scope'}
        approval.update(changes)
        return approval

    def test_chat_approval_records_evidence_and_allows_scoped_work(self):
        approval = self.chat_approval()
        grant = self.cli('authorize', '--chat-approval', '-', input=json.dumps(approval))
        self.assertEqual(grant['data']['actor'], 'user')
        self.assertEqual(grant['data']['approval'], {
            'source': 'chat', 'message': approval['message'], 'reference': approval['reference']})
        self.assertEqual(grant['data']['revision'], approval['revision'])
        self.assertEqual(grant['data']['actions'], ['inspect', 'review'])
        self.assertEqual(grant['data']['paths'], ['input.txt'])
        self.cli('inspect', '--paths', 'input.txt')
        self.cli('plan')
        self.assertEqual(self.work('reviewer-1')['data']['status'], 'completed')

    def test_invalid_chat_approval_does_not_mutate(self):
        valid = self.chat_approval()
        invalid = [dict(valid, **change) for change in (
            {'run_id': 'other'}, {'revision': 'stale'}, {'approval_scope': 'implementation-ready'},
            {'paths': ['.']}, {'paths': []}, {'actions': []}, {'message': ' '},
            {'reference': ''}, {'unexpected': 'value'})]
        invalid.append({key: value for key, value in valid.items() if key != 'reference'})
        with Store(self.run, readonly=True) as store:
            before = store.events()
        for payload in invalid:
            with self.subTest(payload=payload):
                self.cli('authorize', '--chat-approval', '-', expected=2, input=json.dumps(payload))
        for flags in (('--actions', 'execute'), ('--paths', '.'), ('--revoke', 'missing'),
                      ('--expires-at', '9999999999'), ('--reason', 'changed')):
            self.cli('authorize', '--chat-approval', '-', *flags, expected=2, input=json.dumps(valid))
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.events(), before)
            self.assertEqual(store.list_records('authorization'), [])

    def test_chat_approval_does_not_transfer_to_changed_revision(self):
        approval = self.chat_approval()
        self.cli('authorize', '--chat-approval', '-', input=json.dumps(approval))
        (self.project / 'input.txt').write_text('changed review input\n')
        self.cli('revise')
        self.cli('inspect', '--paths', 'input.txt', expected=2)
        self.cli('authorize', '--chat-approval', '-', expected=2, input=json.dumps(approval))

    def test_startup_handoff_precedes_go_and_one_go_covers_later_revisions(self):
        checkpoint = self.cli('handoff', '--worker', 'reviewer-2')
        self.assertEqual('sequence', checkpoint['run_id'])
        self.assertEqual(str(self.run), checkpoint['run_dir'])
        self.assertEqual([], checkpoint['tasks'])
        self.assertFalse(checkpoint['start_authorized'])
        self.assertFalse(checkpoint['start_approval_recorded'])
        self.cli('plan')
        blocked = self.work('reviewer-1')
        self.assertEqual('blocked', blocked['state'])
        approval = self.chat_approval(follow_revisions=True, message='go')
        self.cli('authorize', '--chat-approval', '-', input=json.dumps(approval))
        self.assertTrue(self.cli('handoff', '--worker', 'reviewer-2')['start_authorized'])
        self.assertEqual('completed', self.work('reviewer-1')['data']['status'])
        self.assertEqual('completed', self.work('reviewer-2')['data']['status'])
        self.cross_check()
        self.cli('challenge')
        self.work('challenger')
        self.assertEqual('APPROVED', self.cli('check')['result'])

        (self.project / 'input.txt').write_text('new evidence after the initial go\n')
        changed = self.cli('revise')
        self.assertNotEqual(approval['revision'], changed['revision'])
        self.assertEqual('BLOCKED', self.cli('check', expected=1)['result'])
        self.cli('inspect', '--paths', 'input.txt')
        self.cli('plan')
        self.assertEqual('completed', self.work('reviewer-1')['data']['status'])
        self.assertEqual('completed', self.work('reviewer-2')['data']['status'])
        with Store(self.run, readonly=True) as store:
            grants = store.list_records('authorization')
            self.assertEqual(1, len(grants))
            self.assertEqual('go', grants[0]['data']['approval']['message'])

    def test_initial_go_covers_evidenced_readonly_recovery_but_not_execute(self):
        self.cli('authorize', '--chat-approval', '-', input=json.dumps(self.chat_approval(follow_revisions=True)))
        self.cli('plan')
        task = self.cli('task', 'claim', '--data', '-', input='{"worker":"reviewer-1"}')
        with Store(self.run) as store:
            data = task['data']
            data.update(status='uncertain', execution={'terminated': True})
            store.put_record('task', task['id'], data)
        request = json.dumps({'task_id': task['id'], 'outcome': 'failed', 'evidence': 'Worker stopped without a result'})
        recovered = self.cli('recover', '--data', '-', input=request)
        self.assertEqual('failed', recovered['data']['status'])
        self.assertEqual('Worker stopped without a result', recovered['data']['recovery_decisions'][0]['evidence'])
        with Store(self.run) as store:
            data.update(kind='execute', status='uncertain')
            store.put_record('task', task['id'], data)
        denied = self.cli('recover', '--data', '-', input=request, expected=2)
        self.assertIn('terminal', denied['error'])

    def test_handoff_identifies_shared_run_and_second_reviewer_can_claim(self):
        self.cli('authorize', '--chat-approval', '-', input=json.dumps(self.chat_approval()))
        self.cli('plan')
        completed = self.work('reviewer-1')
        checkpoint = self.cli('handoff', '--worker', 'reviewer-2')
        self.assertNotIn(completed['data']['result']['evidence_ids'][0], json.dumps(checkpoint))
        self.assertEqual(checkpoint['run_id'], 'sequence')
        self.assertEqual(checkpoint['run_dir'], str(self.run))
        self.assertEqual(checkpoint['project_root'], str(self.project))
        self.assertEqual(checkpoint['paths'], ['input.txt'])
        self.assertEqual(checkpoint['approval_scope'], 'technical-plan')
        request = json.dumps({'worker': 'reviewer-2'})
        claimed = subprocess.run([
            sys.executable, '-B', str(Path(checkpoint['run_dir']) / 'protocol' / 'review.py'),
            '--root', str(Path(checkpoint['run_dir']).parent), '--run', checkpoint['run_id'],
            'task', 'claim', '--data', '-'], cwd=self.base, input=request,
            text=True, capture_output=True, timeout=30)
        self.assertEqual(claimed.returncode, 0, claimed.stderr)
        task = json.loads(claimed.stdout)
        self.assertEqual(task['data']['worker'], 'reviewer-2')
        self.assertEqual(task['data']['payload']['revision'], checkpoint['revision'])

    def test_two_chats_poll_through_challenge_and_completion(self):
        self.reviews()
        self.cross_check()
        first = self.cli('poll', '--worker', 'reviewer-1', '--wait-seconds', '0.01')
        second = self.cli('poll', '--worker', 'reviewer-2', '--wait-seconds', '0.01')
        self.assertEqual(len(first['challenge_tasks']), 1)
        self.assertEqual(second['challenge_tasks'], [])
        self.assertEqual(self.work('challenger')['data']['status'], 'completed')
        for worker, previous in (('reviewer-1', first), ('reviewer-2', second)):
            terminal = self.cli('poll', '--worker', worker, '--after', str(previous['next_after']),
                                '--wait-seconds', '0.01')
            self.assertEqual(terminal['state'], 'complete')
            self.assertEqual(terminal['approval']['result'], 'APPROVED')

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
        self.cross_check()
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
