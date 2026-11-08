"""Final integration challenge after the two planned independent reviews.

A review plan is required: its reserved challenge time funds this task. Results
are ingested by the coordinator's result handler before checking readiness.
"""
import hashlib
import json
import math


CONTEXT_KINDS = ('finding', 'question', 'answer', 'option', 'assumption', 'source',
                 'claim', 'recommendation', 'review', 'authorization', 'authorization_revocation',
                 'integration_surface', 'recovery', 'dialect', 'dialect_test', 'dialect_ack',
                 'consensus_point', 'point_approval')


def _plan(store):
    from .research import research_review_gaps

    if research_review_gaps(store):
        raise ValueError('Research reviews need reassessment against the current research fingerprint')
    revision = store.get_meta('revision')
    plans = [plan for plan in store.list_records('review_plan')
             if plan['data'].get('request', {}).get('revision') == revision]
    if len(plans) != 1:
        raise ValueError('One current review plan is required before a challenge')
    plan = plans[0]
    tasks = [store.get_record('task', task_id) for task_id in plan['data']['task_ids']]
    reviews = [task for task in tasks if task and task['data']['kind'] == 'review']
    if len(reviews) != 2:
        raise ValueError('Review plan must contain two independent review tasks')
    identities = []
    for task in reviews:
        data = task['data']
        worker = data.get('worker')
        if data['status'] != 'completed' or data['payload']['revision'] != revision or not worker:
            raise ValueError('Both independent review tasks must be completed on the current revision')
        evidence = [record for record in store.list_records('review')
                    if record['data'].get('revision') == revision
                    and record['data'].get('reviewer') == worker
                    and record['data'].get('task_id') == task['id']
                    and record['data'].get('evidence_ids')]
        if not evidence:
            raise ValueError('Each independent review requires current task-bound review evidence')
        identities.append(worker)
    if len(set(identities)) != 2:
        raise ValueError('Independent reviewers must have distinct identities')
    return plan, reviews, identities


