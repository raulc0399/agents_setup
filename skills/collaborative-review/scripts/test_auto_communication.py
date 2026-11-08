"""Two independent teams exchange reviews and follow up without manual messages."""
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import sys
import tempfile
import time
import unittest

from review_core.aasm_records import prepare_result
from review_core.authorization import record_authorization
from review_core.cli import _coordinator
from review_core.delegation import plan_review
from review_core.discussions import schedule_discussions
from review_core.messages import read_messages
from review_core.snapshots import create_snapshot
from review_core.store import Store


class AutoCommunicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        project = self.root / 'project'
        project.mkdir()
        (project / 'code.py').write_text('value = 1\n')
        self.store = Store(self.root / 'runs' / 'run', create=True)
        snapshot = create_snapshot(project, ['code.py'], self.root / 'runs', 'run')
        for key, value in dict(snapshot=snapshot, project_root=str(project), paths=['code.py'],
                revision=snapshot['revision'], status='ready', budget_seconds=100,
                spent_seconds=0, max_workers=2).items():
            self.store.set_meta(key, value)
        record_authorization(self.store, snapshot['revision'], ['review'], ['code.py'])
        self.coordinator = _coordinator(self.store)
        self.reviews = plan_review(self.store, self.coordinator)

    def tearDown(self):
        self.store.close()
        for path in self.root.rglob('*'):
            if path.is_dir():
                path.chmod(0o755)
        self.temp.cleanup()

    def complete_review(self, worker):
        task = self.coordinator.claim_task(worker)
        body = (f'COV id={worker}-coverage paths=["code.py"] status=reviewed evidence=["code.py:1 value = 1"]\n'
                f'VOTE verdict=APPROVED evidence=["{worker}-coverage"]')
        return self.coordinator.complete_task(task['id'], worker, task['data']['lease_id'],
                                              prepare_result(self.store, worker, body))

    def test_exchange_waits_for_both_teams_and_publishes_once(self):
        first = self.complete_review('reviewer-1')
        self.assertEqual(schedule_discussions(self.store, self.coordinator), [])
        self.assertEqual(read_messages(self.store, 'reviewer-2'), [])
        self.complete_review('reviewer-2')
        tasks = schedule_discussions(self.store, self.coordinator)
        self.assertEqual(len(tasks), 2)
        self.assertEqual({t['data']['payload']['worker'] for t in tasks}, {'reviewer-1', 'reviewer-2'})
        message = read_messages(self.store, 'reviewer-2')[0]
        self.assertEqual(message['data']['operations'][0]['args']['refs'], [first['id']])
        followup = next(task for task in tasks if task['data']['payload']['worker'] == 'reviewer-2')
        context = json.loads(followup['data']['payload']['brief'].split('Source context: ', 1)[1])
        self.assertEqual(context['review']['aasm'], first['data']['result']['aasm'])
        self.assertEqual(message['data']['sender'], 'reviewer-1')
        self.assertEqual([op['op'] for op in message['data']['operations']], ['NOTE', 'Q'])
        self.assertEqual(len(self.store.list_records('coverage')), 2)
        self.assertEqual(len(self.store.list_records('review')), 2)
        before = self.store.events()
        self.assertEqual(schedule_discussions(self.store, self.coordinator), tasks)
        self.assertEqual(before, self.store.events())

    def test_concurrent_pollers_do_not_duplicate_exchange(self):
        self.complete_review('reviewer-1')
        self.complete_review('reviewer-2')
        def schedule(_):
            with Store(self.store.run_dir) as store:
                return schedule_discussions(store, _coordinator(store))
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(schedule, range(2)))
        self.assertEqual(results[0], results[1])
        self.assertEqual(len(results[0]), 2)
        self.assertEqual(len(read_messages(self.store, 'reviewer-1')), 1)

    def test_poll_waits_for_peer_then_returns_only_own_followup(self):
        from review_core.dispatcher import poll_worker
        self.complete_review('reviewer-1')
        def finish_peer():
            time.sleep(.1)
            with Store(self.store.run_dir) as store:
                coordinator = _coordinator(store)
                task = coordinator.claim_task('reviewer-2')
                body = ('COV id=peer paths=["code.py"] status=reviewed evidence=["code.py:1"]\n'
                        'VOTE verdict=APPROVED evidence=["peer"]')
                coordinator.complete_task(task['id'], 'reviewer-2', task['data']['lease_id'],
                                          prepare_result(store, 'reviewer-2', body))
        with ThreadPoolExecutor(max_workers=1) as pool:
            finished = pool.submit(finish_peer)
            result = poll_worker(self.store, 'reviewer-1', wait_seconds=2)
            finished.result()
        self.assertEqual(result['state'], 'ready')
        self.assertEqual(len(result['messages']), 1)
        self.assertEqual(len(result['tasks']), 1)
        self.assertEqual(result['tasks'][0]['worker'], 'reviewer-1')
        self.assertEqual(result['next_after'], result['messages'][0]['sequence'])
        again = poll_worker(self.store, 'reviewer-1', after=result['next_after'], wait_seconds=.01)
        self.assertEqual(again['messages'], [])

    def test_dispatch_cross_checks_before_challenge_and_stops(self):
        from review_core.dispatcher import dispatch_run
        code = '''import json,sys
r=json.load(sys.stdin)
t=r['task']
if t['kind']=='discussion':
    context=json.loads(t['brief'].split('Source context: ',1)[1])
    for q in context['question_records']:
        print('A id=answer-'+q['id']+' question='+q['id']+' text="Compared code.py:1" evidence=["value = 1"]')
    if t['worker']=='reviewer-1' and any(q['id'].startswith('peer-review-') for q in context['question_records']):
        print('Q id=extra-check text="Confirm code.py value in the peer concern team"')
else:
    if t['kind']=='challenge':
        context=json.loads(t['brief'].split('Context: ',1)[1])
        assert len(context['records']['answer']) == 3
    i=t['id']+'-coverage'
    print('COV id='+i+' paths=["code.py"] status=reviewed evidence=["code.py:1 value = 1"]')
    print('VOTE verdict=APPROVED evidence=["'+i+'"]')
'''
        result = dispatch_run(self.store.run_dir, [sys.executable, '-c', code], timeout_seconds=2)
        self.assertEqual(result['state'], 'complete', result)
        self.assertEqual(result['approval']['result'], 'APPROVED', result)
        self.assertEqual(len(result['completed_task_ids']), 6)
        self.assertEqual(len(self.store.list_records('answer')), 3)
        extra = next(task for task in self.store.list_records('task')
                     if task['data']['payload'].get('question_ids') == ['extra-check'])
        self.assertEqual(extra['data']['worker'], 'reviewer-2')
        again = dispatch_run(self.store.run_dir, [sys.executable, '-c', code], timeout_seconds=2)
        self.assertEqual(again['completed_task_ids'], [])

    def test_poll_wait_limit_does_not_shorten_followup_budget(self):
        from review_core.dispatcher import poll_worker
        self.complete_review('reviewer-1')
        self.complete_review('reviewer-2')
        result = poll_worker(self.store, 'reviewer-1', wait_seconds=.01, timeout_seconds=12)
        task = self.store.get_record('task', result['tasks'][0]['task_id'])
        self.assertEqual(task['data']['payload']['timeout_seconds'], 12)

    def test_joining_handoff_hides_peer_results_and_keeps_own_brief(self):
        from review_core.recovery import handoff
        self.complete_review('reviewer-1')
        checkpoint = handoff(self.store, worker='reviewer-2')
        self.assertNotIn('reviewer-1-coverage', json.dumps(checkpoint))
        own = next(task for task in checkpoint['tasks']
                   if task['data']['payload'].get('reviewer') == 'reviewer-2')
        self.assertIn('subagent', own['data']['payload']['brief'])
        peer = next(task for task in checkpoint['tasks'] if task['id'] != own['id'])
        self.assertEqual(peer['data']['status'], 'completed')
        self.assertNotIn('result', peer['data'])

    def test_poll_preserves_blindness_and_times_out_without_work(self):
        from review_core.dispatcher import poll_worker
        self.complete_review('reviewer-1')
        start = time.monotonic()
        result = poll_worker(self.store, 'reviewer-1', wait_seconds=.05)
        self.assertEqual(result['state'], 'waiting')
        self.assertEqual(result['messages'], [])
        self.assertEqual(result['tasks'], [])
        self.assertEqual(result['next_after'], 0)
        self.assertGreaterEqual(time.monotonic() - start, .05)
        self.assertLess(time.monotonic() - start, 1)

    def test_paused_or_invalid_poll_does_not_schedule_exchange(self):
        from review_core.dispatcher import poll_worker
        self.complete_review('reviewer-1')
        self.complete_review('reviewer-2')
        before = self.store.events()
        for kwargs in ({'after': -1}, {'wait_seconds': 31}, {'timeout_seconds': 0}):
            with self.assertRaises(ValueError):
                poll_worker(self.store, 'reviewer-1', **kwargs)
        with self.assertRaises(ValueError):
            poll_worker(self.store, 'unknown')
        self.assertEqual(before, self.store.events())
        self.store.set_meta('status', 'paused')
        before = self.store.events()
        result = poll_worker(self.store, 'reviewer-1')
        self.assertEqual(result['state'], 'blocked')
        self.assertEqual(result['blockers'][0]['code'], 'pause')
        self.assertEqual(before, self.store.events())

    def test_challenge_requires_both_cross_checks(self):
        from review_core.challenge import schedule_challenge
        self.complete_review('reviewer-1')
        self.complete_review('reviewer-2')
        with self.assertRaisesRegex(ValueError, 'cross-checks'):
            schedule_challenge(self.store, self.coordinator)
        schedule_discussions(self.store, self.coordinator)
        task = self.coordinator.claim_task('reviewer-1')
        question = task['data']['payload']['question_ids'][0]
        body = f'A id=checked question={question} text="Checked value" evidence=["code.py:1"]'
        self.coordinator.complete_task(task['id'], 'reviewer-1', task['data']['lease_id'],
                                      prepare_result(self.store, 'reviewer-1', body))
        with self.assertRaisesRegex(ValueError, 'cross-checks'):
            schedule_challenge(self.store, self.coordinator)


if __name__ == '__main__':
    unittest.main()
