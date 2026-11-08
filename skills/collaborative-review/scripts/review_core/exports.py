"""Generated, read-only views of structured review records and immutable events."""
import hashlib
import html
import json
import os
from pathlib import Path
import tempfile


RECORD_KINDS = ('option', 'coverage', 'question', 'answer', 'finding', 'review',
                'assumption', 'source', 'claim', 'recommendation', 'task',
                'authorization', 'authorization_revocation', 'message', 'dialect',
                'dialect_test', 'dialect_ack', 'aasm_note', 'aasm_domain', 'protocol_proposal',
                'consensus_point', 'point_approval')
EXPORT_NAMES = ('OPTIONS.md', 'COVERAGE.md', 'DECISIONS.md', 'SUMMARY.md', 'TRANSCRIPT.html')


def _md(value):
    """Encode markup punctuation so all supplied text remains inert Markdown."""
    if isinstance(value, (dict, list)):
        value = json.dumps(value, ensure_ascii=False, sort_keys=True)
    text = ' '.join(str(value).split())
    return ''.join('&#' + str(ord(char)) + ';' if char in '&<>\\`*_{}[]()#+!|~' else char for char in text)


def _ref(kind, record_id, records):
    label = _md(record_id)
    if not any(row['id'] == record_id for row in records[kind]):
        return label + ' (missing)'
    return '[' + label + '](TRANSCRIPT.html#' + _anchor(kind, record_id) + ')'


def _refs(kind, ids, records):
    return ', '.join(_ref(kind, record_id, records) for record_id in ids) or 'None'


def _table(headers, rows):
    lines = ['| ' + ' | '.join(headers) + ' |', '| ' + ' | '.join('---' for _ in headers) + ' |']
    lines.extend('| ' + ' | '.join(row) + ' |' for row in rows)
    return '\n'.join(lines) + '\n' if len(lines) > 2 else 'None.\n'


def _groups(records, revision):
    return [('Current revision', [row for row in records if row['data'].get('revision') == revision]),
            ('Historical revisions', [row for row in records if row['data'].get('revision') != revision])]


def _options(records, revision):
    lines = ['# Options', '']
    selected = {row['data'].get('answer_id') for row in records['question']
                if row['data'].get('status') == 'answered'}
    for label, options in _groups(records['option'], revision):
        lines.extend(['## ' + label, ''])
        rows = []
        for row in options:
            data = row['data']
            answers = [answer['id'] for answer in records['answer'] if answer['id'] in selected
                       and answer['data'].get('option_id') == row['id']
                       and answer['data'].get('revision') == data.get('revision')]
            refs = _refs('answer', answers, records) if answers else _refs('finding', data.get('finding_ids', []), records)
            rows.append([_ref('option', row['id'], records), _md(data.get('title', '')),
                         _md(data.get('author', '')), 'selected' if answers else 'not selected',
                         refs, _md(data.get('revision', '')), _md(data.get('description', ''))])
        lines.extend([_table(['ID', 'Title', 'Author', 'Selection', 'Answer or finding references', 'Revision', 'Description'], rows), ''])
    return '\n'.join(lines)


def _coverage(records, revision):
    lines = ['# Coverage', '']
    for label, coverage in _groups(records['coverage'], revision):
        rows = []
        for row in coverage:
            data = row['data']
            for path in data.get('paths', []):
                rows.append([_md(path), _md(data.get('status', '')), _md(data.get('author', '')),
                             _md(data.get('evidence', [])), _refs('finding', data.get('finding_ids', []), records),
                             _md(data.get('revision', ''))])
        lines.extend(['## ' + label, '', _table(['Path', 'Status', 'Author', 'Evidence', 'Findings', 'Revision'], rows), ''])
    return '\n'.join(lines)


def _research_lifecycle(kind, data, records):
    if data.get('status') != 'retired':
        return 'active'
    replacement = _refs(kind, [data['replacement_id']] if data.get('replacement_id') else [], records)
    return ('retired by ' + _md(data.get('retired_by', '')) + '; evidence: ' +
            _md(data.get('retirement_evidence', [])) + '; replacement: ' + replacement)


