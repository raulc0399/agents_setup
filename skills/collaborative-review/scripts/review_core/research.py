"""Captured research provenance and human-attested, revision-bound support."""
from datetime import date
import hashlib
import json
from pathlib import Path
import re
from urllib.parse import urlsplit
import uuid


VERIFICATIONS = ('unverified', 'verified', 'disputed')
IMPACTS = ('decision-changing', 'not-decision-changing')
RESEARCH_KINDS = ('source', 'claim', 'recommendation')
CLAIM_ATTESTATION_FIELDS = ('verification', 'evidence', 'verified_by', 'creation_request')


def research_context(store):
    """Capture common research inputs; verification attestations are review outputs."""
    with store.read_transaction():
        revision = store.get_meta('revision')
        records = {kind: [dict(id=row['id'], data=row['data']) for row in store.list_records(kind)
                          if row['data'].get('revision') == revision] for kind in RESEARCH_KINDS}
        material = {}
        for kind, rows in records.items():
            material[kind] = []
            for row in rows:
                data = {key: value for key, value in row['data'].items() if key != 'creation_request'}
                if kind == 'claim':
                    data = {key: value for key, value in data.items() if key not in CLAIM_ATTESTATION_FIELDS}
                    data['disputed'] = row['data'].get('verification') == 'disputed'
                material[kind].append(dict(id=row['id'], data=data))
            material[kind].sort(key=lambda row: row['id'])
        fingerprint = hashlib.sha256(json.dumps(dict(revision=revision, records=material),
                                     sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        return dict(revision=revision, records=records, fingerprint=fingerprint)


def visible_research_context(store):
    """During blind work expose only the inputs frozen before either team began."""
    from .consensus import blind_pending

    if store.get_meta('approval_scope') != 'report-ready':
        return None
    if blind_pending(store):
        plans = [row for row in store.list_records('review_plan')
                 if row['data']['request']['revision'] == store.get_meta('revision')]
        return plans[0]['data'].get('research_context') if len(plans) == 1 else None
    return research_context(store)


def research_review_gaps(store):
    if store.get_meta('approval_scope') != 'report-ready':
        return []
    current = research_context(store)
    return [dict(kind='research-review', id=row['id'], reviewer=row['data']['reviewer'],
                 reason='Research evidence changed; independently reassess and record the current fingerprint')
            for row in store.list_records('review')
            if row['data']['revision'] == current['revision']
            and row['data'].get('research_fingerprint') != current['fingerprint']]


def _text(value, label):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f'{label} must be a nonempty string')
    return value


def _strings(values, label):
    if not isinstance(values, (tuple, list)):
        raise ValueError(f'{label} must be a list')
    return list(dict.fromkeys(_text(value, label) for value in values))


def _context(store, author, revision):
    _text(author, 'author')
    if _text(revision, 'revision') != store.get_meta('revision'):
        raise ValueError('revision must match current run')


def _refs(store, kind, ids, revision):
    ids = _strings(ids, f'{kind} IDs')
    for record_id in ids:
        record = store.get_record(kind, record_id)
        if record is None or record['data'].get('revision') != revision:
            raise ValueError(f'Unknown or stale {kind}: {record_id}')
    return ids


def _create(store, kind, record_id, data):
    record_id = record_id if record_id is not None else f'{kind}-{uuid.uuid4().hex}'
    if not isinstance(record_id, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.:-]*', record_id):
        raise ValueError('Invalid record ID')
    old = store.get_record(kind, record_id)
    if old:
        old_request = old['data'].get('creation_request', old['data']) if kind in ('claim', 'recommendation') else old['data']
        new_request = data.get('creation_request', data) if kind in ('claim', 'recommendation') else data
        if old_request != new_request:
            raise ValueError(f'{kind} is immutable; use a new ID')
        return old
    return store.put_record(kind, record_id, data, actor=data['author'], event=f'{kind}.created', expected_version=0)


def add_source(store, author, revision, locator, access_date, content=None, file=None, excerpt=None, source_id=None):
    """Capture supplied UTF-8 text; locator provenance is attested, never fetched."""
    _context(store, author, revision)
    _text(locator, 'locator')
    parsed = urlsplit(locator)
    if not ((parsed.scheme in ('https', 'http') and parsed.netloc) or re.fullmatch(r'[^\x00\r\n]+:[1-9][0-9]*(?:-[1-9][0-9]*)?', locator)):
        raise ValueError('locator must be an HTTP URL or file:line')
    try:
        if not isinstance(access_date, str) or date.fromisoformat(access_date).isoformat() != access_date:
            raise ValueError()
    except (ValueError, TypeError):
        raise ValueError('access_date must be YYYY-MM-DD') from None
    if (content is None) == (file is None):
        raise ValueError('Supply exactly one of content or file')
    if file is not None:
        content = Path(_text(file, 'file')).read_bytes().decode('utf-8')
    _text(content, 'content')
    excerpt = content if excerpt is None else _text(excerpt, 'excerpt')
    if excerpt not in content:
        raise ValueError('excerpt must occur in captured content')
    data = dict(author=author, revision=revision, locator=locator, access_date=access_date, content=content,
                sha256=hashlib.sha256(content.encode('utf-8')).hexdigest(), excerpt=excerpt)
    with store.transaction():
        _context(store, author, revision)
        return _create(store, 'source', source_id, data)


def add_claim(store, author, revision, text, source_ids=(), verification='unverified', impact='decision-changing',
              inference=False, contradictions=(), evidence=(), verified_by=None, claim_id=None):
    """Verification records an independent person's attestation, not automated truth."""
    _context(store, author, revision)
    _text(text, 'text')
    if verification not in VERIFICATIONS or impact not in IMPACTS:
        raise ValueError('Invalid verification or impact')
    if type(inference) is not bool:
        raise ValueError('inference must be boolean')
    evidence = _strings(evidence, 'evidence')
    if verified_by is not None:
        _text(verified_by, 'verified_by')
    if verification == 'verified':
        if not evidence or not verified_by:
            raise ValueError('Verified claims require evidence and verified_by')
        if impact == 'decision-changing' and verified_by == author:
            raise ValueError('Decision-changing verification must be independent of author')
    with store.transaction():
        _context(store, author, revision)
        sources = _refs(store, 'source', source_ids, revision)
        conflicts = _refs(store, 'claim', contradictions, revision)
        if claim_id in conflicts:
            raise ValueError('A claim cannot contradict itself')
        if verification == 'verified' and not sources:
            raise ValueError('Verified claims require captured sources')
        data = dict(author=author, revision=revision, text=text, source_ids=sources, verification=verification,
                    impact=impact, inference=inference, label='inference' if inference else ('sourced-claim' if sources else 'assumption'),
                    contradictions=conflicts, evidence=evidence, verified_by=verified_by)
        data['creation_request'] = dict(data)
        return _create(store, 'claim', claim_id, data)


def add_recommendation(store, author, revision, text, claim_ids, case_basis, recommendation_id=None):
    _context(store, author, revision)
    _text(text, 'text')
    _text(case_basis, 'case_basis')
    with store.transaction():
        _context(store, author, revision)
        claims = _refs(store, 'claim', claim_ids, revision)
        if not claims:
            raise ValueError('Recommendations require supporting claims')
        data = dict(author=author, revision=revision, text=text, claim_ids=claims, case_basis=case_basis)
        if recommendation_id is None or store.get_record('recommendation', recommendation_id) is None:
            if any(store.get_record('claim', claim)['data'].get('status') == 'retired' for claim in claims):
                raise ValueError('Recommendations cannot use retired claims')
        return _create(store, 'recommendation', recommendation_id, data)


def _retire(store, kind, record_id, actor, evidence, replacement_id):
    _text(actor, 'actor')
    evidence = _strings(evidence, 'retirement evidence')
    if not evidence:
        raise ValueError('Retirement requires evidence')
    with store.transaction():
        row = store.get_record(kind, _text(record_id, 'record ID'))
        if row is None:
            raise ValueError('Unknown ' + kind)
        data = dict(row['data'])
        _context(store, actor, data['revision'])
        retirement = dict(status='retired', retired_by=actor, retirement_evidence=evidence,
                          replacement_id=replacement_id)
        if data.get('status') == 'retired':
            if any(data.get(key) != value for key, value in retirement.items()):
                raise ValueError('Retirement is immutable')
            return row
        if replacement_id is not None:
            _refs(store, kind, [replacement_id], data['revision'])
            if replacement_id == record_id or store.get_record(kind, replacement_id)['data'].get('status') == 'retired':
                raise ValueError('Replacement must be a different active ' + kind)
        data.setdefault('creation_request', dict(data))
        data.update(retirement)
        return store.put_record(kind, record_id, data, actor=actor, event=kind + '.retired', expected_version=row['version'])


def retire_claim(store, claim_id, actor, evidence, replacement_id=None):
    return _retire(store, 'claim', claim_id, actor, evidence, replacement_id)


def retire_recommendation(store, recommendation_id, actor, evidence, replacement_id=None):
    return _retire(store, 'recommendation', recommendation_id, actor, evidence, replacement_id)


def research_gaps(store):
    """Return all current-revision support gaps, without asserting source truth."""
    revision = store.get_meta('revision')
    claims = {row['id']: row['data'] for row in store.list_records('claim') if row['data'].get('revision') == revision}
    sources = {row['id']: row['data'] for row in store.list_records('source') if row['data'].get('revision') == revision}
    recommendations = [row for row in store.list_records('recommendation')
                       if row['data'].get('revision') == revision and row['data'].get('status') != 'retired']
    gaps = []
    if not recommendations:
        gaps.append({'id': None, 'reason': 'No current recommendation'})
    recommended_claims = {claim_id for row in recommendations for claim_id in row['data'].get('claim_ids', [])}
    for claim_id, claim in claims.items():
        if claim.get('status') == 'retired':
            if claim_id in recommended_claims:
                gaps.append({'id': claim_id, 'reason': 'Active recommendation uses a retired claim'})
            continue
        if claim.get('impact') != 'decision-changing' and claim_id not in recommended_claims:
            continue
        if not claim.get('source_ids') or any(source_id not in sources for source_id in claim['source_ids']):
            gaps.append({'id': claim_id, 'reason': 'Missing current captured source support'})
        if claim.get('verification') != 'verified' or not claim.get('evidence') or not claim.get('verified_by') or claim.get('verified_by') == claim.get('author'):
            gaps.append({'id': claim_id, 'reason': 'Decision-changing claim lacks independent verified support'})
        if claim.get('contradictions') or any(claim_id in other.get('contradictions', []) for other in claims.values()
                                              if other.get('status') != 'retired'):
            gaps.append({'id': claim_id, 'reason': 'Decision-changing claim has unresolved contradictions'})
    for row in recommendations:
        if not row['data'].get('claim_ids') or any(claim_id not in claims for claim_id in row['data']['claim_ids']):
            gaps.append({'id': row['id'], 'reason': 'Recommendation lacks current supporting claims'})
    return gaps


def verify_claim(store, claim_id, actor, evidence, verification='verified', contradictions=None, source_ids=None):
    """Attest support or dispute, optionally resolving explicit contradiction links."""
    _text(actor, 'actor')
    evidence = _strings(evidence, 'evidence')
    if not evidence or verification not in ('verified', 'disputed'):
        raise ValueError('Verification requires evidence and verified/disputed status')
    with store.transaction():
        row = store.get_record('claim', _text(claim_id, 'claim_id'))
        if row is None:
            raise ValueError('Unknown claim')
        data = dict(row['data'])
        _context(store, actor, data['revision'])
        if data.get('status') == 'retired':
            raise ValueError('Cannot verify a retired claim')
        if data['impact'] == 'decision-changing' and actor == data['author']:
            raise ValueError('Decision-changing verification must be independent of author')
        if source_ids is not None:
            data['source_ids'] = _refs(store, 'source', source_ids, data['revision'])
            data['label'] = 'inference' if data['inference'] else ('sourced-claim' if data['source_ids'] else 'assumption')
        if verification == 'verified' and not _refs(store, 'source', data['source_ids'], data['revision']):
            raise ValueError('Verified claims require captured sources')
        if contradictions is not None:
            data['contradictions'] = _refs(store, 'claim', contradictions, data['revision'])
            if claim_id in data['contradictions']:
                raise ValueError('A claim cannot contradict itself')
        data.update(verification=verification, evidence=evidence, verified_by=actor)
        if data == row['data']:
            return row
        return store.put_record('claim', claim_id, data, actor=actor, event='claim.verified', expected_version=row['version'])
