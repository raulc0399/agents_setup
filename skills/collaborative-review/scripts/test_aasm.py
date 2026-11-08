"""Conformance tests for strict AASM parsing and declarative expansion."""
import unittest

from review_core.aasm import (
    BUILTIN_OPERATIONS, MAX_COMMANDS, MAX_DOCUMENT_BYTES, MAX_LINE_BYTES,
    expand_operations, format_document, parse_document, validate_definitions,
)


class AasmTests(unittest.TestCase):
    def test_all_operations_and_defaults_roundtrip(self):
        examples = '''B id=b at=src/a.py:1-2 claim="Wrong result" fix="Correct result" evidence=["test fails"]
D id=d at=src/a.py:1 claim=detail fix=correct evidence=["observation"]
O id=o at=src/a.py:1 claim=optional fix=improve evidence=["observation"]
FIX id=b evidence=["test passes"]
REOPEN id=b reason=regression evidence=["test fails"]
DISPUTE id=q finding=b reason=uncertain evidence=["counterexample"]
Q id=q text="Which fix?"
A id=a question=q text="Use fix" evidence=["proof"]
OPT id=opt title=Fix description="Correct result"
COV id=c paths=["src/a.py"] status=reviewed evidence=["reviewed line 1"]
VOTE verdict=APPROVED evidence=["c"]
NOTE text="Retest required"
SRC id=s locator="https://example.org" date="2026-01-01" content="Source facts"
F id=f text="Research claim" sources=["s"]
CONFIRM id=f evidence=["independent proof"]
GAP id=g text="Missing coverage"
REC id=r text=Recommendation claims=["f"] case="Decision basis"
ASSUME id=x text="Input is valid" paths=["src/a.py"]
VERIFY id=x evidence=["checked"]
IMP proposal=p text="Future improvement"
POINT id=point text="Disputed retry behavior" paths=["src/a.py"] evidence=["trace"]
PROPOSE id=point digest=current text="Retry once" evidence=["trace"]
AGREE id=point digest=current evidence=["independently checked"]
CONTEST id=point digest=current text="Retry loses input" evidence=["failure trace"]
RETIRE kind=claim id=f evidence=["Refuted by later evidence"]'''
        ast = parse_document(examples)
        self.assertEqual(set(BUILTIN_OPERATIONS), {item['op'] for item in ast})
        self.assertEqual('high', ast[0]['args']['severity'])
        self.assertTrue(ast[6]['args']['blocking'])
        self.assertEqual([], ast[11]['args']['refs'])
        self.assertEqual(ast, parse_document(format_document(ast)))
        self.assertEqual(format_document(ast), format_document(parse_document(format_document(ast))))

    def test_reject_malformed_duplicate_unknown_and_implicit_input(self):
        invalid = [
            'Some prose', ' NOTE text=x', '# comment', 'RUN command=x',
            'NOTE text=x text=y', 'NOTE text=x unknown=y', 'NOTE',
            'NOTE text="x"junk', 'NOTE text=x trailing', 'NOTE text=hello world',
            'NOTE text=["x",]', 'NOTE text={"a":1,"a":2}', 'NOTE text=NaN',
            'NOTE text=Infinity', 'NOTE text=-Infinity', 'NOTE text=',
            'NOTE text="unterminated', 'NOTE text=x\\y', 'NOTE text=""',
            'Q id=q text=x blocking=1', 'FIX id=f evidence=[]',
            'VOTE verdict=approved evidence=["c"]', 'NOTE text=x refs=[true]',
            'FIX id="bad id" evidence=["x"]', 'NOTE text=null',
        ]
        for text in invalid:
            with self.subTest(text=text), self.assertRaisesRegex(ValueError, 'line 1:'):
                parse_document(text)
        with self.assertRaisesRegex(ValueError, 'line 3:'):
            parse_document('NOTE text=x\n\nUNKNOWN x=y')

    def test_paths_locations_and_typed_numbers(self):
        for location in ('/a:1', '../a:1', 'a//b:1', 'a/../b:1', 'a:0', 'a:01', 'a:3-2', 'a:1-0', 'a:1x', 'C:\\a:1'):
            with self.subTest(location=location), self.assertRaises(ValueError):
                parse_document('B id=f at=' + repr(location).replace("'", '"') + ' claim=x fix=y evidence=["z"]')
        definition = [dict(op='COUNT', meaning='Count observations.', fields={'count': dict(type='integer', required=True)})]
        self.assertEqual(2, parse_document('COUNT count=2', definition)[0]['args']['count'])
        for value in ('true', '0', '-1', '2.0', '1e999'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse_document('COUNT count=' + value, definition)
        self.assertEqual('123abc', parse_document('FIX id=123abc evidence=["x"]')[0]['args']['id'])

    def test_extension_expansion_preserves_original_and_validates_runtime(self):
        definitions = [dict(op='OBSERVE', meaning='Record an informational observation.', fields={'message': dict(type='text', required=True)}, expands_to=dict(op='NOTE', args={'text': {'field': 'message'}})), dict(op='DOMAIN', meaning='A typed domain observation without control effects.', fields={'paths': dict(type='paths', required=True)})]
        ast = parse_document('OBSERVE message="A fact"\nDOMAIN paths=["a.py"]', definitions)
        self.assertEqual('OBSERVE', ast[0]['op'])
        expanded = expand_operations(ast, definitions)
        self.assertEqual(dict(op='NOTE', args=dict(text='A fact', refs=[])), expanded[0])
        self.assertEqual(ast[1], expanded[1])
        self.assertEqual(ast, parse_document(format_document(ast, definitions), definitions))
        self.assertEqual(definitions, validate_definitions(definitions))
        alias = [dict(op='RESOLVE', meaning='Resolve finding.', fields={'id': dict(type='id', required=True), 'proof': dict(type='strings', required=True)}, expands_to=dict(op='FIX', args={'id': {'field': 'id'}, 'evidence': {'field': 'proof'}}))]
        with self.assertRaises(ValueError):
            expand_operations(parse_document('RESOLVE id=f proof=[]', alias), alias)

    def test_reject_invalid_definitions(self):
        valid = dict(op='CUSTOM', meaning='Explicit meaning.', fields={'x': dict(type='text', required=True)})
        invalid = [None, {}, [dict(valid, fields={'x': {'type': [], 'required': True}})], [dict(valid, expands_to={'op': [], 'args': {}})], [dict(valid, op='NOTE')], [valid, valid], [dict(valid, meaning='')], [dict(valid, op='lower')], [dict(valid, fields={'x': {'type': 'shell', 'required': True}})], [dict(valid, fields={'x': {'type': 'text', 'required': 1}})], [dict(valid, fields={'x': {'type': 'enum', 'required': True, 'enum': ['a', 'a']}})], [dict(valid, expands_to={'op': 'CUSTOM', 'args': {}})], [dict(valid, expands_to={'op': 'NOTE', 'args': {}})], [dict(valid, expands_to={'op': 'NOTE', 'args': {'text': {'field': 'missing'}}})], [dict(valid, expands_to={'op': 'NOTE', 'args': {'text': {'value': ''}}})]]
        for definitions in invalid:
            with self.subTest(definitions=definitions), self.assertRaises(ValueError):
                validate_definitions(definitions)
        with self.assertRaises(ValueError):
            parse_document('NOTE text=x', {})

    def test_decoded_unicode_must_be_valid_utf8(self):
        for text in ('NOTE text="\\ud800"', 'NOTE text="ok" refs=["\\udfff"]'):
            with self.subTest(text=text), self.assertRaisesRegex(ValueError, 'line 1:.*UTF-8'):
                parse_document(text)
        with self.assertRaisesRegex(ValueError, 'line 2:.*UTF-8'):
            parse_document('NOTE text=ok\nNOTE text="' + chr(0xd800) + '"')
        ast = parse_document('NOTE text="\\ud83c\\udf05"')
        self.assertEqual(ast, parse_document(format_document(ast)))

    def test_normalized_size_boundaries_roundtrip(self):
        overhead = len('NOTE text="" refs=[]')
        body = 'NOTE text=' + 'x' * (MAX_LINE_BYTES - overhead)
        ast = parse_document(body)
        self.assertEqual(MAX_LINE_BYTES, len(format_document(ast).encode('utf-8')))
        self.assertEqual(ast, expand_operations(ast))
        with self.assertRaisesRegex(ValueError, 'line 1:.*normalized line'):
            parse_document(body + 'x')
        raw_limit = 'NOTE text=' + 'x' * (MAX_LINE_BYTES - len('NOTE text='))
        with self.assertRaisesRegex(ValueError, 'line 1:.*normalized line'):
            parse_document(raw_limit)
        # Canonical defaults and quotes must also fit the total document cap.
        count = 32
        payload_length = (MAX_DOCUMENT_BYTES - (count - 1)) // count - overhead
        lines = ['NOTE text=' + 'x' * payload_length] * count
        accepted = parse_document('\n'.join(lines))
        self.assertEqual(accepted, parse_document(format_document(accepted)))
        with self.assertRaisesRegex(ValueError, 'line 32:.*normalized document'):
            parse_document('\n'.join(lines[:-1] + [lines[-1] + 'x' * count]))

    def test_caps_and_ast_validation(self):
        for text in (' ' * (MAX_DOCUMENT_BYTES + 1), 'NOTE text="' + 'x' * MAX_LINE_BYTES + '"', '\n'.join(['NOTE text=x'] * (MAX_COMMANDS + 1))):
            with self.assertRaises(ValueError):
                parse_document(text)
        for ast in ({}, [{'op': 'RUN', 'args': {}}], [{'op': 'NOTE', 'args': {}, 'extra': True}]):
            with self.assertRaises(ValueError):
                format_document(ast)


if __name__ == '__main__':
    unittest.main()
