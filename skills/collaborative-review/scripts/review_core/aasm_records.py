"""Apply validated AASM data through the revision-bound record APIs."""
import hashlib
from pathlib import Path
import re

from . import assumptions, findings, research
from .aasm import BASE_DIALECT, BUILTIN_OPERATIONS, expand_operations, parse_document
from .dialects import get_dialect
from .snapshots import _read_file, _safe_path


REFERENCE_KINDS = ('finding', 'question', 'answer', 'option', 'coverage', 'source', 'claim',
                   'recommendation', 'assumption', 'message', 'aasm_note', 'aasm_domain',
                   'protocol_proposal', 'review', 'task', 'review_plan', 'authorization',
                   'authorization_revocation')


def _summary(operations):
    votes = [item for item in operations if item['op'] == 'VOTE']
    if votes and (len(votes) != 1 or operations[-1]['op'] != 'VOTE'):
        raise ValueError('VOTE must occur exactly once as the final operation')
    result = {'evidence_ids': votes[0]['args']['evidence'] if votes else [],
              'coverage': [item['args']['id'] for item in operations if item['op'] == 'COV']}
    if votes:
        result['verdict'] = votes[0]['args']['verdict']
    return result


def prepare_result(store, worker, body, dialect_id=BASE_DIALECT):
    """Build a helper-owned result without mutating any records."""
    with store.read_transaction():
        dialect = get_dialect(store, dialect_id, participant=worker)
        parsed = parse_document(body, dialect['definitions'])
        operations = expand_operations(parsed, dialect['definitions'])
        return dict(aasm=body, dialect_id=dialect_id,
                    dialect_digest=dialect['digest'], operations=operations, **_summary(operations))


def _location(store, location):
    match = re.fullmatch(r'(.+):([1-9][0-9]*)(?:-([1-9][0-9]*))?', location)
    if not match:
        raise ValueError('Invalid location')
    path, start, end = match.groups()
    findings._paths(store, [path])
    snapshot = store.get_meta('snapshot', {})
    entry = next((item for item in snapshot.get('files', []) if item['path'] == path), None)
    if entry is None or not snapshot.get('root'):
        raise ValueError('Location requires an immutable snapshot file')
    try:
        content, _ = _read_file(_safe_path(Path(snapshot['root']), path))
    except OSError as error:
        raise ValueError('Snapshot location is unavailable') from error
    if hashlib.sha256(content).hexdigest() != entry.get('sha256') or len(content) != entry.get('size'):
        raise ValueError('Snapshot location content changed')
    last = int(end or start)
    line_count = len(content.splitlines())
    if last < int(start) or last > line_count:
        raise ValueError('Location is outside snapshot line bounds')
    return path


def _note_refs(store, author, revision, refs):
    for record_id in refs:
        matches = []
        for kind in REFERENCE_KINDS:
            row = store.get_record(kind, record_id)
            if row and row['data'].get('revision', row['data'].get('payload', {}).get('revision')) == revision:
                if kind == 'message' and author not in (row['data'].get('sender'), row['data'].get('recipient')):
                    continue
                matches.append(row)
        if len(matches) != 1:
            raise ValueError(f'Unknown, stale or ambiguous reference: {record_id}')


def _record(store, kind, record_id, data, author):
    return findings._create(store, kind, record_id, dict(data, author=author))


def _scope(store, task_id, author, revision, operations, definitions):
    if task_id is None:
        return
    task = store.get_record('task', task_id)
    if not task or task['data'].get('payload', {}).get('revision') != revision or task['data'].get('worker') != author:
        raise ValueError('Unknown, stale or foreign task')
    scope = task['data']['payload'].get('scope_paths', [])
    for operation in operations:
        op, args = operation['op'], operation['args']
        schema = BUILTIN_OPERATIONS.get(op, definitions.get(op))
        paths = []
        for name, field in schema['fields'].items():
            if name not in args:
                continue
            if field['type'] == 'location':
                paths.append(args[name].rsplit(':', 1)[0])
            elif field['type'] == 'path':
                paths.append(args[name])
            elif field['type'] == 'paths':
                paths.extend(args[name])
        finding_ids = list(args.get('findings', [])) if op in ('Q', 'OPT', 'COV') else []
        if op == 'A':
            for kind, record_id in [('question', args['question']), ('option', args.get('option'))]:
                row = store.get_record(kind, record_id) if record_id is not None else None
                if row:
                    finding_ids.extend(row['data'].get('finding_ids', []))
        targets = [('finding', record_id, 'paths') for record_id in finding_ids]
        if op in ('FIX', 'REOPEN', 'DISPUTE'):
            targets.append(('finding', args.get('finding', args.get('id')), 'paths'))
        elif op == 'VERIFY':
            targets = [('assumption', args['id'], 'affected_paths')]
        elif op == 'VOTE':
            targets = [('coverage', record_id, 'paths') for record_id in args['evidence']]
        for kind, record_id, field in targets:
            row = store.get_record(kind, record_id)
            if row:
                paths.extend(row['data'].get(field, []))
        for path in paths:
            if not any(allowed == '.' or path == allowed or path.startswith(allowed + '/') for allowed in scope):
                raise ValueError(f'Path is outside task scope: {path}')


