"""Versioned, evidence-bound agreement on individual disputed points."""
import hashlib
import json

from . import findings


POINT_ACTIONS = ('PROPOSE', 'AGREE', 'CONTEST')
SUPPORT_LINKS = {
    'finding': {}, 'source': {}, 'assumption': {},
    'question': {'finding_ids': 'finding', 'answer_id': 'answer'},
    'answer': {'question_id': 'question', 'option_id': 'option'},
    'option': {'finding_ids': 'finding'}, 'coverage': {'finding_ids': 'finding'},
    'claim': {'source_ids': 'source', 'contradictions': 'claim'},
    'recommendation': {'claim_ids': 'claim'},
}


def _hash(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def current_plan(store):
    plans = [row for row in store.list_records('review_plan')
             if row['data']['request']['revision'] == store.get_meta('revision')
             and row['data'].get('point_consensus_required')]
    return plans[0] if len(plans) == 1 else None


def _leads(store):
    plan = current_plan(store)
    if plan is None:
        raise ValueError('Point consensus requires a current review plan')
    reviews = [store.get_record('task', task_id) for task_id in plan['data']['task_ids']]
    leads = [task['data'].get('worker') or task['data']['payload'].get('reviewer')
             for task in reviews if task and task['data']['kind'] == 'review']
    if len(leads) != 2 or len(set(leads)) != 2 or not all(leads):
        raise ValueError('Point consensus requires two distinct team leads')
    return leads


def blind_pending(store):
    return any(task['data']['payload'].get('blind') and task['data']['status'] != 'completed'
               for task in store.list_records('task')
               if task['data']['payload']['revision'] == store.get_meta('revision'))


def _support(store, refs):
    """Bind referenced records and their transitive evidence, without cycles."""
    records = {}
    revision = store.get_meta('revision')
    def capture(kind, identity):
        key = kind + ':' + identity
        if key in records:
            return
        row = store.get_record(kind, identity)
        if row is None or row['data'].get('revision') != revision:
            raise ValueError('Missing or stale point evidence: ' + key)
        records[key] = row['data']
        for field, target in SUPPORT_LINKS[kind].items():
            values = row['data'].get(field) or []
            for value in values if isinstance(values, list) else [values]:
                capture(target, value)
    for identity in refs:
        matches = [kind for kind in SUPPORT_LINKS if store.get_record(kind, identity) is not None]
        if len(matches) != 1:
            raise ValueError('Unknown or ambiguous point evidence: ' + identity)
        capture(matches[0], identity)
    return records


def _support_scope(support, paths):
    for data in support.values():
        if not set(data.get('paths', []) + data.get('affected_paths', [])) <= set(paths):
            raise ValueError('Point evidence is outside its declared paths')


def create_point(store, author, revision, point_id, text, paths, evidence, refs=(), question_id=None):
    with store.transaction():
        _leads(store)
        findings._revision(store, revision)
        paths = findings._paths(store, paths)
        evidence = findings._strings(evidence, 'evidence')
        refs = findings._strings(refs, 'refs')
        if not paths or not evidence:
            raise ValueError('A disputed point requires paths and evidence')
        _support_scope(_support(store, refs), paths)
        data = dict(author=findings._text(author, 'author'), revision=revision,
                    text=findings._text(text, 'text'), paths=paths, evidence=evidence, refs=refs,
                    question_id=question_id, content_version=1, proposal=None,
                    challenge=None, needs_revision=True)
        return findings._create(store, 'consensus_point', point_id, data)


def point_state(store, point_id):
    with store.read_transaction():
        point = findings._existing(store, 'consensus_point', point_id)
        data = point['data']
        refs = list(data['refs'])
        for item in (data['proposal'], data['challenge']):
            if item:
                refs.extend(item['refs'])
        support = _support(store, refs)
        _support_scope(support, data['paths'])
        question = None
        if data.get('question_id'):
            row = findings._existing(store, 'question', data['question_id'])
            question = {key: row['data'][key] for key in ('text', 'finding_ids', 'content_version')}
        digest = _hash(dict(point=data, support=support, question=question))
        leads = _leads(store)
        approved = sorted({row['data']['author'] for row in store.list_records('point_approval')
                           if row['data']['revision'] == data['revision']
                           and row['data']['point_id'] == point_id and row['data']['digest'] == digest
                           and row['data']['author'] in leads})
        agreed = bool(data['proposal']) and not data['needs_revision'] and set(approved) == set(leads)
        return dict(point=point, digest=digest, support=support, approved_by=approved,
                    pending_approvals=[lead for lead in leads if lead not in approved],
                    status='agreed' if agreed else 'open')


def _current(store, author, point_id, digest):
    if author not in _leads(store):
        raise PermissionError('Only the two team leads may propose, challenge or approve a point')
    if blind_pending(store):
        raise ValueError('Point deliberation waits for both independent reviews')
    state = point_state(store, point_id)
    if state['digest'] != digest:
        raise ValueError('Point resolution or evidence changed; read its current digest')
    return state


def propose(store, author, point_id, digest, text, evidence, refs=()):
    return _change(store, author, point_id, digest, text, evidence, refs, 'proposal')


def contest(store, author, point_id, digest, text, evidence, refs=()):
    return _change(store, author, point_id, digest, text, evidence, refs, 'challenge')


def _change(store, author, point_id, digest, text, evidence, refs, field):
    with store.transaction():
        state = _current(store, author, point_id, digest)
        point = state['point']
        evidence = findings._strings(evidence, 'evidence')
        refs = findings._strings(refs, 'refs')
        if not evidence:
            raise ValueError('Point deliberation requires evidence')
        _support_scope(_support(store, refs), point['data']['paths'])
        data = dict(point['data'])
        data[field] = dict(author=author, text=findings._text(text, 'text'), evidence=evidence, refs=refs)
        data['needs_revision'] = field == 'challenge'
        data['content_version'] += 1
        return store.put_record('consensus_point', point_id, data, actor=author,
                                event='point.' + field, expected_version=point['version'])


def agree(store, author, point_id, digest, evidence):
    with store.transaction():
        state = _current(store, author, point_id, digest)
        data = state['point']['data']
        evidence = findings._strings(evidence, 'evidence')
        if not data['proposal'] or data['needs_revision'] or not evidence:
            raise ValueError('Approval requires a proposed resolution and explicit review evidence')
        identity = 'point-approval-' + _hash([data['revision'], point_id, digest, author])
        approval = findings._create(store, 'point_approval', identity, dict(author=author,
            revision=data['revision'], point_id=point_id, digest=digest, evidence=evidence))
        if point_state(store, point_id)['status'] == 'agreed' and data.get('question_id'):
            question = findings._existing(store, 'question', data['question_id'])
            if question['data']['status'] != 'answered':
                findings.answer_question(store, author=author, revision=data['revision'],
                    question_id=question['id'], text=data['proposal']['text'],
                    evidence=data['proposal']['evidence'] + evidence,
                    answer_id='point-answer-' + _hash([point_id, digest]))
        return approval


def consensus_gaps(store):
    with store.read_transaction():
        gaps = []
        for point in store.list_records('consensus_point'):
            if point['data']['revision'] != store.get_meta('revision'):
                continue
            try:
                state = point_state(store, point['id'])
                if state['status'] == 'agreed':
                    continue
                reason = 'Point requires a resolution approved by both leads: ' + point['data']['text']
            except ValueError as error:
                reason = str(error)
            gaps.append(dict(kind='consensus_point', id=point['id'], reason=reason))
        return gaps


def schedule_consensus(store, coordinator, timeout_seconds=300):
    """Assign one point at a time per task; repeat until both leads explicitly agree."""
    if coordinator.store is not store:
        raise ValueError('Coordinator must use the supplied store')
    with store.transaction():
        plan = current_plan(store)
        if plan is None or blind_pending(store):
            return []
        leads = _leads(store)
        tasks = []
        for point in store.list_records('consensus_point'):
            if point['data']['revision'] != store.get_meta('revision'):
                continue
            state = point_state(store, point['id'])
            previous = [task for task in store.list_records('task')
                        if task['data']['kind'] == 'consensus'
                        and task['data']['payload'].get('point_id') == point['id']
                        and task['data']['payload']['revision'] == point['data']['revision']]
            if any(task['data']['status'] == 'running' for task in previous):
                continue
            if any(task['data']['status'] in ('failed', 'uncertain') or
                   (task['data']['status'] == 'cancelled' and not task['data'].get('consensus_superseded'))
                   for task in previous):
                continue
            for task in previous:
                if task['data']['status'] == 'pending' and (state['status'] == 'agreed' or
                        task['data']['payload']['point_digest'] != state['digest'] or
                        task['data']['payload']['worker'] in state['approved_by']):
                    cancelled = coordinator.cancel_task(task['id'], 'Point changed or was already agreed')
                    store.put_record('task', task['id'], dict(cancelled['data'], consensus_superseded=True),
                                     event='point.task_superseded')
            if state['status'] == 'agreed':
                continue
            data = point['data']
            if data['needs_revision']:
                challenger = (data['challenge'] or {}).get('author', data['author'])
                worker = next(lead for lead in leads if lead != challenger)
            else:
                worker = state['pending_approvals'][0]
            identity = 'consensus-' + _hash([point['id'], state['digest'], worker])
            existing = store.get_record('task', identity)
            if existing:
                tasks.append(existing)
                continue
            brief = ('Deliberate only the assigned disputed point. Challenge its claims and evidence; '
                     'start fresh concern subagents for additional research or review whenever needed. '
                     'Address the recorded counterargument rather than trading acknowledgements. '
                     'Return exactly one PROPOSE, CONTEST, or AGREE for this point using its supplied '
                     'digest. AGREE approves only this exact resolution and evidence; proposing does not '
                     'approve it. Do not use VOTE or approve other points in this task. If external input '
                     'is essential, return a blocking GAP instead. Include actual checks and subagent '
                     'provenance in evidence. Context: ' + json.dumps(state, sort_keys=True))
            payload = dict(worker=worker, revision=data['revision'], point_id=point['id'],
                point_digest=state['digest'], brief=brief, scope_paths=data['paths'], required_action='review',
                timeout_seconds=min(timeout_seconds, plan['data']['peer_review_budget_seconds']))
            tasks.append(coordinator.enqueue_task('consensus', payload, task_id=identity))
        return tasks


def supersede_result(store, task):
    data = task['data']
    state = point_state(store, data['payload']['point_id'])
    if (state['digest'] == data['payload']['point_digest'] and state['status'] != 'agreed'
            and data['worker'] not in state['approved_by']):
        return False
    reason = 'Point changed before this response completed'
    data.update(status='cancelled', error=reason, consensus_superseded=True, result_disposition='superseded')
    if data.get('attempts'):
        data['attempts'][-1].update(status='cancelled', error=reason)
    store.put_record('task', task['id'], data, actor=data['worker'], event='point.task_superseded')
    return True


def validate_result(task, operations):
    if any(op['op'] in ('POINT', 'DISPUTE') for op in operations):
        raise ValueError('Open other points in a separate message, not this assigned point response')
    actions = [op for op in operations if op['op'] in POINT_ACTIONS]
    if not actions and any(op['op'] == 'GAP' for op in operations):
        return
    if len(actions) != 1 or actions[0]['args']['id'] != task['data']['payload']['point_id']:
        raise ValueError('A consensus task must address exactly its assigned point')
    if actions[0]['args']['digest'] != task['data']['payload']['point_digest']:
        raise ValueError('A consensus task must use its assigned point digest')


def publish_result(store, task, result):
    from .messages import send_message
    data = task['data']
    recipient = next(lead for lead in _leads(store) if lead != data['worker'])
    send_message(store, data['worker'], recipient, result['aasm'], task_id=task['id'],
                 lease_id=data['lease_id'], dialect_id=result['dialect_id'],
                 message_id='point-result-' + data['lease_id'])
