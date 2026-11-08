"""Revision-bound AASM transport for trusted local review participants."""
import hashlib
import json
import re
import time
import uuid

from .aasm import BASE_DIALECT, BUILTIN_OPERATIONS, expand_operations, parse_document
from .aasm_records import REFERENCE_KINDS
from .authorization import authorize_task
from .dialects import get_dialect


CREATED_KINDS = dict(B='finding', D='finding', O='finding', DISPUTE='question', Q='question',
                    A='answer', OPT='option', COV='coverage', SRC='source', F='claim',
                    GAP='question', REC='recommendation', ASSUME='assumption', IMP='protocol_proposal',
                    POINT='consensus_point')
REFERENCE_FIELDS = {'FIX': {'id': 'finding'}, 'REOPEN': {'id': 'finding'},
                    'DISPUTE': {'finding': 'finding'}, 'A': {'question': 'question', 'option': 'option'},
                    'CONFIRM': {'id': 'claim'}, 'VERIFY': {'id': 'assumption'},
                    'VOTE': {'evidence': 'coverage'},
                    'PROPOSE': {'id': 'consensus_point'}, 'AGREE': {'id': 'consensus_point'},
                    'CONTEST': {'id': 'consensus_point'}}
SHARED_REFERENCE_FIELDS = {'findings': 'finding', 'sources': 'source', 'contradictions': 'claim',
                           'claims': 'claim', 'refs': None}


def _name(value, label):
    if not isinstance(value, str) or not value or value != value.strip():
        raise ValueError(f'{label} must be a nonempty trimmed string')
    return value


def _message_id(value):
    value = 'message-' + uuid.uuid4().hex if value is None else value
    if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.:-]*', value):
        raise ValueError('Invalid message ID')
    return value


def _context(store):
    revision = _name(store.get_meta('revision'), 'Revision')
    tasks = [record for record in store.list_records('task')
             if record['data'].get('payload', {}).get('revision') == revision]
    explicit = store.get_meta('participants', [])
    if (not isinstance(explicit, list) or any(not isinstance(name, str) or not name.strip()
            or name != name.strip() for name in explicit) or len(set(explicit)) != len(explicit)):
        raise ValueError('participants must be a list of unique nonempty identities')
    known = set(explicit)
    for task in tasks:
        data = task['data']
        for name in (data.get('worker'), data.get('payload', {}).get('worker'),
                     data.get('payload', {}).get('reviewer')):
            if isinstance(name, str) and name.strip():
                known.add(name)
    return revision, tasks, explicit, known


def _blind_pending(tasks):
    return any(task['data']['payload'].get('blind')
               and task['data'].get('status') != 'completed' for task in tasks)


def _visible(data, participant, revision, blind_pending):
    return (data.get('revision') == revision and participant in (data.get('sender'), data.get('recipient'))
            and (not blind_pending or participant == 'coordinator' or data.get('sender') == participant
                 or data.get('sender') == 'coordinator'))


def _sequence(store, record):
    events = [event for event in store.events() if event['kind'] == 'message.sent'
              and event['record_id'] == record['id']]
    if len(events) != 1:
        raise ValueError('Message must have exactly one publication event')
    return {**record, 'sequence': events[0]['seq']}


