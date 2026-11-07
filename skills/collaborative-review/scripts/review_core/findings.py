"""Typed, revision-bound review records; prose is never parsed for state."""
from pathlib import PurePosixPath
import re
import uuid


SEVERITIES = ('critical', 'high', 'medium', 'low', 'info')
FINDING_STATUSES = ('open', 'resolved', 'dismissed')
COVERAGE_STATUSES = ('reviewed', 'partial', 'unreviewed')


def _text(value, label):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f'{label} must be a nonempty string')
    return value


def _id(value, kind):
    value = value if value is not None else f'{kind}-{uuid.uuid4().hex}'
    if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.:-]*', value):
        raise ValueError('Invalid record ID')
    return value


def _revision(store, revision):
    _text(revision, 'revision')
    if revision != store.get_meta('revision'):
        raise ValueError('revision must match the current run revision')
    return revision


def _strings(values, label):
    if not isinstance(values, (list, tuple)):
        raise ValueError(f'{label} must be a list of strings')
    return list(dict.fromkeys(_text(value, label) for value in values))


def _paths(store, paths):
    paths = _strings(paths, 'paths')
    manifest = store.get_meta('snapshot', {})
    files = manifest.get('files', []) if isinstance(manifest, dict) else []
    known = {entry['path'] for entry in files if isinstance(entry, dict) and 'path' in entry}
    for path in paths:
        parts = PurePosixPath(path).parts
        if '\x00' in path or '\\' in path or ':' in path or path.startswith('/') or '..' in parts or path != str(PurePosixPath(path)) or path == '.':
            raise ValueError('paths must be canonical relative file paths')
        if isinstance(manifest, dict) and 'files' in manifest and path not in known:
            raise ValueError(f'Path is outside snapshot: {path}')
    return paths


def _refs(store, kind, ids, revision):
    ids = _strings(ids, f'{kind} IDs')
    for record_id in ids:
        record = store.get_record(kind, record_id)
        if record is None or record['data']['revision'] != revision:
            raise ValueError(f'Unknown or stale {kind}: {record_id}')
    return ids


def _bool(value, label):
    if type(value) is not bool:
        raise ValueError(f'{label} must be boolean')
    return value


def _create(store, kind, record_id, data):
    record_id = _id(record_id, kind)
    with store.transaction():
        old = store.get_record(kind, record_id)
        if old:
            if old['data'] != data:
                raise ValueError(f'{kind} ID already has different data')
            return old
        return store.put_record(kind, record_id, data, actor=data['author'], event=f'{kind}.created', expected_version=0)


def _existing(store, kind, record_id):
    record = store.get_record(kind, _id(record_id, kind))
    if record is None:
        raise ValueError(f'Unknown {kind}: {record_id}')
    _revision(store, record['data']['revision'])
    return record


def _locations(store, locations, paths):
    locations = _strings(locations, 'locations')
    for location in locations:
        match = re.fullmatch(r'(.+):([1-9][0-9]*)(?:-([1-9][0-9]*))?', location)
        if not match or (match[3] is not None and int(match[3]) < int(match[2])):
            raise ValueError('Invalid finding location')
        if _paths(store, [match[1]])[0] not in paths:
            raise ValueError('Finding location must belong to finding paths')
    return locations


def _finding(store, data):
    _text(data['author'], 'author')
    _revision(store, data['revision'])
    _text(data['claim'], 'claim')
    _text(data['proposed_fix'], 'proposed_fix')
    data['paths'] = _paths(store, data['paths'])
    if 'locations' in data:
        data['locations'] = _locations(store, data['locations'], data['paths'])
    data['evidence'] = _strings(data['evidence'], 'evidence')
    data['resolution_evidence'] = _strings(data['resolution_evidence'], 'resolution_evidence')
    _bool(data['verified'], 'verified')
    _bool(data['blocking'], 'blocking')
    if data['severity'] not in SEVERITIES or data['status'] not in FINDING_STATUSES:
        raise ValueError('Invalid finding severity or status')
    if data['verified'] and not data['evidence']:
        raise ValueError('Verified findings require evidence')
    if data['status'] != 'open' and not data['resolution_evidence']:
        raise ValueError('Closing findings requires resolution evidence')
    return data


