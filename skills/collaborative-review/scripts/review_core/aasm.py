"""Strict, declarative AASM syntax and typed operation validation."""
import copy
import json
import re

BASE_DIALECT = 'aasm-1'
MAX_DOCUMENT_BYTES = 1024 * 1024
MAX_LINE_BYTES = 65536
MAX_COMMANDS = 4096
ID_PATTERN = re.compile(r'[A-Za-z0-9][A-Za-z0-9_.:-]*\Z')
OP_PATTERN = re.compile(r'[A-Z][A-Z0-9_]*\Z')
KEY_PATTERN = re.compile(r'[a-z][a-z0-9_]*\Z')
TYPES = {'text', 'id', 'path', 'location', 'strings', 'ids', 'paths', 'boolean', 'integer', 'enum'}


def _field(kind, required=True, **options):
    return dict(type=kind, required=required, **options)


def _schema(meaning, **fields):
    return dict(meaning=meaning, fields=fields)


FINDING_FIELDS = dict(id=_field('id'), at=_field('location'), claim=_field('text'), fix=_field('text'), evidence=_field('strings', nonempty=True))
BUILTIN_OPERATIONS = {
    op: _schema(meaning, **FINDING_FIELDS, severity=_field('enum', False, enum=severities, default=default))
    for op, meaning, severities, default in (
        ('B', 'Verified blocking finding.', ['critical', 'high', 'medium', 'low'], 'high'),
        ('D', 'Nonblocking finding detail.', ['medium', 'low'], 'medium'),
        ('O', 'Optional nonblocking finding.', ['medium', 'low'], 'low'))
}
BUILTIN_OPERATIONS.update({
    'FIX': _schema('Resolve finding with evidence.', id=_field('id'), evidence=_field('strings', nonempty=True)),
    'REOPEN': _schema('Reopen finding.', id=_field('id'), reason=_field('text'), evidence=_field('strings', nonempty=True)),
    'DISPUTE': _schema('Create blocking question linked to finding.', id=_field('id'), finding=_field('id'), reason=_field('text'), evidence=_field('strings', nonempty=True)),
    'POINT': _schema('Open one disputed point for explicit agreement by both leads.', id=_field('id'), text=_field('text'), paths=_field('paths', nonempty=True), evidence=_field('strings', nonempty=True), refs=_field('ids', False, default=[])),
    'PROPOSE': _schema('Propose a resolution for one point without approving it.', id=_field('id'), digest=_field('text'), text=_field('text'), evidence=_field('strings', nonempty=True), refs=_field('ids', False, default=[])),
    'AGREE': _schema('Approve exactly one point resolution and evidence digest.', id=_field('id'), digest=_field('text'), evidence=_field('strings', nonempty=True)),
    'CONTEST': _schema('Challenge one point and require a revised resolution.', id=_field('id'), digest=_field('text'), text=_field('text'), evidence=_field('strings', nonempty=True), refs=_field('ids', False, default=[])),
    'Q': _schema('Create question.', id=_field('id'), text=_field('text'), findings=_field('ids', False, default=[]), blocking=_field('boolean', False, default=True)),
    'A': _schema('Answer question.', id=_field('id'), question=_field('id'), text=_field('text'), evidence=_field('strings', nonempty=True), option=_field('id', False)),
    'OPT': _schema('Create option.', id=_field('id'), title=_field('text'), description=_field('text'), findings=_field('ids', False, default=[])),
    'COV': _schema('Record coverage.', id=_field('id'), paths=_field('paths', nonempty=True), status=_field('enum', enum=['reviewed', 'partial', 'unreviewed']), evidence=_field('strings', nonempty=True), findings=_field('ids', False, default=[])),
    'VOTE': _schema('Record task verdict with coverage references.', verdict=_field('enum', enum=['APPROVED', 'BLOCKED']), evidence=_field('ids', nonempty=True)),
    'NOTE': _schema('Information without approval effect.', text=_field('text'), refs=_field('ids', False, default=[])),
    'SRC': _schema('Capture source.', id=_field('id'), locator=_field('text'), date=_field('text'), content=_field('text'), excerpt=_field('text', False)),
    'F': _schema('Unverified research claim.', id=_field('id'), text=_field('text'), sources=_field('ids'), impact=_field('enum', False, enum=['decision-changing', 'not-decision-changing'], default='decision-changing'), inference=_field('boolean', False, default=False), contradictions=_field('ids', False, default=[])),
    'CONFIRM': _schema('Independently verify claim.', id=_field('id'), evidence=_field('strings', nonempty=True), sources=_field('ids', False), contradictions=_field('ids', False)),
    'GAP': _schema('Blocking question about missing evidence.', id=_field('id'), text=_field('text')),
    'REC': _schema('Recommendation.', id=_field('id'), text=_field('text'), claims=_field('ids', nonempty=True), case=_field('text')),
    'RETIRE': _schema('Retire a refuted claim or abandoned recommendation with audit evidence.', kind=_field('enum', enum=['claim', 'recommendation']), id=_field('id'), evidence=_field('strings', nonempty=True), replacement=_field('id', False)),
    'ASSUME': _schema('Record assumption.', id=_field('id'), text=_field('text'), paths=_field('paths', nonempty=True), blocking=_field('boolean', False, default=True)),
    'VERIFY': _schema('Verify assumption.', id=_field('id'), evidence=_field('strings', nonempty=True)),
    'IMP': _schema('Propose future improvement without authorization.', proposal=_field('id'), text=_field('text')),
})


