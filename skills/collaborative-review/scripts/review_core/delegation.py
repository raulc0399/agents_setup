"""Atomic planning of blind reviews against one pinned revision."""
import hashlib
import math
from pathlib import PurePosixPath


MIN_TIMEOUT_SECONDS = 1


def _names(values, label):
    if isinstance(values, str):
        raise ValueError(f'{label} must contain names')
    values = list(values)
    if not all(isinstance(value, str) and value.strip() for value in values):
        raise ValueError(f'{label} must contain nonempty names')
    return sorted(set(values))


def _number(value, label):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
        raise ValueError(f'{label} must be finite and nonnegative')
    return value


def _identity(revision, role):
    return hashlib.sha256((revision + '\0' + role).encode()).hexdigest()


def plan_review(store, coordinator, risks=(), specialists=(), max_workers=None):
    """Return task envelopes; identical revision/request retries reuse the plan.

    Each task gets the same full snapshot scope. Named risks and specialists add
    focused tasks without replacing either independent reviewer. One equal time
    share is reserved for the later challenge; all shares use the run budget.
    """
    if coordinator.store is not store:
        raise ValueError('Coordinator must use the supplied store')
    risks = _names(risks, 'risks')
    specialists = _names(specialists, 'specialists')
    with store.transaction():
        revision = store.get_meta('revision')
        if not isinstance(revision, str) or not revision:
            raise ValueError('A pinned revision is required')
        run_cap = store.get_meta('max_workers')
        cap = run_cap if max_workers is None else max_workers
        if any(isinstance(value, bool) or not isinstance(value, int) or value < 1 for value in (run_cap, cap)):
            raise ValueError('max_workers must be a positive integer')
        if cap > run_cap:
            raise ValueError('max_workers exceeds the run capacity')
        request = dict(revision=revision, risks=risks, specialists=specialists, max_workers=cap)
        plan_id = 'plan-' + _identity(revision, 'review')
        existing = store.get_record('review_plan', plan_id)
        if existing:
            if existing['data']['request'] != request:
                raise ValueError('Revision already has a different review plan')
            tasks = [store.get_record('task', task_id) for task_id in existing['data']['task_ids']]
            if any(task is None for task in tasks):
                raise ValueError('Review plan references a missing task')
            return tasks

        snapshot = store.get_meta('snapshot')
        if not isinstance(snapshot, dict) or snapshot.get('revision') != revision:
            raise ValueError('Snapshot must match the current revision')
        files = snapshot.get('files')
        if not isinstance(files, list) or not files:
            raise ValueError('Snapshot must contain files')
        paths = []
        for entry in files:
            path = entry.get('path') if isinstance(entry, dict) else None
            if not isinstance(path, str) or not path or '\\' in path or PurePosixPath(path).is_absolute() or '..' in PurePosixPath(path).parts or str(PurePosixPath(path)) != path or path == '.':
                raise ValueError('Snapshot contains an invalid file path')
            paths.append(path)
        paths = sorted(set(paths))
        roles = [('review', 'reviewer-1', None), ('review', 'reviewer-2', None)]
        for label, names in (('risk', risks), ('specialist', specialists)):
            roles.extend(('specialist', label + '-' + _identity(revision, name)[:16], f'{label}: {name}') for name in names)

        budget = _number(store.get_meta('budget_seconds'), 'budget_seconds')
        spent = _number(store.get_meta('spent_seconds', 0), 'spent_seconds')
        reserved = sum(_number(task['data']['payload']['timeout_seconds'], 'task timeout')
                       for task in store.list_records('task') if task['data']['status'] in ('pending', 'running'))
        timeout = (budget - spent - reserved) / (len(roles) + 1)
        if timeout < MIN_TIMEOUT_SECONDS:
            raise ValueError('Budget cannot fund reviews and a challenge')
        tasks = []
        for kind, reviewer, focus in roles:
            brief = ('Independently review every pinned scope file. Report all actionable findings with '
                     'file evidence, coverage, and a revision-bound verdict. Do not consult peer reviews. '
                     'Return only AASM commands using the supplied language schemas: B/D/O for findings, '
                     'COV for explicit file coverage, and one final VOTE referencing your COV IDs. '
                     'Use exact path:line or path:start-end locations; explain reasoning in text fields. '
                     'First understand intended behavior and the user case, trace relevant callers and data flow, '
                     'and examine failure paths, conflicting assumptions and blast radius. Run relevant checks '
                     'when feasible; COV evidence must name actual checks and observed results, with partial '
                     'or unreviewed coverage explicit. Distinguish untested hypotheses and limitations from '
                     'verified observations. Each finding claim must explain the triggering condition, observed '
                     'behavior and impact, with a concrete target, proposed fix and supporting evidence. '
                     'Do not impose a finding count limit.')
            if focus:
                brief += ' Pay particular attention to ' + focus + '.'
            payload = dict(revision=revision, brief=brief, scope_paths=paths,
                           timeout_seconds=timeout, reviewer=reviewer, required_action='review', blind=True)
            if focus:
                payload['focus'] = focus
            tasks.append(coordinator.enqueue_task(kind, payload, task_id='task-' + _identity(revision, reviewer)))
        if cap != run_cap:
            store.set_meta('max_workers', cap)
        store.put_record('review_plan', plan_id, dict(request=request, task_ids=[task['id'] for task in tasks],
                         challenge_budget_seconds=timeout), event='review.planned', expected_version=0)
        return tasks