def _context(store):
    revision = store.get_meta('revision')
    records = {}
    for kind in CONTEXT_KINDS:
        records[kind] = [record for record in store.list_records(kind)
                         if record['data'].get('revision', revision) == revision]
    # Challenger coverage and lifecycle updates are outputs, not new input.
    challengers = {'challenger'} | {task['data']['payload'].get('reviewer') for task in store.list_records('task')
                   if task['data']['kind'] == 'challenge'}
    records['coverage'] = [record for record in store.list_records('coverage')
                           if record['data'].get('revision') == revision
                           and record['data'].get('author') not in challengers]
    for kind in ('message', 'aasm_domain'):
        records[kind] = [record for record in store.list_records(kind)
                         if record['data'].get('revision') == revision
                         and record['data'].get('sender', record['data'].get('author')) not in challengers]
    recovery = [dict(id=task['id'], kind=task['data']['kind'],
                     recovery_decisions=task['data'].get('recovery_decisions', []),
                     error=task['data'].get('error'))
                for task in store.list_records('task')
                if task['data']['kind'] != 'challenge' and
                task['data']['payload'].get('revision') == revision]
    context = dict(revision=revision, snapshot=store.get_meta('snapshot'), records=records,
                   recovery=recovery, changed_paths=store.get_meta('changed_paths', []),
                   integration_surfaces=store.get_meta('integration_surfaces', []),
                   approval_scope=store.get_meta('approval_scope'))
    fingerprint = hashlib.sha256(json.dumps(context, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    return context, fingerprint


def current_context_fingerprint(store):
    """Return the current evidence-context identity for challenge lease checks."""
    with store.read_transaction():
        return _context(store)[1]


def schedule_challenge(store, coordinator):
    """Return the same task for unchanged context; changed evidence needs a new one."""
    if coordinator.store is not store:
        raise ValueError('Coordinator must use the supplied store')
    with store.transaction():
        plan, reviews, identities = _plan(store)
        from .consensus import consensus_gaps
        if consensus_gaps(store):
            raise ValueError('Every disputed point needs explicit approval from both leads before the challenge')
        if 'peer_review_budget_seconds' in plan['data']:
            discussions = [task for task in store.list_records('task')
                           if task['data']['kind'] in ('discussion', 'consensus')
                           and task['data']['payload']['revision'] == store.get_meta('revision')
                           and not task['data'].get('discussion_superseded')
                           and not task['data'].get('consensus_superseded')]
            completed = {task['data']['worker'] for task in discussions
                         if task['data']['status'] == 'completed'
                         and task['data']['payload'].get('peer_review_plan_id') == plan['id']}
            if not set(identities) <= completed or any(
                    task['data']['status'] != 'completed' for task in discussions):
                raise ValueError('Both teams must complete their peer cross-checks and point tasks before the challenge')
        reviewer = 'challenger'
        if reviewer in identities:
            raise ValueError('Challenger identity must differ from independent reviewers')
        context, fingerprint = _context(store)
        for task in store.list_records('task'):
            data = task['data']
            if (data['kind'] == 'challenge' and data['status'] == 'completed'
                    and data['payload'].get('budget_plan_id') == plan['id']
                    and data['payload'].get('revision') == context['revision']
                    and data.get('completion_context_fingerprint') == fingerprint):
                return task
        task_id = 'challenge-' + hashlib.sha256((plan['id'] + fingerprint).encode()).hexdigest()
        existing = store.get_record('task', task_id)
        if existing is not None:
            return existing
        bound = [task for task in store.list_records('task')
                 if task['data']['kind'] == 'challenge'
                 and task['data']['payload'].get('budget_plan_id') == plan['id']
                 and task['data']['payload'].get('revision') == context['revision']]
        if any(task['data']['status'] == 'running' for task in bound):
            raise ValueError('Cannot replace a running challenge before it finishes')
        timeout = plan['data']['challenge_budget_seconds']
        if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not math.isfinite(timeout) or timeout <= 0:
            raise ValueError('Challenge reserve must be positive and finite')
        spent = [task['data'].get('spent_seconds', 0) for task in bound]
        if any(isinstance(value, bool) or not isinstance(value, (int, float))
               or not math.isfinite(value) or value < 0 for value in spent):
            raise ValueError('Challenge spending must be nonnegative and finite')
        timeout -= math.fsum(spent)
        if timeout <= 0:
            raise ValueError('Plan challenge reserve is exhausted')
        brief = ('Challenge the integrated plan, changes, integration surfaces, assumptions, '
                 'research claims, approvals, findings, questions, and recovery decisions. '
                 'Trace intended behavior, relevant callers and data flow; examine failure paths, conflicts '
                 'and blast radius. Run relevant checks when feasible and cite exact observed results in COV '
                 'evidence; mark partial or unreviewed coverage and untested uncertainty explicitly. '
                 'Report every actionable issue using AASM B/D/O commands with exact file locations, '
                 'triggering condition, observed behavior, impact, concrete fix and actual evidence. '
                 'Return only AASM commands using the supplied language schemas. '
                 'Include COV for every scope path on this revision, then one final VOTE. Example:\n'
                 'COV id=coverage-example paths=["scope/file.py"] status=reviewed evidence=["concrete evidence"]\n'
                 'VOTE verdict=APPROVED evidence=["coverage-example"]\n'
                 'Use verdict=BLOCKED when issues prevent approval; use stable IDs unique to this task. '
                 'Context: ' + json.dumps(context, sort_keys=True))
        payload = dict(revision=context['revision'], brief=brief, reviewer=reviewer,
                       required_action='review', scope_paths=reviews[0]['data']['payload']['scope_paths'],
                       timeout_seconds=timeout, budget_plan_id=plan['id'],
                       context_fingerprint=fingerprint)
        for old in store.list_records('task'):
            data = old['data']
            if (data['kind'] == 'challenge' and data['status'] == 'pending'
                    and data['payload'].get('budget_plan_id') == plan['id'] and old['id'] != task_id):
                coordinator.cancel_task(old['id'], 'Superseded by changed challenge context')
        return coordinator.enqueue_task('challenge', payload,
                                        dependencies=[task['id'] for task in reviews], task_id=task_id)


def challenge_gaps(store):
    """Return readiness blockers; process completion alone never approves a challenge."""
    if not store.get_meta('challenge_required', True):
        return []
    with store.read_transaction():
        try:
            plan, reviews, identities = _plan(store)
        except ValueError as error:
            return [dict(kind='challenge', id='challenge', reason=str(error))]
        _, fingerprint = _context(store)
        tasks = [task for task in store.list_records('task')
                 if task['data']['kind'] == 'challenge'
                 and task['data']['payload'].get('revision') == store.get_meta('revision')
                 and task['data']['payload'].get('budget_plan_id') == plan['id']
                 and task['data'].get('completion_context_fingerprint',
                                      task['data']['payload'].get('context_fingerprint')) == fingerprint]
        reason = 'A challenge of the current review context is required'
        for task in tasks:
            data = task['data']
            worker = data.get('worker')
            result = data.get('result') or {}
            if data['status'] != 'completed':
                reason = 'Current challenge has not completed'
                continue
            if not worker or worker in identities or worker != data['payload'].get('reviewer'):
                reason = 'Challenge requires a new independent challenger identity'
                continue
            evidence_ids = result.get('evidence_ids')
            if (result.get('verdict') != 'APPROVED' or not isinstance(evidence_ids, list)
                    or not evidence_ids or not all(isinstance(value, str) and value.strip() for value in evidence_ids)):
                reason = 'Challenge requires APPROVED verdict and explicit evidence_ids'
                continue
            covered = set()
            for coverage in store.list_records('coverage'):
                proof = coverage['data']
                if (coverage['id'] in evidence_ids and proof.get('revision') == data['payload']['revision']
                        and proof.get('author') == worker and proof.get('status') == 'reviewed'
                        and proof.get('evidence')):
                    covered.update(proof.get('paths', []))
            if not set(data['payload']['scope_paths']) <= covered:
                reason = 'Challenge requires current reviewed coverage referenced by evidence_ids for every scope path'
                continue
            return []
        return [dict(kind='challenge', id=tasks[0]['id'] if tasks else 'challenge', reason=reason)]