def _path(value):
    if not isinstance(value, str) or not value or value.startswith('/') or any(part in ('', '.', '..') for part in value.split('/')) or any(char in value for char in '\\:') or any(ord(char) < 32 or ord(char) == 127 for char in value):
        raise ValueError('expected canonical relative path')


def _value(value, field):
    kind = field['type']
    if isinstance(value, str):
        try:
            value.encode('utf-8')
        except UnicodeEncodeError as error:
            raise ValueError('expected valid UTF-8 string') from error
    if kind in ('strings', 'ids', 'paths'):
        if not isinstance(value, list) or (field.get('nonempty') and not value):
            raise ValueError('expected ' + kind + (' nonempty' if field.get('nonempty') else ''))
        item_type = {'strings': 'text', 'ids': 'id', 'paths': 'path'}[kind]
        for item in value:
            _value(item, {'type': item_type})
    elif kind == 'boolean':
        if type(value) is not bool:
            raise ValueError('expected boolean')
    elif kind == 'integer':
        if type(value) is not int or value <= 0:
            raise ValueError('expected positive integer')
    elif kind == 'enum':
        if not isinstance(value, str) or value not in field['enum']:
            raise ValueError('expected enum member')
    elif kind == 'path':
        _path(value)
    elif kind == 'location':
        if not isinstance(value, str):
            raise ValueError('expected location')
        match = re.fullmatch(r'(.+):([1-9][0-9]*)(?:-([1-9][0-9]*))?', value)
        if not match or (match[3] and int(match[3]) < int(match[2])):
            raise ValueError('expected path:positive_line or path:start-end')
        _path(match[1])
    elif not isinstance(value, str) or not value.strip() or (kind == 'id' and not ID_PATTERN.fullmatch(value)):
        raise ValueError('expected ' + kind)
    return copy.deepcopy(value)


def _arguments(args, schema):
    if not isinstance(args, dict):
        raise ValueError('arguments must be object')
    fields = schema['fields']
    if set(args) - set(fields):
        raise ValueError('unknown arguments: ' + ', '.join(sorted(set(args) - set(fields))))
    result = {}
    for name, field in fields.items():
        if name in args:
            try:
                result[name] = _value(args[name], field)
            except ValueError as error:
                raise ValueError(f'{name}: {error}') from error
        elif field.get('required', True):
            raise ValueError('missing argument: ' + name)
        elif 'default' in field:
            result[name] = copy.deepcopy(field['default'])
    return result


