"""Dispatch real worker subprocesses promptly without inventing authorization."""
import sys
import time
import unittest

import test_runner


class DispatcherTests(unittest.TestCase):
    setUp = test_runner.RunnerTests.setUp
    tearDown = test_runner.RunnerTests.tearDown

    def test_dispatch_drains_two_workers_concurrently(self):
        from review_core.dispatcher import dispatch_run
        revision = self.store.get_meta('revision')
        self.store.set_meta('challenge_required', False)
        second = self.coordinator.enqueue_task('review', dict(revision=revision,
            scope_paths=['code.py'], brief='Independent review', reviewer='reviewer-2', timeout_seconds=10))
        code = '''import json,sys,time
r=json.load(sys.stdin)
time.sleep(.3)
w=r['task']['worker']
print('COV id='+w+'-coverage paths=["code.py"] status=reviewed evidence=["observed"]')
print('VOTE verdict=APPROVED evidence=["'+w+'-coverage"]')
'''
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', code], timeout_seconds=2)
        self.assertEqual(result['state'], 'complete')
        self.assertEqual(set(result['completed_task_ids']), {self.task['id'], second['id']})
        first_data = self.store.get_record('task', self.task['id'])['data']
        second_data = self.store.get_record('task', second['id'])['data']
        self.assertLess(max(first_data['started_at'], second_data['started_at']),
                        min(first_data['finished_at'], second_data['finished_at']))

    def test_dispatch_reports_missing_grant_without_waiting_or_mutation(self):
        from review_core.dispatcher import dispatch_run
        self.store.set_meta('revision', 'unapproved')
        manifest = self.store.get_meta('snapshot')
        manifest['revision'] = 'unapproved'
        self.store.set_meta('snapshot', manifest)
        task = self.coordinator.enqueue_task('review', dict(revision='unapproved',
            scope_paths=['code.py'], brief='Review', timeout_seconds=10))
        start = time.monotonic()
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise Exception()'])
        self.assertLess(time.monotonic() - start, 1)
        self.assertEqual(result['state'], 'blocked')
        self.assertIn('authorization', [b['code'] for b in result['blockers']])
        self.assertEqual(self.store.get_record('task', task['id'])['data']['attempt'], 0)

    def test_dispatch_reports_failed_task_without_retry(self):
        from review_core.dispatcher import dispatch_run
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise SystemExit(3)'])
        self.assertEqual(result['state'], 'blocked')
        self.assertIn('failed', [b['code'] for b in result['blockers']])
        self.assertEqual(self.store.get_record('task', self.task['id'])['data']['attempt'], 1)

    def test_expired_external_lease_requires_recovery(self):
        from review_core.dispatcher import dispatch_run
        claimed = self.coordinator.claim_task('reviewer-1')
        claimed['data']['lease_expires'] = time.time() - 1
        self.store.put_record('task', self.task['id'], claimed['data'])
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise Exception()'])
        self.assertEqual(result['state'], 'blocked')
        self.assertIn('recovery', [b['code'] for b in result['blockers']])
        self.assertEqual(self.store.get_record('task', self.task['id'])['data']['status'], 'running')

    def test_second_controller_cannot_dispatch(self):
        import fcntl
        from review_core.dispatcher import dispatch_run
        with (self.store.run_dir / 'dispatch.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise Exception()'])
        self.assertEqual(result['state'], 'blocked')
        self.assertEqual(result['blockers'][0]['code'], 'dispatcher')
        self.assertEqual(self.store.get_record('task', self.task['id'])['data']['attempt'], 0)

    def test_discussion_scheduling_error_becomes_explicit_blocker(self):
        from unittest.mock import patch
        from review_core.dispatcher import dispatch_run
        with patch('review_core.discussions.schedule_discussions', side_effect=ValueError('Message question has not been applied: q')):
            result = dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise SystemExit(3)'])
        self.assertEqual(result['state'], 'blocked')
        self.assertIn('discussion', [blocker['code'] for blocker in result['blockers']])

    def test_preclaim_worker_error_is_not_retried(self):
        from unittest.mock import patch
        from review_core.dispatcher import dispatch_run
        calls = []
        def reject(*args):
            calls.append(args)
            if len(calls) > 1:
                from review_core.store import Store
                from review_core.coordinator import Coordinator
                with Store(self.store.run_dir) as separate:
                    Coordinator(separate).cancel_task(self.task['id'], 'Stop erroneous automatic retry')
            raise ValueError('Dialect not accepted')
        with patch('review_core.dispatcher._worker', side_effect=reject):
            result = dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise Exception()'])
        self.assertEqual(len(calls), 1)
        self.assertEqual(result['state'], 'blocked')

    def test_completion_notifications_are_distinct_per_recipient(self):
        from review_core.dispatcher import dispatch_run
        dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise SystemExit(3)'], recipient='first')
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', 'raise SystemExit(3)'], recipient='second')
        self.assertEqual(result['state'], 'blocked')

    def test_changed_running_challenge_drains_after_successful_replacement(self):
        import threading
        from review_core.coordinator import Coordinator
        from review_core.dispatcher import dispatch_run
        from review_core.store import Store
        revision = self.store.get_meta('revision')
        second = self.coordinator.enqueue_task('review', dict(revision=revision,
            scope_paths=['code.py'], brief='Second review', reviewer='reviewer-2', timeout_seconds=10))
        self.store.put_record('review_plan', 'plan', dict(request=dict(revision=revision),
            task_ids=[self.task['id'], second['id']], challenge_budget_seconds=20))
        code = '''import json,sys,time
r=json.load(sys.stdin)
time.sleep(.2)
i=r['task']['id']+'-coverage'
print('COV id='+i+' paths=["code.py"] status=reviewed evidence=["observed"]')
print('VOTE verdict=APPROVED evidence=["'+i+'"]')
'''
        initial = dispatch_run(self.store.run_dir, [sys.executable, '-c', code])
        self.assertEqual(initial['state'], 'complete')
        self.store.set_meta('integration_surfaces', ['First changed interface'])
        changed = threading.Event()
        def update_running_context():
            with Store(self.store.run_dir) as observer:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    if any(task['data']['kind'] == 'challenge' and task['data']['status'] == 'running'
                           for task in observer.list_records('task')):
                        observer.set_meta('integration_surfaces', ['Second changed interface'])
                        changed.set()
                        return
                    time.sleep(.01)
        thread = threading.Thread(target=update_running_context)
        thread.start()
        try:
            result = dispatch_run(self.store.run_dir, [sys.executable, '-c', code])
        finally:
            thread.join()
        self.assertTrue(changed.is_set())
        self.assertEqual(result['blockers'], [])
        self.assertEqual(result['state'], 'complete')
        self.assertEqual(result['approval']['result'], 'APPROVED')
        self.assertTrue(any(task['data']['status'] == 'failed' for task in self.store.list_records('task')))

    def test_superseded_discussion_cancellation_does_not_block_challenge(self):
        from review_core.dispatcher import dispatch_run
        revision = self.store.get_meta('revision')
        second = self.coordinator.enqueue_task('review', dict(revision=revision,
            scope_paths=['code.py'], brief='Second review', reviewer='reviewer-2', timeout_seconds=10))
        self.store.put_record('review_plan', 'plan', dict(request=dict(revision=revision),
            task_ids=[self.task['id'], second['id']], challenge_budget_seconds=20))
        obsolete = self.coordinator.enqueue_task('discussion', dict(revision=revision,
            scope_paths=['code.py'], brief='Already answered', worker='reviewer-2', timeout_seconds=10))
        cancelled = self.coordinator.cancel_task(obsolete['id'], 'Question answered externally')
        cancelled['data']['discussion_superseded'] = True
        self.store.put_record('task', obsolete['id'], cancelled['data'])
        code = '''import json,sys
r=json.load(sys.stdin)
i=r['task']['id']+'-coverage'
print('COV id='+i+' paths=["code.py"] status=reviewed evidence=["observed"]')
print('VOTE verdict=APPROVED evidence=["'+i+'"]')
'''
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', code])
        self.assertEqual(result['state'], 'complete')
        self.assertEqual(result['approval']['result'], 'APPROVED')
        self.assertTrue(any(task['data']['kind'] == 'challenge' for task in self.store.list_records('task')))

    def test_current_challenge_failure_remains_blocked(self):
        from review_core.dispatcher import dispatch_run
        revision = self.store.get_meta('revision')
        second = self.coordinator.enqueue_task('review', dict(revision=revision,
            scope_paths=['code.py'], brief='Second review', reviewer='reviewer-2', timeout_seconds=10))
        self.store.put_record('review_plan', 'plan', dict(request=dict(revision=revision),
            task_ids=[self.task['id'], second['id']], challenge_budget_seconds=20))
        code = '''import json,sys
r=json.load(sys.stdin)
if r['task']['kind']=='challenge':
    raise SystemExit(3)
i=r['task']['id']+'-coverage'
print('COV id='+i+' paths=["code.py"] status=reviewed evidence=["observed"]')
print('VOTE verdict=APPROVED evidence=["'+i+'"]')
'''
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', code])
        self.assertEqual(result['state'], 'blocked')
        challenge = next(task for task in self.store.list_records('task') if task['data']['kind'] == 'challenge')
        self.assertEqual(challenge['data']['status'], 'failed')
        self.assertEqual(challenge['data']['attempt'], 1)
        self.assertIn(challenge['id'], [blocker['task_id'] for blocker in result['blockers']])

    def test_worker_stderr_trailing_newline_preserves_structured_failure(self):
        from review_core.dispatcher import dispatch_run
        from review_core.notifications import read_notifications
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c',
            'import sys; sys.stderr.write("Observed worker failure\\n"); raise SystemExit(3)'])
        self.assertEqual(result['state'], 'blocked')
        failure = next(blocker for blocker in result['blockers'] if blocker['code'] == 'failed')
        self.assertTrue(failure['reason'].endswith('Observed worker failure\n'))
        notifications = read_notifications(self.store, 'coordinator')['notifications']
        self.assertEqual(len(notifications), 1)
        summary = notifications[0]['data']['summary']
        self.assertEqual(summary, summary.strip())