def _authorize(store, sender, recipient, revision, tasks, explicit, task_id, lease_id):
    if task_id is None:
        if lease_id is not None or sender not in explicit or recipient not in explicit:
            raise PermissionError('Taskless messaging requires explicit participants')
        paths = store.get_meta('paths')
        if not paths:
            paths = [entry['path'] for entry in store.get_meta('snapshot', {}).get('files', [])]
        authorize_task(store, 'review', revision, paths)
        return
    task = next((task for task in tasks if task['id'] == task_id), None)
    if task is None:
        raise ValueError('Unknown or stale sender task')
    data = task['data']
    if not lease_id or data.get('worker') != sender or data.get('lease_id') != lease_id:
        raise PermissionError('Sender must match task worker and lease')
    if data.get('status') not in ('running', 'completed'):
        raise PermissionError('Sender task must be running or completed')
    if data['status'] == 'running' and time.time() >= data.get('lease_expires', 0):
        raise PermissionError('Sender lease has expired')
    action = 'review'
    if data['status'] == 'completed' and recipient == 'coordinator':
        from .coordinator import ACTIONS
        action = ACTIONS.get(data.get('kind'))
        if action is None or data['payload'].get('required_action', action) != action:
            raise ValueError('Task action must match its kind')
    authorize_task(store, action, revision, data['payload']['scope_paths'])


def _references(store, operations, sender, revision, blind_pending):
    available = {}
    for kind in REFERENCE_KINDS:
        for record in store.list_records(kind):
            data = record['data']
            if data.get('revision', data.get('payload', {}).get('revision')) != revision:
                continue
            if kind == 'message':
                if not _visible(data, sender, revision, blind_pending):
                    continue
            elif blind_pending:
                owners = [data[field] for field in ('author', 'reviewer') if field in data]
                # Missing provenance is private during independent review too.
                if not owners or any(owner != sender for owner in owners):
                    continue
            available.setdefault(record['id'], set()).add(kind)
    for operation in operations:
        op, args = operation['op'], operation['args']
        if op not in BUILTIN_OPERATIONS:
            continue
        fields = {**SHARED_REFERENCE_FIELDS, **REFERENCE_FIELDS.get(op, {})}
        for field, kind in fields.items():
            if field not in args:
                continue
            values = args[field] if isinstance(args[field], list) else [args[field]]
            for record_id in values:
                kinds = available.get(record_id, set())
                if (kind is None and len(kinds) != 1) or (kind is not None and kind not in kinds):
                    raise ValueError(f'Unknown, stale, ambiguous or inaccessible reference: {record_id}')
        if op in CREATED_KINDS:
            available.setdefault(args['proposal'] if op == 'IMP' else args['id'], set()).add(CREATED_KINDS[op])


def send_message(store, sender, recipient, body, dialect_id=BASE_DIALECT, reply_to=None,
                 message_id=None, task_id=None, lease_id=None, apply=None):
    """Validate, optionally apply, and publish once in a single transaction.

    The caller asserts sender identity using the same trusted local process model
    as Coordinator; AASM text never grants authority or executes a command.
    """
    _name(sender, 'Sender')
    _name(recipient, 'Recipient')
    message_id = _message_id(message_id)
    if reply_to is not None:
        _message_id(reply_to)
    if apply is not None and not callable(apply):
        raise ValueError('apply must be callable')
    with store.transaction():
        revision, tasks, explicit, known = _context(store)
        if sender not in known or recipient not in known | {'coordinator'}:
            raise ValueError('Unknown sender or recipient')
        request = dict(body=body, dialect_id=dialect_id, sender=sender, recipient=recipient,
                       reply_to=reply_to, revision=revision, task_id=task_id, lease_id=lease_id)
        old = store.get_record('message', message_id)
        if old is not None:
            if any(old['data'].get(key) != value for key, value in request.items()):
                raise ValueError('Message ID already has a different request')
            return _sequence(store, old)
        _authorize(store, sender, recipient, revision, tasks, explicit, task_id, lease_id)
        blind_pending = _blind_pending(tasks)
        if reply_to is not None:
            parent = store.get_record('message', reply_to)
            if (reply_to == message_id or parent is None
                    or not _visible(parent['data'], sender, revision, blind_pending)
                    or {parent['data']['sender'], parent['data']['recipient']} != {sender, recipient}):
                raise ValueError('Reply must reference an accessible conversation in this revision')
        dialect = get_dialect(store, dialect_id, participant=sender)
        if dialect_id != BASE_DIALECT and recipient != 'coordinator':
            get_dialect(store, dialect_id, participant=recipient)
        operations = expand_operations(parse_document(body, dialect['definitions']), dialect['definitions'])
        _references(store, operations, sender, revision, blind_pending)
        data = {**request, 'operations': operations, 'dialect_digest': dialect['digest']}
        if apply is not None:
            apply(operations, data)
        record = store.put_record('message', message_id, data, actor=sender,
                                  event='message.sent', expected_version=0)
        return _sequence(store, record)


