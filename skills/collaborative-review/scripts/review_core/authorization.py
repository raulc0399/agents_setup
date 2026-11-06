"""Revision-bound operator grants for trusted local review processes.

These lexical scope checks are an authorization policy, not an OS sandbox.
"""
import math
import re
import time
import uuid


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


def record_authorization(store, revision, actions, paths, expires_at=None, actor='operator', decision='approved'):
    """Record a new immutable decision; later revocation never changes it."""
    _text(revision, 'Revision')
    _text(actor, 'Actor')
    if decision not in ('approved', 'denied'):
        raise ValueError('Decision must be approved or denied')
    actions = _actions(actions)
    paths = canonical_paths(paths)
    if expires_at is not None:
        _timestamp(expires_at, 'Expiry')
    data = {'revision': revision, 'actions': actions, 'paths': paths,
            'expires_at': expires_at, 'actor': actor, 'decision': decision}
    return store.put_record('authorization', 'authorization-' + uuid.uuid4().hex, data,
                            actor=actor, event='authorization.recorded', expected_version=0)


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


def authorize_task(store, action, revision, paths, now=None):
    """Return one live grant covering every requested path, or deny the task."""
    _actions([action])
    _text(revision, 'Revision')
    paths = canonical_paths(paths)
    now = _timestamp(time.time() if now is None else now, 'Current time')
    revoked = {record['data']['authorization_id'] for record in store.list_records('authorization_revocation')}
    for record in store.list_records('authorization'):
        data = record['data']
        if (record['id'] in revoked or data['decision'] != 'approved' or data['revision'] != revision
                or action not in data['actions']):
            continue
        if data['expires_at'] is not None and now >= data['expires_at']:
            continue
        if all(any(scope == '.' or path == scope or path.startswith(scope + '/')
                   for scope in data['paths']) for path in paths):
            return record['id']
    raise PermissionError(f'No current authorization for {action} at revision {revision}')
