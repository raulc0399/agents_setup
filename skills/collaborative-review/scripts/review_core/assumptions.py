"""Explicit assumptions and their evidence across snapshot revisions."""
from pathlib import PurePosixPath
import re
import uuid


def _text(value, name):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f'{name} must be a nonempty string')
    return value


def _strings(values, name):
    if not isinstance(values, (list, tuple)):
        raise ValueError(f'{name} must be a list of strings')
    return sorted(set(_text(value, name) for value in values))


def _overlaps(left, right):
    return left == right or left.startswith(right + '/') or right.startswith(left + '/')


def _paths(store, values, check_scope=True):
    values = _strings(values, 'paths')
    known = [entry['path'] for entry in store.get_meta('snapshot', {}).get('files', [])]
    for path in values:
        if path == '.' or path != str(PurePosixPath(path)) or path.startswith('/') or '..' in PurePosixPath(path).parts or any(char in path for char in ('\\', ':', '\x00')):
            raise ValueError('paths must be canonical relative paths')
        if check_scope and not any(file == path or file.startswith(path + '/') for file in known):
            raise ValueError(f'Path is outside snapshot: {path}')
    return values


def _revision(store, revision):
    _text(revision, 'revision')
    if revision != store.get_meta('revision'):
        raise ValueError('revision must match current run revision')


def add_assumption(store, *, author, revision, statement, affected_paths, evidence=(), blocks_approval=True, assumption_id=None):
    _text(author, 'author')
    _text(statement, 'statement')
    if type(blocks_approval) is not bool:
        raise ValueError('blocks_approval must be boolean')
    assumption_id = assumption_id if assumption_id is not None else f'assumption-{uuid.uuid4().hex}'
    if not isinstance(assumption_id, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.:-]*', assumption_id):
        raise ValueError('Invalid assumption ID')
    with store.transaction():
        _revision(store, revision)
        paths = _paths(store, affected_paths)
        if not paths:
            raise ValueError('affected_paths must not be empty')
        data = dict(author=author, revision=revision, statement=statement, affected_paths=paths, evidence=_strings(evidence, 'evidence'), status='unverified', blocks_approval=blocks_approval, source_revision=revision, history=[])
        creation = dict(data)
        data['creation'] = creation
        old = store.get_record('assumption', assumption_id)
        if old is not None:
            if old['data']['creation'] != creation:
                raise ValueError('Assumption ID already has different data')
            return old
        return store.put_record('assumption', assumption_id, data, actor=author, event='assumption.created', expected_version=0)


def verify_assumption(store, *, assumption_id, author, revision, evidence):
    _text(author, 'author')
    evidence = _strings(evidence, 'evidence')
    if not evidence:
        raise ValueError('Verification requires explicitly supplied evidence references')
    with store.transaction():
        _revision(store, revision)
        old = store.get_record('assumption', assumption_id)
        if old is None or old['data']['revision'] != revision:
            raise ValueError('Unknown or stale assumption')
        data = dict(old['data'], status='verified', evidence=evidence, verified_by=author)
        if old['data'] == data:
            return old
        return store.put_record('assumption', assumption_id, data, actor=author, event='assumption.verified', expected_version=old['version'])


def invalidate_assumptions(store, changed_paths, new_revision):
    """Advance all assumptions, retaining proof provenance and invalidating overlap."""
    with store.transaction():
        _revision(store, new_revision)
        changed = _paths(store, changed_paths, check_scope=False)
        updated = []
        for old in store.list_records('assumption'):
            data = old['data']
            if data['revision'] == new_revision:
                continue
            affected = any(_overlaps(path, change) for path in data['affected_paths'] for change in changed)
            trace = dict(source_revision=data['revision'], revision=new_revision, status=data['status'], evidence=data['evidence'], changed_paths=changed, action='invalidated' if affected else 'carried')
            data = dict(data, revision=new_revision, history=data['history'] + [trace])
            if affected:
                data.update(status='invalidated', evidence=[])
                data.pop('verified_by', None)
            updated.append(store.put_record('assumption', old['id'], data, event='assumption.' + trace['action'], expected_version=old['version']))
        return updated


def assumption_gaps(store):
    revision = store.get_meta('revision')
    gaps = []
    for record in store.list_records('assumption'):
        data = record['data']
        if not data['blocks_approval']:
            continue
        if data['revision'] != revision or data['status'] != 'verified' or not data['evidence']:
            reason = 'Stale assumption: ' if data['revision'] != revision else 'Unverified assumption: '
            gaps.append(dict(kind='assumption', id=record['id'], reason=reason + data['statement']))
    return gaps