def validate_definitions(definitions):
    """Validate extension data; extensions can only map to builtins."""
    if not isinstance(definitions, list):
        raise ValueError('definitions must be a list')
    result = copy.deepcopy(definitions)
    names = set(BUILTIN_OPERATIONS)
    for definition in result:
        if not isinstance(definition, dict) or set(definition) - {'op', 'meaning', 'fields', 'expands_to'} or not {'op', 'meaning', 'fields'} <= set(definition):
            raise ValueError('invalid extension definition')
        op = definition['op']
        if not isinstance(op, str) or not OP_PATTERN.fullmatch(op) or op in names:
            raise ValueError('invalid or duplicate extension opcode')
        names.add(op)
        _value(definition['meaning'], {'type': 'text'})
        fields = definition['fields']
        if not isinstance(fields, dict):
            raise ValueError('fields must be object')
        for name, field in fields.items():
            if not isinstance(name, str) or not KEY_PATTERN.fullmatch(name) or not isinstance(field, dict) or set(field) - {'type', 'required', 'enum'}:
                raise ValueError('invalid extension field')
            if not isinstance(field.get('type'), str) or field['type'] not in TYPES or type(field.get('required')) is not bool:
                raise ValueError('field requires known type and required boolean')
            if field['type'] == 'enum':
                choices = field.get('enum')
                if not isinstance(choices, list) or not choices or any(not isinstance(item, str) or not item.strip() for item in choices) or len(set(choices)) != len(choices):
                    raise ValueError('enum requires unique nonempty strings')
            elif 'enum' in field:
                raise ValueError('enum choices require enum type')
        if 'expands_to' in definition:
            expansion = definition['expands_to']
            if not isinstance(expansion, dict) or set(expansion) != {'op', 'args'} or not isinstance(expansion['op'], str) or expansion['op'] not in BUILTIN_OPERATIONS or not isinstance(expansion['args'], dict):
                raise ValueError('expansion must target builtin')
            target = BUILTIN_OPERATIONS[expansion['op']]['fields']
            mappings = expansion['args']
            if set(mappings) - set(target) or any(field.get('required', True) and name not in mappings for name, field in target.items()):
                raise ValueError('expansion arguments do not match builtin')
            for name, mapping in mappings.items():
                if not isinstance(mapping, dict) or len(mapping) != 1 or not set(mapping) <= {'field', 'value'}:
                    raise ValueError('invalid expansion mapping')
                if 'value' in mapping:
                    _value(mapping['value'], target[name])
                    continue
                source = fields.get(mapping['field']) if isinstance(mapping['field'], str) else None
                if source is None or source['type'] != target[name]['type'] or (target[name].get('required', True) and not source['required']):
                    raise ValueError('incompatible expansion field')
                if source['type'] == 'enum' and not set(source['enum']) <= set(target[name]['enum']):
                    raise ValueError('incompatible expansion enum')
    return result


def _schemas(definitions):
    return dict(BUILTIN_OPERATIONS, **{item['op']: item for item in validate_definitions([] if definitions is None else definitions)})


def _pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate JSON key: ' + key)
        result[key] = value
    return result


def _constant(value):
    raise ValueError('nonfinite JSON number: ' + value)


DECODER = json.JSONDecoder(object_pairs_hook=_pairs, parse_constant=_constant)


def _canonical_line(op, args):
    return op + ''.join(' ' + name + '=' + json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(',', ':'), sort_keys=True) for name, value in args.items())