def create_finding(store, *, author, revision, claim, proposed_fix, paths=(), evidence=(), severity='medium', verified=False, blocking=True, status='open', resolution_evidence=(), finding_id=None, locations=None):
    data = _finding(store, dict(author=author, revision=revision, claim=claim, proposed_fix=proposed_fix, paths=paths, evidence=evidence, severity=severity, verified=verified, blocking=blocking, status=status, resolution_evidence=resolution_evidence))
    if locations is not None:
        data['locations'] = _locations(store, locations, data['paths'])
    return _create(store, 'finding', finding_id, data)


def update_finding(store, *, finding_id, actor, claim=None, proposed_fix=None, paths=None, evidence=None, severity=None, verified=None, blocking=None, status=None, resolution_evidence=None, expected_version=None):
    _text(actor, 'actor')
    changes = dict(claim=claim, proposed_fix=proposed_fix, paths=paths, evidence=evidence, severity=severity, verified=verified, blocking=blocking, status=status, resolution_evidence=resolution_evidence)
    with store.transaction():
        old = _existing(store, 'finding', finding_id)
        data = {**old['data'], **{key: value for key, value in changes.items() if value is not None}}
        data['paths'] = _paths(store, data['paths'])
        data['evidence'] = _strings(data['evidence'], 'evidence')
        substantive = any(data[key] != old['data'][key] for key in ('claim', 'proposed_fix', 'paths', 'evidence'))
        if substantive:
            # Proof and closure apply to the exact claim and scope they reviewed.
            if evidence is None:
                data['evidence'] = []
            if verified is None:
                data['verified'] = False
            if resolution_evidence is None:
                data['resolution_evidence'] = []
                if status is None:
                    data['status'] = 'open'
        if data['status'] == 'open':
            data['resolution_evidence'] = []
        elif old['data']['status'] != data['status'] and resolution_evidence is None:
            raise ValueError('Closing findings requires explicitly supplied resolution evidence')
        data = _finding(store, data)
        return store.put_record('finding', finding_id, data, actor=actor, event='finding.updated', expected_version=old['version'] if expected_version is None else expected_version)


def create_question(store, *, author, revision, text, finding_ids=(), blocking=True, question_id=None):
    _text(author, 'author')
    _revision(store, revision)
    data = dict(author=author, revision=revision, text=_text(text, 'text'), finding_ids=_refs(store, 'finding', finding_ids, revision), blocking=_bool(blocking, 'blocking'), status='unanswered', answer_id=None, content_version=1)
    return _create(store, 'question', question_id, data)


def answer_question(store, *, question_id, author, revision, text, evidence=(), option_id=None, answer_id=None):
    _text(author, 'author')
    _revision(store, revision)
    with store.transaction():
        question = _existing(store, 'question', question_id)
        if option_id is not None:
            _refs(store, 'option', [option_id], revision)
        data = dict(author=author, revision=revision, question_id=question_id, question_version=question['data']['content_version'], text=_text(text, 'text'), evidence=_strings(evidence, 'evidence'), option_id=option_id)
        answer = _create(store, 'answer', answer_id, data)
        if question['data']['answer_id'] == answer['id']:
            return answer
        if question['data']['status'] == 'answered':
            raise ValueError('Question already answered')
        store.put_record('question', question_id, {**question['data'], 'status': 'answered', 'answer_id': answer['id']}, actor=author, event='question.answered', expected_version=question['version'])
        return answer


def create_option(store, *, author, revision, title, description, finding_ids=(), option_id=None):
    _text(author, 'author')
    _revision(store, revision)
    data = dict(author=author, revision=revision, title=_text(title, 'title'), description=_text(description, 'description'), finding_ids=_refs(store, 'finding', finding_ids, revision))
    return _create(store, 'option', option_id, data)


