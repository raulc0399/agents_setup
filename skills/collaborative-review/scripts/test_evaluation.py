import copy
import json
from pathlib import Path
import unittest

from review_core.evaluation import evaluate


class EvaluationTests(unittest.TestCase):
    def setUp(self):
        self.cases = [dict(id=name, trigger='trigger', expected_finding='finding',
                           source_commit='commit', test='test') for name in ('a', 'b')]
        self.result = dict(run_id='run-1', strategy='single', detected_ids=['a'],
                           elapsed_seconds=12, cost=0.25, currency='USD')

    def test_normal_aggregate(self):
        result = evaluate(self.cases, [self.result])
        metrics = result['aggregate']
        self.assertEqual((metrics['true_positives'], metrics['false_positives'], metrics['misses']), (1, 0, 1))
        self.assertEqual((metrics['precision'], metrics['recall']), (1, 0.5))
        self.assertEqual((metrics['elapsed_seconds'], metrics['cost']), (12, 0.25))
        self.assertEqual(result['runs'][0]['missed_ids'], ['b'])

    def test_unknown_ids_are_false_positives(self):
        self.result['detected_ids'] = ['a', 'unknown']
        result = evaluate(self.cases, [self.result])['runs'][0]
        self.assertEqual(result['false_positive_ids'], ['unknown'])
        self.assertEqual(result['precision'], 0.5)

    def test_duplicate_detections_rejected(self):
        self.result['detected_ids'] = ['a', 'a']
        with self.assertRaisesRegex(ValueError, 'duplicate detection'):
            evaluate(self.cases, [self.result])

    def test_invalid_measurements(self):
        for field in ('elapsed_seconds', 'cost'):
            for value in (None, True, '1', -1, float('nan'), float('inf')):
                with self.subTest(field=field, value=value):
                    result = dict(self.result, **{field: value})
                    with self.assertRaisesRegex(ValueError, field):
                        evaluate(self.cases, [result])

    def test_oversized_measurements_rejected(self):
        for field in ('elapsed_seconds', 'cost'):
            with self.subTest(field=field):
                with self.assertRaisesRegex(ValueError, field):
                    evaluate(self.cases, [dict(self.result, **{field: 10 ** 400})])

    def test_aggregate_overflow_rejected(self):
        for field in ('elapsed_seconds', 'cost'):
            for strategy in ('single', 'parallel'):
                with self.subTest(field=field, strategy=strategy):
                    first = dict(self.result, **{field: 1e308})
                    second = dict(first, run_id='run-2', strategy=strategy)
                    with self.assertRaisesRegex(ValueError, field):
                        evaluate(self.cases, [first, second])

    def test_strategy_comparison_preserves_measured_totals(self):
        second = dict(self.result, run_id='run-2', strategy='parallel',
                      detected_ids=['a', 'b'], elapsed_seconds=8, cost=0.75)
        third = dict(second, run_id='run-3', elapsed_seconds=10)
        result = evaluate(self.cases, [self.result, second, third])
        self.assertEqual(result['strategies']['single']['recall'], 0.5)
        self.assertEqual(result['strategies']['parallel']['recall'], 1)
        self.assertEqual(result['strategies']['parallel']['elapsed_seconds'], 18)
        self.assertEqual(result['aggregate']['cost'], 1.75)
        self.assertEqual(result['aggregate']['true_positives'], 5)

    def test_invalid_schema_and_duplicate_identifiers(self):
        for cases, results in (([], []), (self.cases, {}),
                               (self.cases + [self.cases[0]], []),
                               ([{'id': 'a'}], []),
                               (self.cases, [self.result, self.result]),
                               (self.cases, [dict(self.result, detected_ids='a')]),
                               (self.cases, [dict(self.result, detected_ids=[1])]),
                               (self.cases, [dict(self.result, strategy='')]),
                               (self.cases, [dict(self.result, currency='')])):
            with self.subTest(cases=cases, results=results):
                with self.assertRaises(ValueError):
                    evaluate(cases, results)

    def test_mixed_currency_rejected(self):
        second = dict(self.result, run_id='run-2', currency='EUR')
        with self.assertRaisesRegex(ValueError, 'single currency'):
            evaluate(self.cases, [self.result, second])

    def test_no_detections_and_no_results(self):
        result = evaluate(self.cases, [dict(self.result, detected_ids=[])])['aggregate']
        self.assertIsNone(result['precision'])
        self.assertEqual(result['recall'], 0)
        empty = evaluate(self.cases, [])['aggregate']
        self.assertIsNone(empty['recall'])
        self.assertIsNone(empty['currency'])
        self.assertEqual(empty['run_count'], 0)

    def test_repository_fixture_contains_only_ground_truth(self):
        fixture = Path(__file__).with_name('fixtures') / 'review-evaluation.json'
        cases = json.loads(fixture.read_text())
        before = copy.deepcopy(cases)
        self.assertEqual(evaluate(cases, [])['case_count'], 4)
        self.assertEqual(cases, before)
        for case in cases:
            self.assertEqual(set(case), {'id', 'trigger', 'expected_finding', 'source_commit', 'test'})
            path, symbol = case['test'].split('::')
            repository = Path(__file__).resolve().parents[3]
            source = (repository / path).read_text()
            self.assertIn('class ' + symbol.split('.')[0] + '(', source)
            self.assertIn('def ' + symbol.split('.')[-1] + '(', source)


if __name__ == '__main__':
    unittest.main()
