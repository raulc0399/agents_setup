"""Subprocess contracts for formal results and negotiated messages."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from review_core.authorization import record_authorization
from review_core.store import Store

SCRIPTS = Path(__file__).resolve().parent
COVERAGE = 'COV id=proof paths=["input.txt"] status=reviewed evidence=["read snapshot"]\n'


class AasmCliTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.project = self.base / 'project'
        self.project.mkdir()
        (self.project / 'input.txt').write_text('first line\nsecond line\n')
        self.root = self.base / 'runs'
        self.run = self.root / 'formal'
        self.create()

    def tearDown(self):
        self.temp.cleanup()

    def create(self):
        self.cli('create', '--project', str(self.project), '--paths', 'input.txt',
                 '--participants', 'alice', 'bob', '--budget-seconds', '120')
        with Store(self.run) as store:
            record_authorization(store, store.get_meta('revision'), ['review'], ['input.txt'])

    def cli(self, *args, input=None, expected=0):
        result = subprocess.run([sys.executable, '-B', str(SCRIPTS / 'review.py'),
            '--root', str(self.root), '--run', self.run.name, *args], cwd=self.project,
            input=input, text=True, capture_output=True, timeout=30)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return json.loads(result.stdout or result.stderr)

    def events(self):
        with Store(self.run, readonly=True) as store:
            return store.events()

    def send(self, body, expected=0, **kwargs):
        args = ['message', 'send', '--worker', 'alice', '--to', 'bob', '--body', '-']
        for key, value in kwargs.items():
            args.extend(['--' + key.replace('_', '-'), value])
        return self.cli(*args, input=body, expected=expected)

    def test_manual_result_binds_worker_task_lease_and_builds_result(self):
        with Store(self.run, readonly=True) as store:
            revision = store.get_meta('revision')
        self.cli('task', 'enqueue', '--data', '-', input=json.dumps({
            'kind': 'review', 'payload': {'revision': revision, 'brief': 'Review input.txt',
                'scope_paths': ['input.txt'], 'timeout_seconds': 30}}))
        task = self.cli('task', 'claim', '--data', '-', input='{"worker":"alice"}')
        args = ['task', 'submit', '--body', '-', '--worker', 'alice',
                '--task-id', task['id'], '--lease-id', task['data']['lease_id']]
        body = COVERAGE + 'VOTE verdict=APPROVED evidence=["proof"]'
        before = self.events()
        for index, invalid in ((5, 'bob'), (7, 'missing'), (9, 'stale')):
            altered = list(args)
            altered[index] = invalid
            self.cli(*altered, input=body, expected=2)
            self.assertEqual(self.events(), before)
        self.cli(*args, input=COVERAGE, expected=2)
        self.assertEqual(self.events(), before)
        body_file = self.base / 'result.aasm'
        body_file.write_text(body)
        args[3] = str(body_file)
        completed = self.cli(*args)
        result = completed['data']['result']
        self.assertEqual(completed['data']['status'], 'completed')
        self.assertEqual(result['dialect_id'], 'aasm-1')
        self.assertEqual(result['verdict'], 'APPROVED')
        self.assertEqual(result['evidence_ids'], ['proof'])
        self.assertEqual([op['op'] for op in result['operations']], ['COV', 'VOTE'])
        with Store(self.run, readonly=True) as store:
            self.assertIsNotNone(store.get_record('coverage', 'proof'))
            self.assertEqual(len(store.list_records('review')), 1)
            self.assertEqual(len(store.list_records('message')), 1)

    def test_peer_messages_apply_once_and_reject_vote_atomically(self):
        body = 'D id=detail at=input.txt:2 claim="Observed detail" fix="Keep evidence" evidence=["snapshot"]'
        message = self.send(body, id='discussion')
        before = self.events()
        self.assertEqual(self.send(body, id='discussion'), message)
        self.assertEqual(self.events(), before)
        self.assertEqual(self.cli('message', 'inbox', '--worker', 'bob'), [message])
        self.assertEqual(self.cli('message', 'inbox', '--worker', 'bob', '--after', str(message['sequence'])), [])
        self.send(COVERAGE + 'VOTE verdict=APPROVED evidence=["proof"]', expected=2)
        self.assertEqual(self.events(), before)
        with Store(self.run, readonly=True) as store:
            self.assertEqual(store.get_record('finding', 'detail')['data']['locations'], ['input.txt:2'])
            self.assertIsNone(store.get_record('coverage', 'proof'))
            self.assertEqual(store.list_records('review'), [])

    def test_invalid_documents_roll_back_all_effects(self):
        invalid = ['NOTE text="hello" unknown=true', 'NOTE text="hello" refs=["missing"]',
                   'D id=bad at=input.txt:3 claim="bad" fix="fix" evidence=["proof"]',
                   'UNKNOWN text="hello"']
        for tail in invalid:
            with self.subTest(tail=tail):
                before = self.events()
                self.send(COVERAGE + tail, expected=2)
                self.assertEqual(self.events(), before)
                with Store(self.run, readonly=True) as store:
                    self.assertIsNone(store.get_record('coverage', 'proof'))
                    self.assertEqual(store.list_records('message'), [])

    def manifest(self):
        return {'name': 'discussion', 'version': 1, 'participants': ['alice', 'bob'],
            'definitions': [{'op': 'SIGNAL', 'meaning': 'Explicit review observation',
                'fields': {'text': {'type': 'text', 'required': True}}}],
            'examples': [{'body': 'SIGNAL text="observed"',
                'expected': [{'op': 'SIGNAL', 'args': {'text': 'observed'}}]}]}

    def agree(self, manifest):
        proposal = self.cli('dialect', 'propose', '--worker', 'alice', '--manifest', '-',
                            input=json.dumps(manifest))
        identity = proposal['id']
        self.cli('dialect', 'ack', '--worker', 'alice', '--id', identity, expected=2)
        self.send('SIGNAL text="observed"', dialect=identity, expected=2)
        failed = self.cli('dialect', 'test', '--worker', 'bob', '--id', identity,
                         '--readback', '-', input='[]')
        self.assertFalse(failed['data']['passed'])
        self.cli('dialect', 'ack', '--worker', 'bob', '--id', identity, expected=2)
        for worker in ('alice', 'bob'):
            tested = self.cli('dialect', 'test', '--worker', worker, '--id', identity,
                             '--readback', '-', input=json.dumps(manifest['examples']))
            self.assertTrue(tested['data']['passed'])
            self.cli('dialect', 'ack', '--worker', worker, '--id', identity)
        self.assertEqual(self.cli('dialect', 'show', '--id', identity)['agreement']['status'], 'accepted')
        return identity

    def test_dialect_readbacks_frozen_meaning_and_export_require_fresh_agreement(self):
        identity = self.agree(self.manifest())
        sent = self.send('SIGNAL text="observed"', dialect=identity)
        altered = self.manifest()
        altered['version'] = 2
        altered['definitions'][0]['meaning'] = 'A different explicit observation'
        self.agree(altered)
        self.assertEqual(self.cli('message', 'inbox', '--worker', 'bob'), [sent])
        with Store(self.run, readonly=True) as store:
            domain = store.list_records('aasm_domain')[0]['data']
            self.assertEqual(domain['definition']['meaning'], 'Explicit review observation')
            self.assertEqual(domain['dialect_id'], identity)
        exported = self.cli('dialect', 'export', '--id', identity)
        self.run = self.root / 'reused'
        self.create()
        self.cli('dialect', 'propose', '--worker', 'alice', '--manifest', '-', input=json.dumps(exported))
        state = self.cli('dialect', 'show', '--id', identity)['agreement']
        self.assertEqual(state['missing_acks'], ['alice', 'bob'])
        self.send('SIGNAL text="observed"', dialect=identity, expected=2)
        self.agree(exported)
        self.assertEqual(self.send('SIGNAL text="observed"', dialect=identity)['data']['operations'],
                         sent['data']['operations'])


if __name__ == '__main__':
    unittest.main()