def read_messages(store, recipient, after=0):
    """Read a durable cursor without marking messages read or changing state."""
    _name(recipient, 'Recipient')
    if type(after) is not int or after < 0:
        raise ValueError('Message cursor must be a nonnegative integer')
    with store.read_transaction():
        revision, tasks, _, known = _context(store)
        if recipient not in known | {'coordinator'}:
            raise ValueError('Unknown recipient')
        blind_pending = _blind_pending(tasks)
        messages = []
        for event in store.events(after):
            if event['kind'] != 'message.sent':
                continue
            record = store.get_record('message', event['record_id'])
            if record is None:
                raise ValueError('Publication references a missing message')
            data = record['data']
            if data['recipient'] != recipient or data.get('revision') != revision:
                continue
            if not _visible(data, recipient, revision, blind_pending):
                # A later visible sequence must never skip a deferred delivery.
                break
            messages.append({**record, 'sequence': event['seq']})
        return messages


def exchange_reviews(store):
    """Publish each team's submitted review once, after every blind task finishes."""
    from .aasm_records import apply_operations

    with store.transaction():
        revision, tasks, _, _ = _context(store)
        if _blind_pending(tasks):
            return []
        plans = [plan for plan in store.list_records('review_plan')
                 if plan['data']['request']['revision'] == revision
                 and 'peer_review_budget_seconds' in plan['data']]
        messages = []
        for plan in plans:
            reviews = [task for task in tasks if task['id'] in plan['data']['task_ids']
                       and task['data']['kind'] == 'review']
            if len(reviews) != 2 or any(task['data']['status'] != 'completed' for task in reviews):
                continue
            if len({task['data']['worker'] for task in reviews}) != 2:
                raise ValueError('Review exchange requires two independent team leads')
            for source, target in (reviews, reviews[::-1]):
                data = source['data']
                sender, recipient = data['worker'], target['data']['worker']
                identity = hashlib.sha256((plan['id'] + source['id'] + data['lease_id']
                                           + target['data']['lease_id']).encode()).hexdigest()
                message_id = 'review-exchange-' + identity
                existing = store.get_record('message', message_id)
                if existing is not None:
                    messages.append(_sequence(store, existing))
                    continue
                result = data.get('result') or {}
                if not result.get('aasm'):
                    raise ValueError('Review exchange requires the submitted AASM result')
                findings = [operation['args']['id'] for operation in result['operations']
                            if operation['op'] in ('B', 'D', 'O')]
                question = ('Automatic cross-check: use your concern subagents to examine the other team\'s '
                            'submitted review against the pinned inputs and your own evidence. '
                            'Answer with evidence for agreements and disagreements; record every actionable '
                            'dispute or missing evidence explicitly. Open a separate POINT for each '
                            'disagreement, or use DISPUTE for a finding; both leads must subsequently '
                            'approve each point individually. Do not claim consensus without checking.')
                body = ('NOTE text="Submitted independent team review" refs=' + json.dumps([source['id']])
                        + '\nQ id=peer-review-' + identity
                        + ' text=' + json.dumps(question) + ' findings=' + json.dumps(findings))
                def apply(operations, envelope):
                    apply_operations(store, envelope['sender'], revision, operations,
                                     task_id=envelope['task_id'])
                messages.append(send_message(store, sender, recipient, body, message_id=message_id,
                    task_id=source['id'], lease_id=data['lease_id'], apply=apply))
        return messages
