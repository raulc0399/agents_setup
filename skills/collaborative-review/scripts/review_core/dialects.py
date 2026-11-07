"""Immutable declarative dialects and participant readback negotiation."""
import hashlib
import json
import re

from .aasm import BASE_DIALECT, validate_definitions, parse_document, expand_operations


_IDENTIFIER = re.compile(r'^[A-Za-z0-9][A-Za-z0-9_.:-]*$')


def _identity(value, label):
    if not isinstance(value, str) or not _IDENTIFIER.fullmatch(value):
        raise ValueError(f'{label} must be a canonical identifier')
    return value


def _json(value):
    try:
        return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False, allow_nan=False)
    except (ValueError, TypeError) as error:
        raise ValueError('Manifest must contain valid JSON') from error


def _digest(value):
    return hashlib.sha256(_json(value).encode('utf-8')).hexdigest()


def _manifest(store, dialect_id):
    record = store.get_record('dialect', dialect_id)
    if record is None:
        raise ValueError(f'Unknown dialect: {dialect_id}')
    return record['data']


def _participant(manifest, participant):
    if participant not in manifest['participants']:
        raise ValueError('Participant is not a member of this dialect')


def _participant_id(dialect_id, participant):
    return _digest([dialect_id, participant])


def _examples(definitions, examples):
    if not isinstance(examples, list) or not examples:
        raise ValueError('Dialect examples must be a nonempty list')
    bodies = set()
    exercised = set()
    for example in examples:
        if not isinstance(example, dict) or set(example) != {'body', 'expected'}:
            raise ValueError('Each example requires only body and expected')
        body = example['body']
        if not isinstance(body, str) or not body.strip() or body in bodies:
            raise ValueError('Example bodies must be nonempty and unique')
        bodies.add(body)
        operations = parse_document(body, definitions)
        exercised.update(item['op'] for item in operations)
        expanded = expand_operations(operations, definitions)
        if not isinstance(example['expected'], list) or _json(expanded) != _json(example['expected']):
            raise ValueError('Example expected meaning does not match expanded AASM')
    if any(definition['op'] not in exercised for definition in definitions):
        raise ValueError('Examples must exercise every extension operation')
    return json.loads(_json(examples))


def propose_dialect(store, author, name, version, participants, definitions, examples, dialect_id=None):
    """Propose immutable data; retrying an identical proposal creates no event."""
    _identity(author, 'Author')
    _identity(name, 'Dialect name')
    if type(version) is not int or version < 1:
        raise ValueError('Dialect version must be a positive integer')
    if not isinstance(participants, list) or len(participants) < 2:
        raise ValueError('Dialect requires at least two participants')
    for participant in participants:
        _identity(participant, 'Participant')
    if len(set(participants)) != len(participants) or author not in participants:
        raise ValueError('Participants must be unique and include the author')
    definitions = validate_definitions(definitions)
    examples = _examples(definitions, examples)
    manifest = {'name': name, 'version': version, 'author': author,
                'participants': sorted(participants), 'definitions': definitions,
                'examples': examples, 'base': BASE_DIALECT}
    manifest['digest'] = _digest(manifest)
    dialect_id = dialect_id or f'{name}:{version}'
    _identity(dialect_id, 'Dialect id')
    if dialect_id == BASE_DIALECT:
        raise ValueError('Cannot redefine the base dialect')
    manifest['id'] = dialect_id
    with store.transaction():
        for record in store.list_records('dialect'):
            old = record['data']
            if record['id'] == dialect_id or (old['name'], old['version']) == (name, version):
                if _json(old) == _json(manifest):
                    return record
                raise ValueError('Dialect name/version and id are immutable; propose a new version')
        return store.put_record('dialect', dialect_id, manifest, actor=author,
                                event='dialect.proposed', expected_version=0)