def apply_operations(store, author, revision, operations, task_id=None, allow_vote=False, dialect_id=BASE_DIALECT):
    """Atomically apply records; only an explicit task-result caller may accept VOTE."""
    with store.transaction():
        findings._text(author, 'author')
        findings._revision(store, revision)
        dialect = get_dialect(store, dialect_id, participant=author)
        operations = expand_operations(operations, dialect['definitions'])
        summary = _summary(operations)
        if 'verdict' in summary and not allow_vote:
            raise ValueError('VOTE is only allowed in task results')
        definitions = {item['op']: item for item in dialect['definitions']}
        _scope(store, task_id, author, revision, operations, definitions)
        context = dict(author=author, revision=revision)
        for operation in operations:
            op, args = operation['op'], operation['args']
            if op in ('B', 'D', 'O'):
                path = _location(store, args['at'])
                findings.create_finding(store, **context, finding_id=args['id'], claim=args['claim'],
                                        proposed_fix=args['fix'], paths=[path], locations=[args['at']],
                                        evidence=args['evidence'], severity=args['severity'], verified=True, blocking=op == 'B')
            elif op == 'FIX':
                findings.update_finding(store, finding_id=args['id'], actor=author, status='resolved', resolution_evidence=args['evidence'])
            elif op == 'REOPEN':
                findings.update_finding(store, finding_id=args['id'], actor=author, status='open')
                _record(store, 'aasm_note', None, dict(revision=revision, text=args['reason'], refs=[args['id']], evidence=args['evidence']), author)
            elif op in ('Q', 'GAP', 'DISPUTE'):
                findings.create_question(store, **context, question_id=args['id'],
                                         text=args.get('text', args.get('reason')), finding_ids=[args['finding']] if op == 'DISPUTE' else args.get('findings', []),
                                         blocking=args.get('blocking', True))
                if op == 'DISPUTE':
                    _record(store, 'aasm_note', None, dict(revision=revision, text=args['reason'], refs=[args['id'], args['finding']], evidence=args['evidence']), author)
            elif op == 'A':
                findings.answer_question(store, **context, answer_id=args['id'], question_id=args['question'],
                                         text=args['text'], evidence=args['evidence'], option_id=args.get('option'))
            elif op == 'OPT':
                findings.create_option(store, **context, option_id=args['id'], title=args['title'], description=args['description'], finding_ids=args['findings'])
            elif op == 'COV':
                findings.create_coverage(store, **context, coverage_id=args['id'], paths=args['paths'], status=args['status'], evidence=args['evidence'], finding_ids=args['findings'])
            elif op == 'VOTE':
                findings._refs(store, 'coverage', args['evidence'], revision)
                if args['verdict'] == 'APPROVED':
                    for record_id in args['evidence']:
                        data = store.get_record('coverage', record_id)['data']
                        if data['author'] != author or data['status'] != 'reviewed' or not data['evidence']:
                            raise ValueError('Approval requires reviewed coverage with evidence')
            elif op == 'NOTE':
                _note_refs(store, author, revision, args['refs'])
                _record(store, 'aasm_note', None, dict(revision=revision, **args), author)
            elif op == 'SRC':
                research.add_source(store, author, revision, args['locator'], args['date'], content=args['content'], excerpt=args.get('excerpt'), source_id=args['id'])
            elif op == 'F':
                research.add_claim(store, author, revision, args['text'], source_ids=args['sources'], impact=args['impact'], inference=args['inference'], contradictions=args['contradictions'], claim_id=args['id'])
            elif op == 'CONFIRM':
                research.verify_claim(store, args['id'], author, args['evidence'], source_ids=args.get('sources'), contradictions=args.get('contradictions'))
            elif op == 'REC':
                research.add_recommendation(store, author, revision, args['text'], args['claims'], args['case'], recommendation_id=args['id'])
            elif op == 'ASSUME':
                assumptions.add_assumption(store, **context, assumption_id=args['id'], statement=args['text'], affected_paths=args['paths'], blocks_approval=args['blocking'])
            elif op == 'VERIFY':
                assumptions.verify_assumption(store, **context, assumption_id=args['id'], evidence=args['evidence'])
            elif op == 'IMP':
                _record(store, 'protocol_proposal', args['proposal'], dict(revision=revision, text=args['text'], evidence=[], status='proposed'), author)
            elif op not in BUILTIN_OPERATIONS:
                for name, field in definitions[op]['fields'].items():
                    if name in args and field['type'] == 'location':
                        _location(store, args[name])
                    elif name in args and field['type'] in ('path', 'paths'):
                        findings._paths(store, [args[name]] if field['type'] == 'path' else args[name])
                _record(store, 'aasm_domain', None, dict(revision=revision, operation=operation,
                        definition=definitions[op], dialect_id=dialect_id, dialect_digest=dialect['digest']), author)
        return summary
