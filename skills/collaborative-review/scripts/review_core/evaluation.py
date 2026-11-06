"""Score explicitly supplied review detections against documented ground truth.

Cases are objects with id, trigger, expected_finding, source_commit and test.
Results are measured runs with run_id, strategy, detected_ids, elapsed_seconds,
cost and currency. Every run evaluates the entire supplied case set. Unknown
IDs are false positives; repeated detection IDs are rejected, not double counted.
No execution, model judgment, currency conversion or cost estimation is performed.
"""

import math


def _text(value, field):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f'{field} must be a nonempty string')
    return value


def _measurement(value, field):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f'{field} must be a finite nonnegative number')
    try:
        finite = math.isfinite(value)
    except OverflowError:
        finite = False
    if not finite or value < 0:
        raise ValueError(f'{field} must be a finite nonnegative number')
    return value


def _metrics(true_positives, false_positives, misses, elapsed_seconds, cost, currency):
    detected = true_positives + false_positives
    expected = true_positives + misses
    return {
        'true_positives': true_positives,
        'false_positives': false_positives,
        'misses': misses,
        'precision': true_positives / detected if detected else None,
        'recall': true_positives / expected if expected else None,
        'elapsed_seconds': elapsed_seconds,
        'cost': cost,
        'currency': currency,
    }


def _measurement_total(runs, field):
    try:
        total = sum(run[field] for run in runs)
    except OverflowError as error:
        raise ValueError(f'aggregate.{field} exceeds finite numeric range') from error
    return _measurement(total, f'aggregate.{field}')


def _aggregate(runs):
    return _metrics(
        sum(run['true_positives'] for run in runs),
        sum(run['false_positives'] for run in runs),
        sum(run['misses'] for run in runs),
        _measurement_total(runs, 'elapsed_seconds'),
        _measurement_total(runs, 'cost'),
        runs[0]['currency'] if runs else None,
    )


def evaluate(cases, results):
    """Return per-run, per-strategy and microaggregate metrics; invalid data raises ValueError.

    Precision/recall with a zero denominator are None. Elapsed seconds and cost
    aggregates are sums of supplied run measurements, not wall-clock duration.
    A single currency is required because no exchange-rate assumptions are made.
    """
    if not isinstance(cases, list) or not cases:
        raise ValueError('cases must be a nonempty list')
    if not isinstance(results, list):
        raise ValueError('results must be a list')

    known = set()
    for case in cases:
        if not isinstance(case, dict):
            raise ValueError('case must be an object')
        for field in ('id', 'trigger', 'expected_finding', 'source_commit', 'test'):
            _text(case.get(field), f'case.{field}')
        if case['id'] in known:
            raise ValueError(f'duplicate case id: {case["id"]}')
        known.add(case['id'])

    runs = []
    run_ids = set()
    currencies = set()
    strategies = {}
    for result in results:
        if not isinstance(result, dict):
            raise ValueError('result must be an object')
        run_id = _text(result.get('run_id'), 'run_id')
        strategy = _text(result.get('strategy'), 'strategy')
        currency = _text(result.get('currency'), 'currency')
        if run_id in run_ids:
            raise ValueError(f'duplicate run_id: {run_id}')
        run_ids.add(run_id)
        currencies.add(currency)
        if len(currencies) > 1:
            raise ValueError('results must use a single currency')

        detected_ids = result.get('detected_ids')
        if not isinstance(detected_ids, list):
            raise ValueError('detected_ids must be a list')
        detected = set()
        for finding_id in detected_ids:
            _text(finding_id, 'detected_ids entry')
            if finding_id in detected:
                raise ValueError(f'duplicate detection id: {finding_id}')
            detected.add(finding_id)
        elapsed = _measurement(result.get('elapsed_seconds'), 'elapsed_seconds')
        cost = _measurement(result.get('cost'), 'cost')
        run = {'run_id': run_id, 'strategy': strategy,
               **_metrics(len(detected & known), len(detected - known),
                          len(known - detected), elapsed, cost, currency),
               'true_positive_ids': sorted(detected & known),
               'false_positive_ids': sorted(detected - known),
               'missed_ids': sorted(known - detected)}
        runs.append(run)
        strategies.setdefault(strategy, []).append(run)

    return {'case_count': len(known), 'runs': runs,
            'strategies': {name: {'run_count': len(group), **_aggregate(group)}
                           for name, group in sorted(strategies.items())},
            'aggregate': {'run_count': len(runs), **_aggregate(runs)}}
