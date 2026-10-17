"""Integration tests; all run artifacts live in temporary directories."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

HELPER = Path(__file__).with_name('review-sync.py')


class ReviewSyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'PLAN.md').write_text('Plan\n')
        (self.root / 'message.md').write_text('Review evidence\n')
        self.call('init', '--paths', 'PLAN.md')

    def call(self, *args):
        result = subprocess.run([sys.executable, str(HELPER), '--run', 'test', *args],
                                cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout) if result.stdout.strip() else None

    def publish(self, agent, status):
        return self.call('publish', '--agent', agent, '--status', status,
                         '--text-file', 'message.md', *self.expected_review())

    def expected_review(self):
        state = self.state()
        return ['--expected-revision', state['revision'], '--expected-round', str(state['round'])]

    def approve(self):
        for agent in ('codex', 'claude'):
            self.publish(agent, 'APPROVED')

    def test_later_blocker_revokes_readiness(self):
        self.approve()
        self.publish('claude', 'BLOCKED')
        state = json.loads((self.root / '.reviews/test/state.json').read_text())
        self.assertEqual(state['consensus'], 'BLOCKED')

    def test_check_recomputes_cached_consensus(self):
        self.publish('codex', 'BLOCKED')
        path = self.root / '.reviews/test/state.json'
        state = json.loads(path.read_text())
        state['consensus'] = 'READY_TO_EXECUTE'
        path.write_text(json.dumps(state))
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'BLOCKED')

    def control(self, kind):
        return self.call('control', '--agent', 'claude', '--kind', kind,
                         '--text-file', 'message.md')

    def test_control_delivered_while_snapshot_missing(self):
        (self.root / 'PLAN.md').unlink()
        sent = self.control('stop')
        received = self.call('wait', '--agent', 'codex', '--after', '0', '--timeout', '0')
        self.assertEqual(received, sent)
        self.assertEqual(received['type'], 'control')

    def test_scope_and_stop_require_two_new_approvals(self):
        for kind in ('scope', 'stop'):
            with self.subTest(kind=kind):
                self.approve()
                self.control(kind)
                self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'PENDING')
                self.publish('codex', 'APPROVED')
                self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'PENDING')
                self.publish('claude', 'APPROVED')
                self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'READY_TO_EXECUTE')

    def test_info_does_not_replace_review(self):
        self.approve()
        self.control('info')
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'READY_TO_EXECUTE')

    def test_changed_snapshot_invalidates_approvals(self):
        self.approve()
        (self.root / 'PLAN.md').write_text('Changed plan')
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'STALE_REVISION')
        self.call('revise', '--agent', 'claude')
        self.publish('claude', 'APPROVED')
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'PENDING')

    def test_publish_rejects_review_of_previous_revision(self):
        reviewed = self.state()
        (self.root / 'PLAN.md').write_text('Changed plan')
        self.call('revise', '--agent', 'claude')
        error = self.run_failing('publish', '--agent', 'codex', '--status', 'APPROVED',
                                 '--text-file', 'message.md', '--expected-revision', reviewed['revision'],
                                 '--expected-round', str(reviewed['round']))
        self.assertIn('Review revision or round changed', error)
        self.assertFalse((self.root / '.reviews/test/codex').exists())

    def test_publish_rejects_review_of_previous_round(self):
        reviewed = self.state()
        self.control('scope')
        error = self.run_failing('publish', '--agent', 'codex', '--status', 'APPROVED',
                                 '--text-file', 'message.md', '--expected-revision', reviewed['revision'],
                                 '--expected-round', str(reviewed['round']))
        self.assertIn('Review revision or round changed', error)
        self.assertFalse((self.root / '.reviews/test/codex').exists())

    def test_failed_init_can_be_retried(self):
        command = [sys.executable, str(HELPER), '--run', 'retry', 'init', '--paths', 'missing.md']
        result = subprocess.run(command, cwd=self.root, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / '.reviews/retry/meta.json').exists())
        (self.root / 'missing.md').write_text('Plan')
        result = subprocess.run(command, cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['round'], 1)

    def test_snapshot_rejects_symlinks(self):
        (self.root / 'src').mkdir()
        (self.root / 'src/code.py').write_text('print(1)')
        for target in ('PLAN.md', 'src', 'missing'):
            with self.subTest(target=target):
                link = self.root / 'src/link'
                link.symlink_to(self.root / target)
                command = [sys.executable, str(HELPER), '--run', 'links', 'init', '--paths', 'src']
                result = subprocess.run(command, cwd=self.root, text=True, capture_output=True)
                link.unlink()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Symlink snapshot path', result.stderr)

    def test_snapshot_rejects_symlink_ancestors(self):
        (self.root / 'linked').symlink_to(self.root, target_is_directory=True)
        command = [sys.executable, str(HELPER), '--run', 'ancestor', 'init', '--paths', 'linked/PLAN.md']
        result = subprocess.run(command, cwd=self.root, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Symlink snapshot path', result.stderr)

    def test_technical_plan_has_distinct_result(self):
        result = subprocess.run([sys.executable, str(HELPER), '--run', 'plan', 'init',
                                 '--paths', 'PLAN.md', '--approval-scope', 'technical-plan'],
                                cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        reviewed = json.loads(result.stdout)
        for agent in ('codex', 'claude'):
            result = subprocess.run([sys.executable, str(HELPER), '--run', 'plan', 'publish',
                                     '--agent', agent, '--status', 'APPROVED',
                                     '--text-file', 'message.md', '--expected-revision', reviewed['revision'],
                                     '--expected-round', str(reviewed['round'])],
                                    cwd=self.root, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        state = json.loads((self.root / '.reviews/plan/state.json').read_text())
        self.assertEqual(state['consensus'], 'PLAN_APPROVED')

    def test_conflicting_approval_scope_rejected(self):
        result = subprocess.run([sys.executable, str(HELPER), '--run', 'test', 'init',
                                 '--paths', 'PLAN.md', '--approval-scope', 'technical-plan'],
                                cwd=self.root, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('different paths or approval scope', result.stderr)

    def test_legacy_reviews_remain_readable(self):
        self.approve()
        run = self.root / '.reviews/test'
        meta = json.loads((run / 'meta.json').read_text())
        meta.pop('approval_scope')
        (run / 'meta.json').write_text(json.dumps(meta))
        for agent in ('codex', 'claude'):
            path = run / agent / '00000001.json'
            message = json.loads(path.read_text())
            message.pop('type')
            message.pop('approval_scope')
            path.write_text(json.dumps(message))
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'READY_TO_EXECUTE')

    def state(self):
        return json.loads((self.root / '.reviews/test/state.json').read_text())

    def run_failing(self, *args):
        result = subprocess.run([sys.executable, str(HELPER), '--run', 'test', *args],
                                cwd=self.root, text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        return result.stderr

    def test_changed_helper_rejected_without_mutating_run(self):
        changed = self.root / 'changed-helper.py'
        changed.write_bytes(HELPER.read_bytes() + b'\n# Different helper build\n')
        run = self.root / '.reviews/test'
        before = {str(path.relative_to(run)): path.read_bytes() for path in run.rglob('*') if path.is_file()}
        for command in (['join', '--agent', 'claude'], ['start', '--agent', 'claude'],
                        ['publish', '--agent', 'claude', '--status', 'APPROVED', '--text-file', 'message.md', *self.expected_review()],
                        ['init', '--paths', 'PLAN.md']):
            result = subprocess.run([sys.executable, str(changed), '--run', 'test', *command],
                                    cwd=self.root, text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('Helper version mismatch', result.stderr)
        after = {str(path.relative_to(run)): path.read_bytes() for path in run.rglob('*') if path.is_file()}
        self.assertEqual(before, after)

    def test_identical_helper_copy_can_join(self):
        copied = self.root / 'copied-helper.py'
        copied.write_bytes(HELPER.read_bytes())
        result = subprocess.run([sys.executable, str(copied), '--run', 'test', 'join', '--agent', 'claude'],
                                cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('helper_sha256', json.loads(result.stdout))

    def test_legacy_join_pins_helper_and_invalidates_old_approvals(self):
        self.approve()
        path = self.root / '.reviews/test/meta.json'
        meta = json.loads(path.read_text())
        meta.pop('helper_sha256', None)
        path.write_text(json.dumps(meta))
        joined = self.call('join', '--agent', 'claude')
        self.assertEqual(joined['consensus'], 'PENDING')
        self.assertEqual(joined['round'], 2)
        self.assertIn('helper_sha256', joined)
        self.assertEqual(self.call('join', '--agent', 'codex')['round'], 2)
        self.assertEqual(len(list((self.root / '.reviews/test/codex').glob('*.json'))), 1)

    def test_join_prints_existing_scope(self):
        joined = self.call('join', '--agent', 'claude')
        self.assertEqual(joined['paths'], ['PLAN.md'])
        self.assertEqual(joined['approval_scope'], 'implementation-ready')

    def test_editor_lock_blocks_reviews_until_revise(self):
        self.approve()
        self.call('begin-edit', '--agent', 'claude', '--text-file', 'message.md')
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'PENDING')
        self.assertIn('claude is editing', self.run_failing('begin-edit', '--agent', 'codex', '--text-file', 'message.md'))
        self.assertIn('claude is editing', self.run_failing('publish', '--agent', 'codex', '--status', 'APPROVED',
                                                             '--text-file', 'message.md', *self.expected_review()))
        self.assertIn('only the editor', self.run_failing('revise', '--agent', 'codex'))
        received = self.call('wait', '--agent', 'codex', '--after', '1', '--timeout', '0')
        self.assertEqual((received['type'], received['kind']), ('control', 'edit'))
        (self.root / 'PLAN.md').write_text('Edited plan\n')
        self.assertEqual(self.call('revise', '--agent', 'claude')['round'], 2)
        self.assertNotIn('editor', self.state())
        self.approve()
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'READY_TO_EXECUTE')

    def test_revise_without_changes_releases_editor(self):
        self.call('begin-edit', '--agent', 'claude', '--text-file', 'message.md')
        state = self.call('revise', '--agent', 'claude')
        self.assertEqual(state['round'], 1)
        self.assertNotIn('editor', state)

    def test_decision_keeps_approvals(self):
        self.approve()
        self.control('decision')
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'READY_TO_EXECUTE')
        self.assertEqual(self.state()['round'], 1)

    def test_gates_recorded_and_cleared(self):
        self.call('publish', '--agent', 'codex', '--status', 'APPROVED', '--text-file', 'message.md',
                  '--gate', 'Metrics need user approval', *self.expected_review())
        self.publish('claude', 'APPROVED')
        self.assertEqual(self.call('check', '--agent', 'codex')['gates'], ['Metrics need user approval'])
        self.publish('claude', 'BLOCKED')
        self.assertEqual(self.state()['gates'], [])
        self.assertIn('only valid with APPROVED', self.run_failing('publish', '--agent', 'codex', '--status', 'BLOCKED',
                                                                    '--text-file', 'message.md', '--gate', 'x', *self.expected_review()))

    def test_uppercase_type_and_verdict_field_readable(self):
        self.approve()
        path = self.root / '.reviews/test/claude/00000001.json'
        message = json.loads(path.read_text())
        message['type'] = 'REVIEW'
        message['verdict'] = message.pop('status')
        path.write_text(json.dumps(message))
        self.assertEqual(self.call('check', '--agent', 'codex')['consensus'], 'READY_TO_EXECUTE')

    def start_watcher(self, pid):
        self.call('start', '--agent', 'codex')
        watcher = subprocess.Popen([sys.executable, str(HELPER), '--run', 'test',
                                    'heartbeat', '--agent', 'codex', '--session-pid', str(pid),
                                    '--interval', '0.02'], cwd=self.root,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(self.stop_process, watcher)
        path = self.root / '.reviews/test/codex.heartbeat.json'
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if json.loads(path.read_text()).get('session_pid') == pid:
                return watcher
            time.sleep(0.01)
        self.fail('Watcher did not start')

    @staticmethod
    def stop_process(process):
        if process.poll() is None:
            process.terminate()
        process.wait(timeout=3)

    def test_heartbeat_stops_when_session_exits(self):
        session = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
        self.addCleanup(self.stop_process, session)
        watcher = self.start_watcher(session.pid)
        session.terminate()
        # An exited but unreaped process must also stop its watcher.
        watcher.wait(timeout=3)
        self.assertEqual(watcher.returncode, 0)

    def test_heartbeat_stops_on_finished_and_restart(self):
        for action in ('finish', 'restart'):
            with self.subTest(action=action):
                watcher = self.start_watcher(os.getpid())
                if action == 'finish':
                    self.call('event', '--agent', 'codex', '--kind', 'FINISHED')
                else:
                    self.call('start', '--agent', 'codex')
                watcher.wait(timeout=3)
                self.assertEqual(watcher.returncode, 0)


if __name__ == '__main__':
    unittest.main()
