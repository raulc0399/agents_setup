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
        initialized = self.call('init', '--paths', 'PLAN.md')
        self.initial_state = initialized.copy()
        initialized['startup_ready'] = True
        (self.root / '.reviews/test/state.json').write_text(json.dumps(initialized))

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
        for agent in ('agent-1', 'agent-2'):
            self.publish(agent, 'APPROVED')

    def test_later_blocker_revokes_readiness(self):
        self.approve()
        self.publish('agent-2', 'BLOCKED')
        state = json.loads((self.root / '.reviews/test/state.json').read_text())
        self.assertEqual(state['consensus'], 'BLOCKED')

    def test_check_recomputes_cached_consensus(self):
        self.publish('agent-1', 'BLOCKED')
        path = self.root / '.reviews/test/state.json'
        state = json.loads(path.read_text())
        state['consensus'] = 'READY_TO_EXECUTE'
        path.write_text(json.dumps(state))
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'BLOCKED')

    def test_both_slots_refresh_status(self):
        status_path = self.root / '.reviews/test/status.json'
        for agent in ('agent-1', 'agent-2'):
            status_path.write_text(json.dumps({'consensus': 'OUTDATED'}))
            checked = self.call('check', '--agent', agent)
            status = json.loads(status_path.read_text())
            self.assertEqual(status['consensus'], checked['consensus'])
            self.assertNotEqual(status['consensus'], 'OUTDATED')

    def control(self, kind):
        return self.send_control('agent-2', kind, 'Review evidence')

    def send_control(self, agent, kind, body):
        (self.root / 'message.md').write_text(body)
        return self.call('control', '--agent', agent, '--kind', kind,
                         '--text-file', 'message.md')

    def write_message(self, body):
        path = self.root / 'message.md'
        path.write_text(body)
        return str(path)

    def messages(self, agent):
        directory = self.root / f'.reviews/test/{agent}'
        return sorted(path.name for path in directory.glob('*.json')) if directory.exists() else []

    def set_startup_ready(self, ready):
        path = self.root / '.reviews/test/state.json'
        state = json.loads(path.read_text())
        state['startup_ready'] = ready
        path.write_text(json.dumps(state))

    def start_both_agents(self):
        self.call('start', '--agent', 'agent-1')
        self.call('join', '--agent', 'agent-2')

    def send_kick(self, agent='agent-1', body='KICK run=test prob="test problem" exe=agent-1 rep=agent-2'):
        return self.send_control(agent, 'info', body)

    def send_go(self, agent='agent-2', body='USR "User approved kickoff" => GO'):
        return self.send_control(agent, 'decision', body)

    def fail_go(self, agent='agent-2', body='USR "User approved kickoff" => GO'):
        (self.root / 'message.md').write_text(body)
        return self.run_failing('control', '--agent', agent, '--kind', 'decision', '--text-file', 'message.md')

    def test_init_starts_with_startup_gate_closed(self):
        self.assertIn('startup_ready', self.initial_state)
        self.assertFalse(self.initial_state['startup_ready'])

    def test_unready_run_rejects_review_publish_and_edit(self):
        self.set_startup_ready(False)
        self.assertIn('startup gate', self.run_failing('event', '--agent', 'agent-1', '--kind', 'REVIEWING').lower())
        self.assertIn('startup gate', self.run_failing('publish', '--agent', 'agent-1', '--status', 'APPROVED',
                                                       '--text-file', 'message.md', *self.expected_review()).lower())
        self.assertIn('startup gate', self.run_failing('begin-edit', '--agent', 'agent-1', '--text-file', 'message.md').lower())
        self.assertFalse(self.state()['startup_ready'])

    def test_go_rejects_absent_sessions_and_missing_kick(self):
        self.set_startup_ready(False)
        self.assertIn('startup gate', self.fail_go().lower())
        self.start_both_agents()
        self.assertIn('startup gate', self.fail_go().lower())
        self.assertFalse(self.state()['startup_ready'])

    def test_go_requires_valid_kick_peer_ack_and_reporter(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        kick = self.send_kick()
        self.assertIn('startup gate', self.fail_go().lower())
        self.assertIn('reporter', self.fail_go('agent-1').lower())
        self.assertIn('existing peer message', self.run_failing('control', '--agent', 'agent-2', '--kind', 'info',
                                                               '--text-file', self.write_message('ACK 999')))
        self.assertIn('startup gate', self.fail_go().lower())
        self.assertIn('existing peer message', self.run_failing('control', '--agent', 'agent-1', '--kind', 'info',
                                                               '--text-file', self.write_message(f'ACK {kick["seq"]}')))
        self.assertIn('startup gate', self.fail_go().lower())
        self.assertFalse(self.state()['startup_ready'])
        self.assertEqual(self.messages('agent-2'), [])
        self.send_control('agent-2', 'info', f'ACK {kick["seq"]}')
        heartbeat_path = self.root / '.reviews/test/agent-1.heartbeat.json'
        heartbeat = json.loads(heartbeat_path.read_text())
        heartbeat['time'] = time.time() - 121
        heartbeat_path.write_text(json.dumps(heartbeat))
        self.assertIn('heartbeat', self.fail_go().lower())
        self.call('heartbeat', '--agent', 'agent-1')
        self.send_go()
        self.assertTrue(self.state()['startup_ready'])
        self.call('event', '--agent', 'agent-1', '--kind', 'REVIEWING')
        self.publish('agent-2', 'BLOCKED')
        self.call('begin-edit', '--agent', 'agent-1', '--text-file', 'message.md')
        self.call('revise', '--agent', 'agent-1')
        heartbeat_path = self.root / '.reviews/test/agent-1.heartbeat.json'
        heartbeat = json.loads(heartbeat_path.read_text())
        heartbeat['time'] = time.time() - 121
        heartbeat_path.write_text(json.dumps(heartbeat))
        self.call('start', '--agent', 'agent-1')
        self.assertTrue(self.state()['startup_ready'])

    def test_go_rejects_wrong_run_or_malformed_kick(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        self.send_kick(body='KICK run=other prob="test problem" exe=agent-1 rep=agent-2')
        self.send_control('agent-2', 'info', 'ACK 1')
        self.assertIn('KICK', self.fail_go())

    def test_go_rejects_malformed_kick_fields(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        self.send_kick(body='KICK run=test prob="test problem" exe=other rep=agent-2')
        self.send_control('agent-2', 'info', 'ACK 1')
        self.assertIn('KICK', self.fail_go())

    def test_ack_cannot_reference_future_kick(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        self.assertIn('existing peer message', self.run_failing('control', '--agent', 'agent-2', '--kind', 'info',
                                                               '--text-file', self.write_message('ACK 1')))
        self.send_kick()
        self.assertIn('startup gate', self.fail_go().lower())
        self.assertFalse(self.state()['startup_ready'])

    def test_go_does_not_parse_quoted_kick_or_ack_text(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        self.send_control('agent-1', 'info', 'Q 1Q1 "KICK run=test prob=\"fake\" exe=agent-1 rep=agent-2"')
        self.send_control('agent-2', 'info', 'Q 2Q1 "ACK 1"')
        self.assertIn('KICK', self.fail_go())

    def test_go_rejects_inactive_peer_session(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        kick = self.send_kick()
        self.send_control('agent-2', 'info', f'ACK {kick["seq"]}')
        path = self.root / '.reviews/test/agent-1.session.json'
        session = json.loads(path.read_text())
        session['active'] = False
        path.write_text(json.dumps(session))
        self.assertIn('session', self.fail_go().lower())

    def test_first_slot_kick_wins_collision_and_valid_go_latches(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        self.send_kick('agent-2', 'KICK run=test prob="second slot proposal" exe=agent-1 rep=agent-2')
        first_kick = self.send_kick('agent-1', 'KICK run=test prob="first slot proposal" exe=agent-2 rep=agent-1')
        self.send_control('agent-2', 'info', f'ACK {first_kick["seq"]}\nQ 1Q1 "acknowledged kickoff"')
        self.send_go('agent-1', 'USR "User said \\"go\\"" => GO\nQ 1Q2 "multiline decision"')
        self.assertTrue(self.state()['startup_ready'])

    def test_corrected_kick_requires_ack_of_latest_proposal(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        self.send_kick(body='KICK run=test prob="first attempt" exe=agent-1 rep=agent-2')
        corrected = self.send_kick(body='KICK run=test prob="corrected problem" exe=agent-1 rep=agent-2')
        self.send_control('agent-2', 'info', 'ACK 1')
        self.fail_go()
        self.assertFalse(self.state()['startup_ready'])
        self.send_control('agent-2', 'info', f'ACK {corrected["seq"]}')
        self.send_go()
        self.assertTrue(self.state()['startup_ready'])

    def test_go_text_inside_user_quote_is_not_a_go_request(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        self.send_kick()
        result = self.send_control('agent-2', 'decision', 'USR "Do not send => GO yet" => WAIT')
        self.assertEqual(result['kind'], 'decision')
        self.assertFalse(self.state()['startup_ready'])

    def test_go_can_be_relayed_after_peer_becomes_stale(self):
        self.set_startup_ready(True)
        self.start_both_agents()
        heartbeat_path = self.root / '.reviews/test/agent-1.heartbeat.json'
        heartbeat = json.loads(heartbeat_path.read_text())
        heartbeat['time'] = time.time() - 121
        heartbeat_path.write_text(json.dumps(heartbeat))
        result = self.send_go()
        self.assertEqual(result['kind'], 'decision')
        self.assertTrue(self.state()['startup_ready'])

    def test_recovery_wait_ignores_long_stale_peer(self):
        self.start_both_agents()
        heartbeat_path = self.root / '.reviews/test/agent-2.heartbeat.json'
        heartbeat = json.loads(heartbeat_path.read_text())
        heartbeat['time'] = time.time() - 7200
        heartbeat_path.write_text(json.dumps(heartbeat))
        wait = [sys.executable, str(HELPER), '--run', 'test', 'wait', '--agent', 'agent-1',
                '--after', '0', '--timeout', '1', '--interval', '0.5']
        stale = subprocess.run(wait, cwd=self.root, capture_output=True, text=True)
        self.assertEqual(stale.returncode, 3)
        recovery = subprocess.run([*wait, '--ignore-stale'], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(recovery.returncode, 2)
        self.send_control('agent-2', 'info', 'Q 2RD1 "ready slot=agent-2 cli=codex"')
        resumed = subprocess.run([*wait, '--ignore-stale'], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(resumed.returncode, 0, resumed.stderr)

    def test_staggered_readiness_message_wakes_first_agent(self):
        self.call('start', '--agent', 'agent-1')
        self.send_control('agent-1', 'info', 'Q 1RD1 "ready slot=agent-1 cli=codex"')
        self.call('join', '--agent', 'agent-2')
        ready = self.send_control('agent-2', 'info', 'Q 2RD1 "ready slot=agent-2 cli=codex"')
        received = self.call('wait', '--agent', 'agent-1', '--after', '0', '--timeout', '0')
        self.assertEqual(received, ready)

    def test_go_not_latched_without_explicit_reporter_decision(self):
        self.set_startup_ready(False)
        self.start_both_agents()
        kick = self.send_kick()
        self.send_control('agent-2', 'info', f'ACK {kick["seq"]}')
        self.send_control('agent-2', 'decision', 'USR "User approved kickoff" => WAIT')
        self.assertFalse(self.state()['startup_ready'])
        self.assertIn('startup gate', self.run_failing('publish', '--agent', 'agent-1', '--status', 'APPROVED',
                                                       '--text-file', 'message.md', *self.expected_review()).lower())

    def test_control_delivered_while_snapshot_missing(self):
        (self.root / 'PLAN.md').unlink()
        sent = self.control('stop')
        received = self.call('wait', '--agent', 'agent-1', '--after', '0', '--timeout', '0')
        self.assertEqual(received, sent)
        self.assertEqual(received['type'], 'control')

    def test_scope_and_stop_require_two_new_approvals(self):
        for kind in ('scope', 'stop'):
            with self.subTest(kind=kind):
                self.approve()
                self.control(kind)
                self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'PENDING')
                self.publish('agent-1', 'APPROVED')
                self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'PENDING')
                self.publish('agent-2', 'APPROVED')
                self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'READY_TO_EXECUTE')

    def test_info_does_not_replace_review(self):
        self.approve()
        self.control('info')
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'READY_TO_EXECUTE')

    def test_changed_snapshot_invalidates_approvals(self):
        self.approve()
        (self.root / 'PLAN.md').write_text('Changed plan')
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'STALE_REVISION')
        self.call('revise', '--agent', 'agent-2')
        self.publish('agent-2', 'APPROVED')
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'PENDING')

    def test_publish_rejects_review_of_previous_revision(self):
        reviewed = self.state()
        (self.root / 'PLAN.md').write_text('Changed plan')
        self.call('revise', '--agent', 'agent-2')
        error = self.run_failing('publish', '--agent', 'agent-1', '--status', 'APPROVED',
                                 '--text-file', 'message.md', '--expected-revision', reviewed['revision'],
                                 '--expected-round', str(reviewed['round']))
        self.assertIn('Review revision or round changed', error)
        self.assertFalse((self.root / '.reviews/test/agent-1').exists())

    def test_publish_rejects_review_of_previous_round(self):
        reviewed = self.state()
        self.control('scope')
        error = self.run_failing('publish', '--agent', 'agent-1', '--status', 'APPROVED',
                                 '--text-file', 'message.md', '--expected-revision', reviewed['revision'],
                                 '--expected-round', str(reviewed['round']))
        self.assertIn('Review revision or round changed', error)
        self.assertFalse((self.root / '.reviews/test/agent-1').exists())

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
        self.assert_scope_result('technical-plan', 'PLAN_APPROVED')

    def test_report_ready_has_distinct_result(self):
        self.assert_scope_result('report-ready', 'REPORT_APPROVED')

    def assert_scope_result(self, scope, expected):
        result = subprocess.run([sys.executable, str(HELPER), '--run', 'plan', 'init',
                                 '--paths', 'PLAN.md', '--approval-scope', scope],
                                cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        reviewed = json.loads(result.stdout)
        reviewed['startup_ready'] = True
        (self.root / '.reviews/plan/state.json').write_text(json.dumps(reviewed))
        for agent in ('agent-1', 'agent-2'):
            result = subprocess.run([sys.executable, str(HELPER), '--run', 'plan', 'publish',
                                     '--agent', agent, '--status', 'APPROVED',
                                     '--text-file', 'message.md', '--expected-revision', reviewed['revision'],
                                     '--expected-round', str(reviewed['round'])],
                                    cwd=self.root, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        state = json.loads((self.root / '.reviews/plan/state.json').read_text())
        self.assertEqual(state['consensus'], expected)

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
        for agent in ('agent-1', 'agent-2'):
            path = run / agent / '00000001.json'
            message = json.loads(path.read_text())
            message.pop('type')
            message.pop('approval_scope')
            path.write_text(json.dumps(message))
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'READY_TO_EXECUTE')

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
        for command in (['join', '--agent', 'agent-2'], ['start', '--agent', 'agent-2'],
                        ['publish', '--agent', 'agent-2', '--status', 'APPROVED', '--text-file', 'message.md', *self.expected_review()],
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
        result = subprocess.run([sys.executable, str(copied), '--run', 'test', 'join', '--agent', 'agent-2'],
                                cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('helper_sha256', json.loads(result.stdout))

    def test_legacy_join_pins_helper_and_invalidates_old_approvals(self):
        self.approve()
        path = self.root / '.reviews/test/meta.json'
        meta = json.loads(path.read_text())
        meta.pop('helper_sha256', None)
        path.write_text(json.dumps(meta))
        joined = self.call('join', '--agent', 'agent-2')
        self.assertEqual(joined['consensus'], 'PENDING')
        self.assertEqual(joined['round'], 2)
        self.assertIn('helper_sha256', joined)
        self.assertEqual(self.call('join', '--agent', 'agent-1')['round'], 2)
        self.assertEqual(len(list((self.root / '.reviews/test/agent-1').glob('*.json'))), 1)

    def test_join_prints_existing_scope(self):
        joined = self.call('join', '--agent', 'agent-2')
        self.assertEqual(joined['paths'], ['PLAN.md'])
        self.assertEqual(joined['approval_scope'], 'implementation-ready')

    def test_editor_lock_blocks_reviews_until_revise(self):
        self.approve()
        self.call('begin-edit', '--agent', 'agent-2', '--text-file', 'message.md')
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'PENDING')
        self.assertIn('agent-2 is editing', self.run_failing('begin-edit', '--agent', 'agent-1', '--text-file', 'message.md'))
        self.assertIn('agent-2 is editing', self.run_failing('publish', '--agent', 'agent-1', '--status', 'APPROVED',
                                                             '--text-file', 'message.md', *self.expected_review()))
        self.assertIn('only the editor', self.run_failing('revise', '--agent', 'agent-1'))
        received = self.call('wait', '--agent', 'agent-1', '--after', '1', '--timeout', '0')
        self.assertEqual((received['type'], received['kind']), ('control', 'edit'))
        (self.root / 'PLAN.md').write_text('Edited plan\n')
        self.assertEqual(self.call('revise', '--agent', 'agent-2')['round'], 2)
        self.assertNotIn('editor', self.state())
        self.approve()
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'READY_TO_EXECUTE')

    def test_revise_without_changes_releases_editor(self):
        self.call('begin-edit', '--agent', 'agent-2', '--text-file', 'message.md')
        state = self.call('revise', '--agent', 'agent-2')
        self.assertEqual(state['round'], 1)
        self.assertNotIn('editor', state)

    def test_decision_keeps_approvals(self):
        self.approve()
        self.control('decision')
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'READY_TO_EXECUTE')
        self.assertEqual(self.state()['round'], 1)

    def test_logoff_keeps_approvals(self):
        self.approve()
        self.control('logoff')
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'READY_TO_EXECUTE')
        self.assertEqual(self.state()['round'], 1)

    def test_gates_recorded_and_cleared(self):
        self.call('publish', '--agent', 'agent-1', '--status', 'APPROVED', '--text-file', 'message.md',
                  '--gate', 'Metrics need user approval', *self.expected_review())
        self.publish('agent-2', 'APPROVED')
        self.assertEqual(self.call('check', '--agent', 'agent-1')['gates'], ['Metrics need user approval'])
        self.publish('agent-2', 'BLOCKED')
        self.assertEqual(self.state()['gates'], [])
        self.assertIn('only valid with APPROVED', self.run_failing('publish', '--agent', 'agent-1', '--status', 'BLOCKED',
                                                                    '--text-file', 'message.md', '--gate', 'x', *self.expected_review()))

    def test_uppercase_type_and_verdict_field_readable(self):
        self.approve()
        path = self.root / '.reviews/test/agent-2/00000001.json'
        message = json.loads(path.read_text())
        message['type'] = 'REVIEW'
        message['verdict'] = message.pop('status')
        path.write_text(json.dumps(message))
        self.assertEqual(self.call('check', '--agent', 'agent-1')['consensus'], 'READY_TO_EXECUTE')

    def test_duplicate_start_join_preserve_session_heartbeat_idle_and_watcher(self):
        watcher = self.start_watcher(os.getpid())
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        session_path = self.root / '.reviews/test/agent-1.session.json'
        heartbeat_path = self.root / '.reviews/test/agent-1.heartbeat.json'
        idle_path = self.root / '.reviews/test/agent-1.idle.json'
        session = json.loads(session_path.read_text())
        heartbeat = json.loads(heartbeat_path.read_text())
        idle = json.loads(idle_path.read_text())
        for command in ('start', 'join'):
            with self.subTest(command=command):
                error = self.run_failing(command, '--agent', 'agent-1')
                self.assertIn('already active', error.lower())
                self.assertEqual(json.loads(session_path.read_text()), session)
                self.assertEqual(json.loads(idle_path.read_text()), idle)
                current_heartbeat = json.loads(heartbeat_path.read_text())
                self.assertEqual(current_heartbeat['session_id'], heartbeat['session_id'])
                self.assertEqual(current_heartbeat['session_pid'], heartbeat['session_pid'])
                self.assertIsNone(watcher.poll())

    def test_stale_and_inactive_slots_can_be_reclaimed(self):
        self.call('start', '--agent', 'agent-1')
        session_path = self.root / '.reviews/test/agent-1.session.json'
        heartbeat_path = self.root / '.reviews/test/agent-1.heartbeat.json'
        initial = json.loads(session_path.read_text())
        heartbeat = json.loads(heartbeat_path.read_text())
        heartbeat['time'] = time.time() - 121
        heartbeat_path.write_text(json.dumps(heartbeat))
        self.call('join', '--agent', 'agent-1')
        stale_reclaimed = json.loads(session_path.read_text())
        self.assertNotEqual(stale_reclaimed['id'], initial['id'])
        self.call('event', '--agent', 'agent-1', '--kind', 'FINISHED')
        self.call('start', '--agent', 'agent-1')
        self.assertTrue(json.loads(session_path.read_text())['active'])

    def test_simultaneous_slot_claims_allow_only_one_winner(self):
        commands = [('start', 'agent-1'), ('join', 'agent-1')]
        processes = [subprocess.Popen([sys.executable, '-B', str(HELPER), '--run', 'test', command,
                                       '--agent', agent], cwd=self.root, text=True,
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                     for command, agent in commands]
        results = [process.communicate(timeout=5) + (process.returncode,) for process in processes]
        self.assertEqual(sorted(result[2] == 0 for result in results), [False, True])

    def start_watcher(self, pid, *extra, env=None):
        self.call('start', '--agent', 'agent-1')
        watcher = subprocess.Popen([sys.executable, str(HELPER), '--run', 'test',
                                    'heartbeat', '--agent', 'agent-1', '--session-pid', str(pid),
                                    '--interval', '0.02', *extra], cwd=self.root, env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(self.stop_process, watcher)
        path = self.root / '.reviews/test/agent-1.heartbeat.json'
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
                    self.call('event', '--agent', 'agent-1', '--kind', 'FINISHED')
                else:
                    self.call('event', '--agent', 'agent-1', '--kind', 'FINISHED')
                    self.call('start', '--agent', 'agent-1')
                watcher.wait(timeout=3)
                self.assertEqual(watcher.returncode, 0)

    def stub_tmux_env(self, results='[]', first_delay='0'):
        """A fake tmux on PATH records each send-keys call."""
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        stub = bin_dir / 'tmux'
        stub.write_text('''#!/usr/bin/env python3
import json
import os
import sys
import time
if len(sys.argv) > 1 and sys.argv[1] == 'display-message' and '#{session_activity}' in sys.argv:
    activity = os.environ.get('TMUX_ACTIVITY_FILE')
    if activity and os.path.exists(activity):
        print(open(activity, encoding='utf-8').read().strip())
    raise SystemExit(0)
if len(sys.argv) < 2 or sys.argv[1] != 'send-keys':
    raise SystemExit(0)
with open(os.environ['TMUX_LOG'], 'a', encoding='utf-8') as log:
    log.write(' '.join(sys.argv[1:]) + '\\n')
    log.flush()
    call = sum(1 for _ in open(os.environ['TMUX_LOG'], encoding='utf-8'))
if call == 1 and float(os.environ.get('TMUX_FIRST_DELAY', '0')):
    time.sleep(float(os.environ['TMUX_FIRST_DELAY']))
results = json.loads(os.environ.get('TMUX_RESULTS', '[]'))
sys.exit(results[call - 1] if call <= len(results) else 0)
''')
        stub.chmod(0o755)
        return {**os.environ, 'PATH': f'{bin_dir}{os.pathsep}{os.environ["PATH"]}',
                'TMUX_LOG': str(self.root / 'tmux.log'), 'TMUX_RESULTS': results,
                'TMUX_FIRST_DELAY': first_delay}

    def tmux_calls(self):
        log = self.root / 'tmux.log'
        return [line for line in log.read_text().splitlines() if line.startswith('send-keys')] if log.exists() else []

    def wait_for_idle_marker(self, exists):
        marker = self.root / '.reviews/test/agent-1.idle.json'
        deadline = time.monotonic() + 3
        while marker.exists() != exists and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(marker.exists(), exists)

    def test_idle_wake_types_into_own_pane_once(self):
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=self.stub_tmux_env())
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.control('info')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 2 and time.monotonic() < deadline:
            time.sleep(0.01)
        time.sleep(0.2)
        self.assertEqual(self.tmux_calls(), ['send-keys -t %9 -l collaborative-review wake: run=test peer=agent-2 after=0',
                                             'send-keys -t %9 Enter'])
        self.assertFalse((self.root / '.reviews/test/agent-1.idle.json').exists())

    def test_guarded_wake_waits_until_user_stops_typing(self):
        activity = self.root / 'activity'
        activity.write_text(str(int(time.time()) + 3600))
        env = {**self.stub_tmux_env(), 'TMUX_ACTIVITY_FILE': str(activity)}
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=env)
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.control('info')
        time.sleep(0.5)
        self.assertEqual(self.tmux_calls(), [])
        self.wait_for_idle_marker(True)
        activity.write_text(str(int(time.time()) - 60))
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 2 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(self.tmux_calls(), ['send-keys -t %9 -l collaborative-review wake: run=test peer=agent-2 after=0',
                                             'send-keys -t %9 Enter'])
        self.wait_for_idle_marker(False)

    def test_quiet_secs_must_be_finite_and_nonnegative(self):
        self.call('start', '--agent', 'agent-1')
        for value in ('-1', 'inf', 'nan'):
            with self.subTest(value=value):
                result = subprocess.run([sys.executable, str(HELPER), '--run', 'test', 'heartbeat',
                                         '--agent', 'agent-1', '--quiet-secs', value],
                                        cwd=self.root, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('--quiet-secs', result.stderr)

    def test_wake_mode_none_never_types(self):
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', '--wake-mode', 'none', env=self.stub_tmux_env())
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.control('info')
        time.sleep(0.5)
        self.assertEqual(self.tmux_calls(), [])
        self.wait_for_idle_marker(True)

    def test_failed_text_send_retries_before_enter(self):
        env = self.stub_tmux_env('[1, 0, 0]')
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=env)
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.control('info')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 3 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(self.tmux_calls(), [
            'send-keys -t %9 -l collaborative-review wake: run=test peer=agent-2 after=0',
            'send-keys -t %9 -l collaborative-review wake: run=test peer=agent-2 after=0',
            'send-keys -t %9 Enter'])
        self.wait_for_idle_marker(False)

    def test_failed_enter_retries_without_repeating_wake_text(self):
        env = self.stub_tmux_env('[0, 1, 0]')
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=env)
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.control('info')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 3 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(self.tmux_calls(), [
            'send-keys -t %9 -l collaborative-review wake: run=test peer=agent-2 after=0',
            'send-keys -t %9 Enter',
            'send-keys -t %9 Enter'])
        self.wait_for_idle_marker(False)

    def test_successful_wake_preserves_newer_idle_marker(self):
        env = self.stub_tmux_env(first_delay='0.3')
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=env)
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.control('info')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 1 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.call('idle', '--agent', 'agent-1', '--after', '1')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 2 and time.monotonic() < deadline:
            time.sleep(0.01)
        time.sleep(0.05)
        marker = json.loads((self.root / '.reviews/test/agent-1.idle.json').read_text())
        self.assertEqual(marker['after'], 1)

    def test_stale_peer_wake_is_one_shot_until_recovery(self):
        env = self.stub_tmux_env()
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=env)
        self.call('heartbeat', '--agent', 'agent-2')
        heartbeat_path = self.root / '.reviews/test/agent-2.heartbeat.json'
        def set_stale_heartbeat():
            heartbeat = json.loads(heartbeat_path.read_text())
            heartbeat['time'] = time.time() - 121
            temporary = heartbeat_path.with_suffix('.tmp')
            temporary.write_text(json.dumps(heartbeat))
            temporary.replace(heartbeat_path)
        set_stale_heartbeat()
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 2 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(len(self.tmux_calls()), 2)
        self.assertIn('peer=agent-2 unavailable', self.tmux_calls()[0])
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        time.sleep(0.15)
        self.assertEqual(len(self.tmux_calls()), 2)
        self.call('heartbeat', '--agent', 'agent-2')
        outage = self.root / '.reviews/test/agent-1.peer-stale.json'
        deadline = time.monotonic() + 3
        while outage.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertFalse(outage.exists())
        self.control('info')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 4 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(len(self.tmux_calls()), 4)
        self.wait_for_idle_marker(False)

    def test_stale_wake_retries_enter_after_peer_recovers(self):
        env = self.stub_tmux_env('[0, 1, 0]')
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=env)
        self.call('heartbeat', '--agent', 'agent-2')
        heartbeat_path = self.root / '.reviews/test/agent-2.heartbeat.json'
        heartbeat = json.loads(heartbeat_path.read_text())
        heartbeat['time'] = time.time() - 121
        temporary = heartbeat_path.with_suffix('.tmp')
        temporary.write_text(json.dumps(heartbeat))
        temporary.replace(heartbeat_path)
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 1 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.call('heartbeat', '--agent', 'agent-2')
        deadline = time.monotonic() + 3
        while len(self.tmux_calls()) < 3 and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(self.tmux_calls(), [
            'send-keys -t %9 -l collaborative-review wake: run=test peer=agent-2 unavailable after=0',
            'send-keys -t %9 Enter',
            'send-keys -t %9 Enter'])

    def test_restart_cancels_pending_wake_enter(self):
        env = self.stub_tmux_env('[0, 1, 0]')
        watcher = self.start_watcher(os.getpid(), '--tmux-pane', '%9', '--interval', '2', env=env)
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.control('info')
        deadline = time.monotonic() + 3
        marker = self.root / '.reviews/test/agent-1.idle.json'
        while (not marker.exists() or 'wake_progress' not in json.loads(marker.read_text())) and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertIn('wake_progress', json.loads(marker.read_text()))
        self.call('event', '--agent', 'agent-1', '--kind', 'FINISHED')
        self.call('start', '--agent', 'agent-1')
        watcher.wait(timeout=3)
        self.assertFalse(marker.exists())
        time.sleep(0.1)
        self.assertEqual(self.tmux_calls(), [
            'send-keys -t %9 -l collaborative-review wake: run=test peer=agent-2 after=0',
            'send-keys -t %9 Enter'])

    def test_no_wake_without_idle_marker_or_unread(self):
        self.start_watcher(os.getpid(), '--tmux-pane', '%9', env=self.stub_tmux_env())
        self.control('info')
        self.call('idle', '--agent', 'agent-1', '--after', '1')
        time.sleep(0.3)
        self.assertEqual(self.tmux_calls(), [])

    def test_own_command_clears_idle_marker(self):
        self.call('idle', '--agent', 'agent-1', '--after', '0')
        self.call('check', '--agent', 'agent-1')
        self.assertFalse((self.root / '.reviews/test/agent-1.idle.json').exists())

    def test_unreachable_tmux_pane_rejected(self):
        env = self.stub_tmux_env()
        (self.root / 'bin/tmux').write_text('#!/bin/sh\nexit 1\n')
        self.call('start', '--agent', 'agent-1')
        result = subprocess.run([sys.executable, str(HELPER), '--run', 'test', 'heartbeat', '--agent', 'agent-1',
                                 '--session-pid', str(os.getpid()), '--tmux-pane', '%9'],
                                cwd=self.root, text=True, capture_output=True, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('not reachable', result.stderr)

    def test_tmux_pane_requires_tmux(self):
        self.call('start', '--agent', 'agent-1')
        result = subprocess.run([sys.executable, str(HELPER), '--run', 'test', 'heartbeat', '--agent', 'agent-1',
                                 '--session-pid', str(os.getpid()), '--tmux-pane', '%9'],
                                cwd=self.root, text=True, capture_output=True, env={**os.environ, 'PATH': str(self.root)})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('requires tmux', result.stderr)


if __name__ == '__main__':
    unittest.main()