def _summary(records, revision, approval, point_states):
    lines = ['# Review summary', '', 'Current captured revision: ' + _md(revision), '',
             'Snapshot approval: **' + _md(approval.get('result', 'UNKNOWN')) + '**', '', '## Approval gaps', '']
    lines.extend('- ' + _md(gap) for gap in approval.get('blockers', []))
    if not approval.get('blockers'):
        lines.append('None recorded.')
    for label, current in (('Current revision', True), ('Historical revisions', False)):
        lines.extend(['', '## ' + label, ''])
        def rows(kind):
            return [row for row in records[kind] if (row['data'].get('revision') == revision) == current]
        lines.extend(['### Recommendations', ''])
        for row in rows('recommendation'):
            data = row['data']
            lines.extend(['- ' + _ref('recommendation', row['id'], records) + ': ' + _md(data.get('text', '')),
                          '  Status: ' + _research_lifecycle('recommendation', data, records),
                          '  Case basis: ' + _md(data.get('case_basis', '')) + '; claims: ' +
                          _refs('claim', data.get('claim_ids', []), records) + '; revision: ' + _md(data.get('revision', ''))])
        if not rows('recommendation'):
            lines.append('None.')
        lines.extend(['', '### Agreement on disputed points', '', _table(
            ['Point', 'Disagreement', 'Resolution', 'Version', 'Status', 'Approved by', 'Evidence digest'], [
                [_ref('consensus_point', row['id'], records), _md(row['data']['text']),
                 _md((row['data'].get('proposal') or {}).get('text', 'Not proposed')),
                 _md(row['data']['content_version']), _md(point_states.get(row['id'], {}).get('status', 'historical')),
                 _md(point_states.get(row['id'], {}).get('approved_by', [])),
                 _md(point_states.get(row['id'], {}).get('digest', ''))]
                for row in rows('consensus_point')])])
        lines.extend(['', '### Claims', '', _table(['ID', 'Claim', 'Status', 'Verification', 'Inference', 'Contradictions', 'Sources', 'Revision'], [
            [_ref('claim', row['id'], records), _md(row['data'].get('text', '')),
             _research_lifecycle('claim', row['data'], records),
             _md(row['data'].get('verification', 'unverified')), _md(row['data'].get('inference', False)),
             _refs('claim', row['data'].get('contradictions', []), records),
             _refs('source', row['data'].get('source_ids', []), records), _md(row['data'].get('revision', ''))]
            for row in rows('claim')])])
        lines.extend(['', '### Source provenance', '', _table(['ID', 'Locator', 'Access date', 'SHA256', 'Excerpt', 'Revision'], [
            [_ref('source', row['id'], records), _md(row['data'].get('locator', '')),
             _md(row['data'].get('access_date', '')), _md(row['data'].get('sha256', '')),
             _md(row['data'].get('excerpt', '')), _md(row['data'].get('revision', ''))]
            for row in rows('source')])])
        lines.extend(['', '### Findings', '', _table(['ID', 'Paths', 'Locations', 'Claim', 'Evidence', 'Severity', 'Verified', 'Status', 'Proposed fix', 'Revision'], [
            [_ref('finding', row['id'], records)] + [_md(row['data'].get(field, '')) for field in
             ('paths', 'locations', 'claim', 'evidence', 'severity', 'verified', 'status', 'proposed_fix', 'revision')] for row in rows('finding')])])
        lines.extend(['', '### Questions', '', _table(['ID', 'Question', 'Status', 'Answer', 'Revision'], [
            [_ref('question', row['id'], records), _md(row['data'].get('text', '')), _md(row['data'].get('status', '')),
             _refs('answer', [row['data']['answer_id']] if row['data'].get('answer_id') else [], records),
             _md(row['data'].get('revision', ''))] for row in rows('question')])])
        lines.extend(['', '### Review methods and limitations', '', _table(
            ['ID', 'Reviewer', 'Paths', 'Coverage status', 'Recorded checks, results and limitations', 'Revision'], [
                [_ref('coverage', row['id'], records)] + [_md(row['data'].get(field, '')) for field in
                 ('author', 'paths', 'status', 'evidence', 'revision')] for row in rows('coverage')])])
        lines.extend(['', '### Reviews', '', _table(['Reviewer', 'Verdict', 'Revision'], [
            [_md(row['data'].get('reviewer', '')), _md(row['data'].get('verdict', '')),
             _md(row['data'].get('revision', ''))] for row in rows('review')])])
    lines.extend(['', '## Options', '', 'See [OPTIONS.md](OPTIONS.md) for selections and their answer references.'])
    return '\n'.join(lines) + '\n'


