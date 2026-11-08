"""Quiet dispatch, review progress and conversation delivery across real CLI processes."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from review_core.authorization import record_authorization
from review_core.store import Store


ENTRY = Path(__file__).resolve().parent / 'review.py'
REVIEW_ADAPTER = '''import json, os, pathlib, subprocess, sys, time
request = json.load(sys.stdin)
task = request['task']
run = pathlib.Path(os.environ['REVIEW_RUN_DIR'])
if task['kind'] == 'review':
    (run / task['worker']).write_text('running')
    peer = 'reviewer-2' if task['worker'] == 'reviewer-1' else 'reviewer-1'
    until = time.monotonic() + 3
    while not (run / peer).exists():
        if time.monotonic() >= until:
            raise RuntimeError('Other review did not start concurrently')
        time.sleep(0.01)
    progress = {'task_id': task['id'], 'worker': task['worker'], 'lease_id': os.environ['LEASE_ID'],
                'summary': 'Read the captured input and checked both lines', 'evidence': ['input.txt:1-2']}
    subprocess.run([sys.executable, '-B', str(run / 'protocol' / 'review.py'), '--root', str(run.parent),
                    '--run', run.name, 'task', 'progress', '--data', '-'], input=json.dumps(progress),
                   text=True, stdout=subprocess.DEVNULL, check=True)
if task['kind'] == 'discussion':
    context = json.loads(task['brief'].split('Source context: ', 1)[1])
    for question in context['question_records']:
        print('A id=answer-' + question['id'] + ' question=' + question['id'] +
              ' text="Compared both captured lines" evidence=["input.txt:1-2"]')
    raise SystemExit(0)
proof = 'coverage-' + task['id']
print('COV id=' + proof + ' paths=["input.txt"] status=reviewed evidence=["Read both captured lines"]')
print('VOTE verdict=APPROVED evidence=["' + proof + '"]')
'''


class FlowCliTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.project = self.base / 'project'
        self.project.mkdir()
        (self.project / 'input.txt').write_text('first\nsecond\n')
        self.root = self.base / 'runs'
        self.run = self.root / 'flow'
        self.adapter = self.base / 'adapter.py'
        self.adapter.write_text(REVIEW_ADAPTER)
        self.cli('create', '--project', str(self.project), '--paths', 'input.txt',
                 '--participants', 'alice', 'bob', '--budget-seconds', '90', '--max-workers', '2')

    def cli(self, *args, input=None, expected=0):
        result = subprocess.run([sys.executable, '-B', str(ENTRY), '--root', str(self.root),
            '--run', self.run.name, *args], input=input, text=True, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return json.loads(result.stdout or result.stderr)

    def grant(self):
        with Store(self.run) as store:
            record_authorization(store, store.get_meta('revision'), ['review'], ['input.txt'])

    def dispatch(self, expected=0):
        return self.cli('dispatch', '--timeout-seconds', '5', '--recipient', 'main-chat',
                        '--', sys.executable, '-B', str(self.adapter), expected=expected)

    def test_dispatch_names_missing_permission_without_starting_workers(self):
        self.cli('plan')
        result = self.dispatch(expected=1)
        self.assertEqual(result['state'], 'blocked')
        self.assertTrue(any(reason['code'] == 'authorization' for reason in result['blockers']))
        status = self.cli('status', '--json')
        self.assertEqual(status['active_tasks'], [])
        self.assertEqual(len(status['blocked_tasks']), 2)
        self.assertFalse((self.run / 'reviewer-1').exists())

    def test_concurrent_review_and_challenge_complete_during_presentation_hold(self):
        self.grant()
        self.cli('plan')
        self.cli('discussion', 'begin', '--recipient', 'main-chat')
        result = self.dispatch()
        self.assertEqual(result['state'], 'complete')
        self.assertEqual(result['approval']['result'], 'APPROVED')
        self.assertEqual(len(result['completed_task_ids']), 5)
        self.assertEqual(self.cli('notifications', '--recipient', 'main-chat'),
                         {'deferred': True, 'notifications': []})
        with Store(self.run, readonly=True) as store:
            reviews = [task for task in store.list_records('task') if task['data']['kind'] == 'review']
            self.assertTrue(all(task['data']['progress']['evidence'] == ['input.txt:1-2'] for task in reviews))
        self.cli('discussion', 'end', '--recipient', 'main-chat')
        updates = self.cli('notifications', '--recipient', 'main-chat')['notifications']
        self.assertEqual(len(updates), 1)
        self.assertNotIn('VOTE', updates[0]['data']['summary'])
        self.assertEqual(self.cli('notifications', '--recipient', 'main-chat',
                         '--after', str(updates[0]['sequence']))['notifications'], [])

    def test_question_dispatch_delivers_one_peer_reply_without_note_loops(self):
        self.grant()
        self.cli('message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-',
                 input='Q id=question text="Which line contains second?"')
        self.adapter.write_text('''import json, sys
request = json.load(sys.stdin)
assert request['task']['kind'] == 'discussion'
print('A id=answer question=question text="input.txt line 2" evidence=["Read captured input.txt:2"]')
''')
        result = self.dispatch()
        self.assertEqual(result['state'], 'complete')
        self.assertEqual(len(result['completed_task_ids']), 1)
        inbox = self.cli('message', 'inbox', '--worker', 'alice')
        self.assertEqual(len(inbox), 1)
        self.assertEqual(inbox[0]['data']['operations'][0]['op'], 'A')
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.get_record('question', 'question')['data']['status'], 'answered')
            self.assertEqual(len(store.list_records('task')), 1)
        self.assertEqual(self.dispatch()['completed_task_ids'], [])
        self.assertEqual(len(self.cli('notifications', '--recipient', 'main-chat')['notifications']), 1)

    def test_discussion_rejects_empty_acknowledgement_without_losing_question(self):
        self.grant()
        self.cli('message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-',
                 input='Q id=question text="Which line?"')
        self.adapter.write_text('import json,sys; json.load(sys.stdin); print("NOTE text=acknowledged")\n')
        result = self.dispatch(expected=1)
        self.assertEqual(result['state'], 'blocked')
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.get_record('question', 'question')['data']['status'], 'unanswered')
            self.assertEqual(store.list_records('aasm_note'), [])
        self.assertEqual(self.cli('message', 'inbox', '--worker', 'alice'), [])

    def test_answer_arriving_during_work_supersedes_response_without_effects(self):
        self.grant()
        self.cli('message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-',
                 input='Q id=question text="Which line?"')
        self.adapter.write_text('''import json, os, pathlib, subprocess, sys
request = json.load(sys.stdin)
run = pathlib.Path(os.environ['REVIEW_RUN_DIR'])
subprocess.run([sys.executable, '-B', str(run / 'protocol' / 'review.py'), '--root', str(run.parent),
    '--run', run.name, 'message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-'],
    input='A id=earlier question=question text="Already resolved" evidence=["checked"]',
    text=True, stdout=subprocess.DEVNULL, check=True)
print('A id=obsolete question=question text="Stale answer" evidence=["checked earlier"]')
''')
        self.assertEqual(self.dispatch()['state'], 'complete')
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.get_record('question', 'question')['data']['answer_id'], 'earlier')
            self.assertIsNone(store.get_record('answer', 'obsolete'))
            task = store.list_records('task')[0]['data']
            self.assertTrue(task['discussion_superseded'])
            self.assertEqual(task['result_disposition'], 'superseded')
            self.assertEqual(task['status'], task['attempts'][-1]['status'])

    def test_partial_answer_race_replaces_only_remaining_assignment(self):
        self.grant()
        self.cli('message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-',
                 input='Q id=first text="First?"\nQ id=second text="Second?"')
        self.adapter.write_text('''import json, os, pathlib, subprocess, sys
request = json.load(sys.stdin)
run = pathlib.Path(os.environ['REVIEW_RUN_DIR'])
marker = run / 'answered-first'
if not marker.exists():
    marker.write_text('answered')
    subprocess.run([sys.executable, '-B', str(run / 'protocol' / 'review.py'), '--root', str(run.parent),
        '--run', run.name, 'message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-'],
        input='A id=earlier question=first text="Already resolved" evidence=["checked"]',
        text=True, stdout=subprocess.DEVNULL, check=True)
    print('A id=obsolete question=first text="Stale answer" evidence=["checked"]')
else:
    print('A id=remaining question=second text="Resolved remainder" evidence=["checked"]')
''')
        self.assertEqual(self.dispatch()['state'], 'complete')
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.get_record('question', 'first')['data']['answer_id'], 'earlier')
            self.assertEqual(store.get_record('question', 'second')['data']['answer_id'], 'remaining')
            tasks = store.list_records('task')
            self.assertEqual(len(tasks), 2)
            finished = [task for task in tasks if task['data']['status'] == 'completed']
            self.assertEqual(finished[0]['data']['payload']['question_ids'], ['second'])

    def test_gap_reply_stops_with_unanswered_question_instead_of_peer_loop(self):
        self.grant()
        self.cli('message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-',
                 input='Q id=question text="Which unavailable evidence?"')
        self.adapter.write_text('import json,sys; json.load(sys.stdin); print(\'GAP id=missing text="Evidence unavailable"\')\n')
        result = self.dispatch()
        self.assertEqual(result['state'], 'complete')
        self.assertEqual(result['approval']['result'], 'BLOCKED')
        with Store(self.run, readonly=True) as store:
            self.assertEqual(len(store.list_records('task')), 1)
            self.assertEqual(store.get_record('question', 'question')['data']['status'], 'unanswered')

    def test_question_edit_during_work_discards_answer_to_old_wording(self):
        self.grant()
        self.cli('message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-',
                 input='Q id=question text="Which first line?"')
        self.adapter.write_text('''import json, os, pathlib, subprocess, sys
request = json.load(sys.stdin)
run = pathlib.Path(os.environ['REVIEW_RUN_DIR'])
marker = run / 'question-edited'
if not marker.exists():
    marker.write_text('edited')
    subprocess.run([sys.executable, '-B', str(run / 'protocol' / 'review.py'), '--root', str(run.parent),
        '--run', run.name, 'record', 'question-update', '--data', '-'],
        input=json.dumps({'question_id': 'question', 'actor': 'alice', 'text': 'Which second line?'}),
        text=True, stdout=subprocess.DEVNULL, check=True)
    print('A id=obsolete question=question text="First line" evidence=["old wording"]')
else:
    assert 'Which second line?' in request['task']['brief']
    print('A id=current question=question text="Second line" evidence=["current wording"]')
''')
        self.assertEqual(self.dispatch()['state'], 'complete')
        with Store(self.run, readonly=True) as store:
            self.assertIsNone(store.get_record('answer', 'obsolete'))
            answer = store.get_record('answer', 'current')['data']
            self.assertEqual(answer['question_version'], 2)
            tasks = store.list_records('task')
            self.assertEqual(len(tasks), 2)
            self.assertEqual({task['data']['status'] for task in tasks}, {'cancelled', 'completed'})


if __name__ == '__main__':
    unittest.main()
