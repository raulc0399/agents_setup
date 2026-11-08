"""Revision or run-scoped operator grants for trusted local review processes.

These lexical scope checks are an authorization policy, not an OS sandbox.
"""
import math
import re
import time
import uuid


RUN_ACTIONS = ('review', 'inspect')


def _text(value, label):
    if not isinstance(value, str) or not value.strip() or value != value.strip():
        raise ValueError(f'{label} must be a nonempty, trimmed string')
    return value


def _timestamp(value, label):
    try:
        valid = type(value) in (int, float) and math.isfinite(value) and value >= 0
    except OverflowError:
        valid = False
    if not valid:
        raise ValueError(f'{label} must be finite nonnegative Unix seconds')
    return value


def canonical_paths(paths):
    """Return sorted unique relative scopes; '.' explicitly covers the root."""
    if not isinstance(paths, (list, tuple)) or not paths:
        raise ValueError('Paths must be a nonempty list or tuple')
    result = set()
    for path in paths:
        _text(path, 'Path')
        if path == '.':
            result.add(path)
            continue
        if ('\\' in path or '\x00' in path or path.startswith('/') or re.match(r'^[A-Za-z]:', path)
                or any(part in ('', '.', '..') for part in path.split('/'))):
            raise ValueError(f'Invalid relative scope: {path!r}')
        result.add(path)
    return sorted(result)


def _actions(actions):
    if not isinstance(actions, (list, tuple)) or not actions:
        raise ValueError('Actions must be a nonempty list or tuple')
    for action in actions:
        _text(action, 'Action')
        if not re.fullmatch(r'[a-z][a-z0-9_-]*', action):
            raise ValueError('Actions must be explicit action names')
    return sorted(set(actions))


def _run_scope(store):
    return dict(run_id=store.run_dir.name, run_dir=str(store.run_dir.absolute()),
                project_root=_text(store.get_meta('project_root'), 'Project root'),
                approval_scope=_text(store.get_meta('approval_scope'), 'Approval scope'))


def record_authorization(store, revision, actions, paths, expires_at=None, actor='operator', decision='approved', approval=None, follow_revisions=False):
    """Record a new immutable decision; later revocation never changes it."""
    _text(revision, 'Revision')
    _text(actor, 'Actor')
    if decision not in ('approved', 'denied'):
        raise ValueError('Decision must be approved or denied')
    actions = _actions(actions)
    paths = canonical_paths(paths)
    if type(follow_revisions) is not bool:
        raise ValueError('follow_revisions must be boolean')
    if expires_at is not None:
        _timestamp(expires_at, 'Expiry')
    data = {'revision': revision, 'actions': actions, 'paths': paths,
            'expires_at': expires_at, 'actor': actor, 'decision': decision}
    if follow_revisions:
        if revision != store.get_meta('revision') or not set(actions) <= set(RUN_ACTIONS):
            raise ValueError('Run authorization requires the current revision and only review/inspect actions')
        data.update(follow_revisions=True, run_scope=_run_scope(store))
    if approval is not None:
        if not isinstance(approval, dict) or set(approval) != {'source', 'message', 'reference'}:
            raise ValueError('Approval evidence requires source, message and reference')
        if approval['source'] != 'chat':
            raise ValueError('Unsupported approval evidence source')
        _text(approval['message'], 'Approval message')
        _text(approval['reference'], 'Approval reference')
        data['approval'] = dict(approval)
    return store.put_record('authorization', 'authorization-' + uuid.uuid4().hex, data,
                            actor=actor, event='authorization.recorded', expected_version=0)


def record_chat_authorization(store, request):
    """Record user approval relayed by a trusted chat agent for this exact run."""
    fields = {'run_id', 'revision', 'approval_scope', 'actions', 'paths', 'message', 'reference'}
    if not isinstance(request, dict) or not fields <= set(request) or set(request) - fields - {'follow_revisions'}:
        raise ValueError('Chat approval requires: ' + ', '.join(sorted(fields)) + '; optional: follow_revisions')

    with store.transaction():
        if request['run_id'] != store.run_dir.name:
            raise ValueError('Chat approval run does not match this run')
        if request['revision'] != store.get_meta('revision'):
            raise ValueError('Chat approval revision is stale or does not match this run')
        if request['approval_scope'] != store.get_meta('approval_scope'):
            raise ValueError('Chat approval scope does not match this run')
        if canonical_paths(request['paths']) != canonical_paths(store.get_meta('paths')):
            raise ValueError('Chat approval paths must match the displayed captured scope')

        evidence = {'source': 'chat', 'message': request['message'], 'reference': request['reference']}
        return record_authorization(store, request['revision'], request['actions'], request['paths'],
                                    actor='user', approval=evidence, follow_revisions=request.get('follow_revisions', False))


def revoke_authorization(store, authorization_id, actor='operator', reason=''):
    """Append one stable revocation record, preserving the original decision."""
    _text(authorization_id, 'Authorization id')
    _text(actor, 'Actor')
    if not isinstance(reason, str):
        raise ValueError('Reason must be a string')
    with store.transaction():
        if store.get_record('authorization', authorization_id) is None:
            raise ValueError(f'Unknown authorization: {authorization_id}')
        revocation_id = 'revocation-' + authorization_id
        existing = store.get_record('authorization_revocation', revocation_id)
        if existing is not None:
            return existing
        return store.put_record('authorization_revocation', revocation_id,
                                {'authorization_id': authorization_id, 'actor': actor, 'reason': reason},
                                actor=actor, event='authorization.revoked', expected_version=0)


def authorize_task(store, action, revision, paths, now=None, require_run_grant=False):
    """Return one live grant covering every requested path, or deny the task."""
    _actions([action])
    _text(revision, 'Revision')
    paths = canonical_paths(paths)
    now = _timestamp(time.time() if now is None else now, 'Current time')
    revoked = {record['data']['authorization_id'] for record in store.list_records('authorization_revocation')}
    for record in store.list_records('authorization'):
        data = record['data']
        if (record['id'] in revoked or data['decision'] != 'approved'
                or action not in data['actions'] or (require_run_grant and not data.get('follow_revisions'))):
            continue
        if data.get('follow_revisions'):
            if (revision != store.get_meta('revision') or action not in RUN_ACTIONS
                    or data.get('run_scope') != _run_scope(store)):
                continue
        elif data['revision'] != revision:
            continue
        if data['expires_at'] is not None and now >= data['expires_at']:
            continue
        if all(any(scope == '.' or path == scope or path.startswith(scope + '/')
                   for scope in data['paths']) for path in paths):
            return record['id']
    raise PermissionError(f'No current authorization for {action} at revision {revision}')


def start_authorized(store, now=None):
    """Expose the startup gate without granting permission or claiming work."""
    paths = store.get_meta('paths')
    if not paths or not store.get_meta('revision'):
        return False
    try:
        authorize_task(store, 'review', store.get_meta('revision'), paths, now=now)
    except PermissionError:
        return False
    return True