def _anchor(kind, record_id):
    return 'record-' + hashlib.sha256((kind + '\0' + record_id).encode()).hexdigest()


def _transcript(records, events, approval):
    targets = {(kind, row['id']): _anchor(kind, row['id']) for kind in records
               for row in records[kind]}

    fields = {'question_id': 'question', 'answer_id': 'answer',
              'finding_id': 'finding', 'finding_ids': 'finding', 'reply_to': 'message',
              'dialect_id': 'dialect', 'task_id': 'task', 'source_ids': 'source', 'claim_ids': 'claim',
              'point_id': 'consensus_point'}

    def render(value, field=None):
        if isinstance(value, dict):
            return '<dl>' + ''.join('<dt>' + html.escape(str(key)) + '</dt><dd>' + render(item, key) + '</dd>'
                                   for key, item in value.items()) + '</dl>'
        if isinstance(value, list):
            return '<ul>' + ''.join('<li>' + render(item, field) + '</li>' for item in value) + '</ul>'
        text = str(value)
        escaped = html.escape(text, quote=True)
        if field == 'body':
            return '<pre>' + escaped + '</pre>'
        target = targets.get((fields.get(field), text))
        if isinstance(value, str) and target:
            return '<a href="#' + target + '">' + escaped + '</a>'
        return escaped

    body = ['<!doctype html><html lang="en"><meta charset="utf-8">',
            '<meta name="viewport" content="width=device-width,initial-scale=1">',
            '<title>Review transcript</title><body><h1>Review transcript</h1>',
            '<h2>Current snapshot approval</h2>', render(approval)]
    for kind, rows in records.items():
        body.append('<h2>' + html.escape(kind) + '</h2>')
        for row in rows:
            body.append('<article id="' + _anchor(kind, row['id']) + '"><h3>' +
                        html.escape(row['id']) + '</h3>' + render(row) + '</article>')
    body.extend(['<h2>Immutable events</h2>', render(events), '</body></html>'])
    return '\n'.join(body)


def _destination(store):
    run = Path(store.run_dir).absolute()
    # Do not follow symlinks in any ancestor, including broken links.
    for path in (run, *run.parents):
        if path.is_symlink():
            raise ValueError('Export run path must not contain symlinks')
    root = run / 'exports'
    if root.is_symlink():
        raise ValueError('Export directory must not be a symlink')
    if root.exists() and not root.is_dir():
        raise ValueError('Export directory must be a directory')
    for name in EXPORT_NAMES:
        target = root / name
        if target.is_symlink() or (target.exists() and not target.is_file()):
            raise ValueError('Unsafe export destination: ' + name)
    root.mkdir(exist_ok=True)
    return root


def _write(root, name, content):
    target = root / name
    if root.is_symlink() or target.is_symlink():
        raise ValueError('Unsafe export destination')
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=root, delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


def export_run(store, approval=None):
    """Return {filename: absolute path}; optional approval supplies a caller check result."""
    with store.read_transaction():
        if approval is None:
            from .cli import readiness
            approval = readiness(store)
        revision = store.get_meta('revision')
        records = {kind: store.list_records(kind) for kind in RECORD_KINDS}
        from .consensus import point_state
        point_states = {}
        for point in records['consensus_point']:
            if point['data']['revision'] == revision:
                try:
                    point_states[point['id']] = point_state(store, point['id'])
                except ValueError as error:
                    point_states[point['id']] = dict(status='blocked: ' + str(error))
        events = store.events()
    decisions = [event for event in events if event['kind'].startswith('authorization.') or
                 event['kind'] in ('task.recovery_resolved', 'task.process_termination_confirmed', 'run.recovered')]
    content = {
        'OPTIONS.md': _options(records, revision),
        'COVERAGE.md': _coverage(records, revision),
        'DECISIONS.md': '# Recorded authorization and recovery events\n\n' + _table(
            ['Sequence', 'Event', 'Actor', 'Record', 'Recorded details'],
            [[_md(event['seq']), _md(event['kind']), _md(event['actor']), _md(event['record_id']),
              _md(event['payload'])] for event in decisions]),
        'SUMMARY.md': _summary(records, revision, approval, point_states),
        'TRANSCRIPT.html': _transcript(records, events, approval),
    }
    root = _destination(store)
    for name, text in content.items():
        _write(root, name, text)
    return {name: str(root / name) for name in EXPORT_NAMES}