def parse_document(text, definitions=None):
    """Parse one strict named-operand command per line, with explicit defaults."""
    if not isinstance(text, str):
        raise ValueError('line 1: document is not text')
    try:
        document_bytes = len(text.encode('utf-8'))
    except UnicodeEncodeError as error:
        line_number = text[:error.start].count('\n') + 1
        raise ValueError(f'line {line_number}: expected valid UTF-8 text') from error
    if document_bytes > MAX_DOCUMENT_BYTES:
        raise ValueError('line 1: document exceeds byte limit')
    schemas = _schemas(definitions)
    operations = []
    normalized_bytes = 0
    for line_number, line in enumerate(text.split('\n'), 1):
        try:
            if len(line.encode('utf-8')) > MAX_LINE_BYTES:
                raise ValueError('line exceeds byte limit')
            if not line.strip():
                continue
            if len(operations) >= MAX_COMMANDS:
                raise ValueError('command count exceeds limit')
            match = re.match(r'([A-Z][A-Z0-9_]*)(?=\s|$)', line)
            if not match or match[1] not in schemas:
                raise ValueError('unknown or malformed opcode')
            op = match[1]
            position = match.end()
            args = {}
            while position < len(line):
                whitespace = re.match(r'[ \t]+', line[position:])
                if not whitespace:
                    raise ValueError('expected operand separator')
                position += whitespace.end()
                if position == len(line):
                    break
                operand = re.match(r'([a-z][a-z0-9_]*)=', line[position:])
                if not operand:
                    raise ValueError('expected named operand key=value')
                key = operand[1]
                position += operand.end()
                if key in args:
                    raise ValueError('duplicate argument: ' + key)
                if position == len(line):
                    raise ValueError('missing operand value')
                if line[position] in '"[{':
                    value, end = DECODER.raw_decode(line, position)
                    position = end
                else:
                    bare = re.match(r"[^\s\"'\[\]{}=\\]+", line[position:])
                    if not bare:
                        raise ValueError('invalid bare identifier; quote text')
                    token = bare[0]
                    if token in ('NaN', 'Infinity', '-Infinity'):
                        raise ValueError('nonfinite number')
                    try:
                        value = json.loads(token, parse_constant=_constant)
                    except json.JSONDecodeError:
                        value = token
                    position += bare.end()
                args[key] = value
            args = _arguments(args, schemas[op])
            line_bytes = len(_canonical_line(op, args).encode('utf-8'))
            if line_bytes > MAX_LINE_BYTES:
                raise ValueError('normalized line exceeds byte limit')
            normalized_bytes += line_bytes + bool(operations)
            if normalized_bytes > MAX_DOCUMENT_BYTES:
                raise ValueError('normalized document exceeds byte limit')
            operations.append({'op': op, 'args': args})
        except (ValueError, RecursionError) as error:
            raise ValueError(f'line {line_number}: {error}') from error
    return operations


def format_document(operations, definitions=None):
    """Validate AST and serialize canonical text, including explicit defaults."""
    schemas = _schemas(definitions)
    if not isinstance(operations, list):
        raise ValueError('operations must be a list')
    normalized = []
    lines = []
    for operation in operations:
        if not isinstance(operation, dict) or set(operation) != {'op', 'args'} or not isinstance(operation['op'], str) or operation['op'] not in schemas:
            raise ValueError('invalid operation AST')
        op = operation['op']
        args = _arguments(operation['args'], schemas[op])
        normalized.append(dict(op=op, args=args))
        lines.append(_canonical_line(op, args))
    document = '\n'.join(lines)
    if parse_document(document, definitions) != normalized:
        raise ValueError('serialization roundtrip mismatch')
    return document


def expand_operations(operations, definitions=None):
    """Expand declared aliases; preserve typed domain messages unchanged."""
    extensions = {item['op']: item for item in validate_definitions([] if definitions is None else definitions)}
    normalized = parse_document(format_document(operations, definitions), definitions)
    result = []
    for operation in normalized:
        expansion = extensions.get(operation['op'], {}).get('expands_to')
        if not expansion:
            result.append(operation)
            continue
        args = {}
        for name, mapping in expansion['args'].items():
            if 'value' in mapping:
                args[name] = mapping['value']
            elif mapping['field'] in operation['args']:
                args[name] = operation['args'][mapping['field']]
        result.append(dict(op=expansion['op'], args=_arguments(args, BUILTIN_OPERATIONS[expansion['op']])))
    return result