def test_dialect(store, dialect_id, participant, interpretations):
    """Record a caller's actual expanded readback, including unsuccessful tests."""
    _json(interpretations)
    with store.transaction():
        manifest = _manifest(store, dialect_id)
        _participant(manifest, participant)
        record_id = _participant_id(dialect_id, participant)
        previous = store.get_record('dialect_test', record_id)
        if previous and _json(previous['data']['interpretations']) == _json(interpretations):
            return previous
        if store.get_record('dialect_ack', record_id):
            raise ValueError('Acknowledged readback is immutable')
        errors = []
        try:
            _examples(manifest['definitions'], manifest['examples'])
            if not isinstance(interpretations, list) or len(interpretations) != len(manifest['examples']):
                raise ValueError('Readback must cover the full example set')
            actual = {}
            for item in interpretations:
                if not isinstance(item, dict) or set(item) != {'body', 'expected'}:
                    raise ValueError('Readbacks require only body and expected')
                if not isinstance(item['body'], str) or item['body'] in actual:
                    raise ValueError('Readback bodies must be unique strings')
                actual[item['body']] = item['expected']
            if _json(actual) != _json({item['body']: item['expected'] for item in manifest['examples']}):
                raise ValueError('Readback differs from declared expanded meaning')
        except ValueError as error:
            errors.append(str(error))
        data = {'dialect_id': dialect_id, 'digest': manifest['digest'], 'participant': participant,
                'interpretations': interpretations, 'passed': not errors, 'errors': errors}
        return store.put_record('dialect_test', record_id, data, actor=participant,
                                event='dialect.tested', expected_version=previous['version'] if previous else 0)


def acknowledge_dialect(store, dialect_id, participant):
    """A participant explicitly ACKs only its own successful readback."""
    with store.transaction():
        manifest = _manifest(store, dialect_id)
        _participant(manifest, participant)
        record_id = _participant_id(dialect_id, participant)
        previous = store.get_record('dialect_ack', record_id)
        if previous:
            return previous
        test = store.get_record('dialect_test', record_id)
        if test is None or not test['data']['passed'] or test['data']['digest'] != manifest['digest']:
            raise ValueError('Participant must pass its readback before acknowledgement')
        return store.put_record('dialect_ack', record_id,
                                {'dialect_id': dialect_id, 'digest': manifest['digest'],
                                 'participant': participant, 'test_version': test['version']},
                                actor=participant, event='dialect.acknowledged', expected_version=0)


def dialect_status(store, dialect_id):
    if dialect_id == BASE_DIALECT:
        return {'id': dialect_id, 'status': 'accepted', 'participants': [], 'tested': [],
                'acknowledged': [], 'missing_tests': [], 'missing_acks': []}
    with store.read_transaction():
        manifest = _manifest(store, dialect_id)
        tested = []
        acknowledged = []
        for participant in manifest['participants']:
            record_id = _participant_id(dialect_id, participant)
            test = store.get_record('dialect_test', record_id)
            if test and test['data']['passed'] and test['data']['digest'] == manifest['digest']:
                tested.append(participant)
            ack = store.get_record('dialect_ack', record_id)
            if ack and ack['data']['digest'] == manifest['digest'] and participant in tested:
                acknowledged.append(participant)
        missing_tests = [item for item in manifest['participants'] if item not in tested]
        missing_acks = [item for item in manifest['participants'] if item not in acknowledged]
        status = 'pending' if missing_tests else 'tested' if missing_acks else 'accepted'
        return {'id': dialect_id, 'status': status, 'participants': manifest['participants'],
                'tested': tested, 'acknowledged': acknowledged,
                'missing_tests': missing_tests, 'missing_acks': missing_acks}


def export_dialect(store, dialect_id):
    """Export proposal data for reuse; tests and ACKs never transfer to another run."""
    if dialect_id == BASE_DIALECT:
        raise ValueError('Base dialect does not require an exported proposal')
    return _manifest(store, dialect_id)


def get_dialect(store, dialect_id, participant=None):
    if dialect_id == BASE_DIALECT:
        return {'id': BASE_DIALECT, 'name': BASE_DIALECT, 'version': 1,
                'base': BASE_DIALECT, 'digest': _digest({'base': BASE_DIALECT}),
                'definitions': [], 'examples': [], 'participants': [], 'status': 'accepted'}
    with store.read_transaction():
        manifest = _manifest(store, dialect_id)
        _participant(manifest, participant)
        status = dialect_status(store, dialect_id)
        if status['status'] != 'accepted':
            raise ValueError('Dialect is not accepted by every participant')
        return {**manifest, 'status': 'accepted'}