def create_coverage(store, *, author, revision, paths, status, evidence=(), finding_ids=(), coverage_id=None):
    _text(author, 'author')
    _revision(store, revision)
    paths = _paths(store, paths)
    evidence = _strings(evidence, 'evidence')
    if not paths or status not in COVERAGE_STATUSES:
        raise ValueError('Coverage requires paths and a valid status')
    if status == 'reviewed' and not evidence:
        raise ValueError('Reviewed coverage requires evidence')
    data = dict(author=author, revision=revision, paths=paths, status=status, evidence=evidence, finding_ids=_refs(store, 'finding', finding_ids, revision))
    return _create(store, 'coverage', coverage_id, data)


def finding_gaps(store):
    """Return every current blocking record; never truncate actionable findings."""
    revision = store.get_meta('revision')
    gaps = []
    for record in store.list_records('finding'):
        data = record['data']
        if data['revision'] == revision and data['verified'] and data['blocking'] and data['status'] == 'open':
            gaps.append({'kind': 'finding', 'id': record['id'], 'reason': data['claim']})
    for record in store.list_records('question'):
        data = record['data']
        if data['revision'] == revision and data['blocking'] and data['status'] == 'unanswered':
            gaps.append({'kind': 'question', 'id': record['id'], 'reason': data['text']})
    return gaps


def update_question(store, *, question_id, actor, text=None, finding_ids=None, blocking=None, expected_version=None):
    _text(actor, 'actor')
    with store.transaction():
        old = _existing(store, 'question', question_id)
        data = dict(old['data'])
        if text is not None:
            data['text'] = _text(text, 'text')
        if finding_ids is not None:
            data['finding_ids'] = _refs(store, 'finding', finding_ids, data['revision'])
        if blocking is not None:
            data['blocking'] = _bool(blocking, 'blocking')
        if any(data[key] != old['data'][key] for key in ('text', 'finding_ids', 'blocking')):
            data['status'] = 'unanswered'
            data['answer_id'] = None
            data['content_version'] += 1
        return store.put_record('question', question_id, data, actor=actor, event='question.updated', expected_version=old['version'] if expected_version is None else expected_version)


def update_option(store, *, option_id, actor, title=None, description=None, finding_ids=None, expected_version=None):
    _text(actor, 'actor')
    with store.transaction():
        old = _existing(store, 'option', option_id)
        data = dict(old['data'])
        for key, value in (('title', title), ('description', description)):
            if value is not None:
                data[key] = _text(value, key)
        if finding_ids is not None:
            data['finding_ids'] = _refs(store, 'finding', finding_ids, data['revision'])
        if data != old['data'] and any(answer['data'].get('option_id') == option_id for answer in store.list_records('answer')):
            raise ValueError('Selected options are immutable; create a new option')
        return store.put_record('option', option_id, data, actor=actor, event='option.updated', expected_version=old['version'] if expected_version is None else expected_version)


def update_coverage(store, *, coverage_id, actor, paths=None, status=None, evidence=None, finding_ids=None, expected_version=None):
    _text(actor, 'actor')
    with store.transaction():
        old = _existing(store, 'coverage', coverage_id)
        data = dict(old['data'])
        if paths is not None:
            data['paths'] = _paths(store, paths)
        if status is not None:
            data['status'] = status
        if evidence is not None:
            data['evidence'] = _strings(evidence, 'evidence')
        if finding_ids is not None:
            data['finding_ids'] = _refs(store, 'finding', finding_ids, data['revision'])
        if any(data[key] != old['data'][key] for key in ('paths', 'finding_ids')) and evidence is None:
            data['evidence'] = []
            if status is None:
                data['status'] = 'unreviewed'
        if not data['paths'] or data['status'] not in COVERAGE_STATUSES or (data['status'] == 'reviewed' and not data['evidence']):
            raise ValueError('Invalid coverage or missing review evidence')
        return store.put_record('coverage', coverage_id, data, actor=actor, event='coverage.updated', expected_version=old['version'] if expected_version is None else expected_version)
