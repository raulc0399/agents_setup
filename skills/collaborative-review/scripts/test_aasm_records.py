"""Semantic bridge tests with actual immutable snapshot bytes."""
import tempfile
import unittest
from pathlib import Path
from review_core.aasm import parse_document, expand_operations
from review_core.aasm_records import apply_operations, prepare_result
from review_core.dialects import propose_dialect, test_dialect as readback, acknowledge_dialect
from review_core.snapshots import create_snapshot
from review_core.store import Store


class AasmRecordsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        project = root / 'project'
        project.mkdir()
        (project / 'a.py').write_text('one\ntwo\nthree\n')
        (project / 'b.py').write_text('outside\n')
        snapshot = create_snapshot(project, ['a.py', 'b.py'], root / 'runs', 'run')
        self.revision = snapshot['revision']
        self.store = Store(root / 'state', create=True, metadata={'revision': self.revision, 'snapshot': snapshot})
        self.addCleanup(self.store.close)

    def apply(self, body, author='alice', **kwargs):
        return apply_operations(self.store, author, self.revision, parse_document(body), **kwargs)

    def test_prepare_pure_raw_body_optional_final_vote(self):
        body = 'NOTE text="hello"\n'
        self.assertEqual(body, prepare_result(self.store, 'alice', body)['aasm'])
        result = prepare_result(self.store, 'alice', 'VOTE verdict=APPROVED evidence=["c"]')
        self.assertEqual(['c'], result['evidence_ids'])
        self.assertEqual('APPROVED', result['verdict'])
        self.assertEqual([], self.store.events())
        for body in ['VOTE verdict=BLOCKED evidence=["c"]\nNOTE text=x', 'VOTE verdict=BLOCKED evidence=["c"]\nVOTE verdict=BLOCKED evidence=["c"]']:
            with self.assertRaises(ValueError):
                prepare_result(self.store, 'alice', body)

    def test_all_builtins(self):
        self.apply('\n'.join([
            'B id=b at=a.py:1-2 claim=wrong fix=correct evidence=["checked"]',
            'D id=d at=a.py:2 claim=detail fix=adjust evidence=["read"]',
            'O id=o at=a.py:3 claim=optional fix=adjust evidence=["read"]',
            'FIX id=b evidence=["passes"]', 'REOPEN id=b reason=regression evidence=["fails"]',
            'DISPUTE id=dis finding=b reason=why evidence=["counterexample"]',
            'Q id=q text=which findings=["b"]', 'OPT id=opt title=fix description=change findings=["b"]',
            'A id=a question=q text=chosen evidence=["analysis"] option=opt',
            'COV id=c paths=["a.py"] status=reviewed evidence=["read"] findings=["b"]',
            'NOTE text=observed refs=["q","a","opt","c"]',
            'SRC id=s locator=https://example.org date=2026-11-07 content="captured fact"',
            'F id=f text=claim sources=["s"]', 'GAP id=gap text=missing',
            'ASSUME id=assume text=assumed paths=["a.py"]', 'VERIFY id=assume evidence=["checked"]',
            'IMP proposal=imp text="future improvement"', 'VOTE verdict=BLOCKED evidence=["c"]']), allow_vote=True)
        self.apply('CONFIRM id=f evidence=["independent"] sources=["s"] contradictions=[]\nREC id=rec text=recommend claims=["f"] case=project', author='bob')
        self.assertEqual(['a.py:1-2'], self.store.get_record('finding', 'b')['data']['locations'])
        self.assertEqual('open', self.store.get_record('finding', 'b')['data']['status'])
        self.assertFalse(self.store.get_record('finding', 'd')['data']['blocking'])
        self.assertEqual('answered', self.store.get_record('question', 'q')['data']['status'])
        self.assertEqual('verified', self.store.get_record('claim', 'f')['data']['verification'])
        self.assertEqual('verified', self.store.get_record('assumption', 'assume')['data']['status'])
        self.assertEqual('proposed', self.store.get_record('protocol_proposal', 'imp')['data']['status'])

    def test_reference_failures_roll_back(self):
        invalid = ['NOTE text=x refs=["missing"]', 'Q id=q text=x findings=["missing"]',
                   'A id=a question=missing text=x evidence=["proof"]', 'OPT id=o title=x description=y findings=["missing"]',
                   'COV id=c paths=["a.py"] status=reviewed evidence=["proof"] findings=["missing"]',
                   'F id=f text=x sources=["missing"]', 'CONFIRM id=missing evidence=["proof"]',
                   'REC id=r text=x claims=["missing"] case=x', 'VERIFY id=missing evidence=["proof"]',
                   'FIX id=missing evidence=["proof"]', 'REOPEN id=missing reason=x evidence=["proof"]',
                   'DISPUTE id=q finding=missing reason=x evidence=["proof"]', 'VOTE verdict=APPROVED evidence=["missing"]']
        for body in invalid:
            with self.subTest(body=body), self.assertRaises(ValueError):
                self.apply('IMP proposal=before text=rollback\n' + body, allow_vote=True)
            self.assertEqual([], self.store.events())

    def test_bounds_integrity_unknown_and_stale(self):
        for at in ['a.py:4', 'a.py:1-4', 'missing.py:1']:
            with self.subTest(at=at), self.assertRaises(ValueError):
                self.apply(f'B id=b at={at} claim=x fix=y evidence=["read"]')
        snapshot = self.store.get_meta('snapshot')
        file = Path(snapshot['root']) / 'a.py'
        file.chmod(0o644)
        file.write_text('changed\n')
        with self.assertRaisesRegex(ValueError, 'changed'):
            self.apply('B id=b at=a.py:1 claim=x fix=y evidence=["read"]')
        with self.assertRaises(ValueError):
            apply_operations(self.store, 'alice', self.revision, [{'op': 'RUN', 'args': {}}])
        with self.assertRaises(ValueError):
            apply_operations(self.store, 'alice', 'old', parse_document('NOTE text=x'))
        self.assertEqual([], self.store.events())

    def test_vote_authority_and_scope_on_mutation_targets(self):
        self.apply('B id=b at=a.py:1 claim=x fix=y evidence=["read"]\nASSUME id=a text=x paths=["a.py"]\nCOV id=c paths=["a.py"] status=reviewed evidence=["read"]')
        with self.assertRaisesRegex(ValueError, 'task results'):
            self.apply('VOTE verdict=APPROVED evidence=["c"]')
        with self.assertRaises(ValueError):
            self.apply('VOTE verdict=APPROVED evidence=["c"]', author='bob', allow_vote=True)
        self.store.put_record('task', 't', {'worker': 'alice', 'payload': {'revision': self.revision, 'scope_paths': ['other.py']}})
        for body in ['FIX id=b evidence=["pass"]', 'REOPEN id=b reason=x evidence=["fail"]', 'VERIFY id=a evidence=["check"]', 'VOTE verdict=APPROVED evidence=["c"]']:
            with self.subTest(body=body), self.assertRaisesRegex(ValueError, 'task scope'):
                self.apply(body, task_id='t', allow_vote=True)
        row = self.store.get_record('task', 't')
        row['data']['payload']['scope_paths'] = ['.']
        self.store.put_record('task', 't', row['data'])
        self.apply('VERIFY id=a evidence=["check"]', task_id='t')

    def test_linked_finding_scope_on_answers_and_references(self):
        self.apply('B id=outside at=b.py:1 claim=x fix=y evidence=["read"]\nQ id=q text=why findings=["outside"]\nOPT id=o title=x description=y findings=["outside"]\nQ id=unlinked text=why')
        self.store.put_record('task', 't', {'worker': 'alice', 'payload': {'revision': self.revision, 'scope_paths': ['a.py']}})
        for body in ['A id=a question=q text=x evidence=["proof"]',
                     'A id=a question=unlinked text=x evidence=["proof"] option=o',
                     'Q id=q2 text=x findings=["outside"]',
                     'OPT id=o2 title=x description=y findings=["outside"]',
                     'COV id=c paths=["a.py"] status=reviewed evidence=["read"] findings=["outside"]']:
            with self.subTest(body=body), self.assertRaisesRegex(ValueError, 'task scope'):
                self.apply(body, task_id='t')
        self.assertEqual('unanswered', self.store.get_record('question', 'q')['data']['status'])

    def accept(self, version, meaning, alias=False):
        definition = {'op': 'OBS', 'meaning': meaning, 'fields': {'text': {'type': 'text', 'required': True}}}
        if alias:
            definition['expands_to'] = {'op': 'NOTE', 'args': {'text': {'field': 'text'}}}
        definitions = [definition]
        body = 'OBS text=observation'
        examples = [{'body': body, 'expected': expand_operations(parse_document(body, definitions), definitions)}]
        row = propose_dialect(self.store, 'alice', 'observations', version, ['alice', 'bob'], definitions, examples)
        for participant in ['alice', 'bob']:
            readback(self.store, row['id'], participant, examples)
            acknowledge_dialect(self.store, row['id'], participant)
        return row['id']

    def test_extensions_freeze_explicit_meaning(self):
        first = self.accept(1, 'Domain observation')
        self.accept(2, 'Alternate observation')
        prepared = prepare_result(self.store, 'alice', 'OBS text=observation', first)
        apply_operations(self.store, 'alice', self.revision, prepared['operations'], dialect_id=first)
        frozen = self.store.list_records('aasm_domain')[0]['data']
        self.assertEqual('Domain observation', frozen['definition']['meaning'])
        self.assertEqual(first, frozen['dialect_id'])
        with self.assertRaises(ValueError):
            apply_operations(self.store, 'alice', self.revision, prepared['operations'])
        alias = self.accept(3, 'Alias observation', alias=True)
        prepared = prepare_result(self.store, 'alice', 'OBS text=observation', alias)
        self.assertEqual('NOTE', prepared['operations'][0]['op'])
        apply_operations(self.store, 'alice', self.revision, prepared['operations'], dialect_id=alias)
        self.assertEqual(1, len(self.store.list_records('aasm_note')))


if __name__ == '__main__':
    unittest.main()
