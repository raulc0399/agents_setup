"""Read-only inspection of pinned inputs for trusted local review processes."""

import codecs
import hashlib
import os
from pathlib import Path
import stat

from .authorization import authorize_task, canonical_paths
from .snapshots import verify_snapshot


MAX_FILES = 100
MAX_FILE_BYTES = 256 * 1024
MAX_OUTPUT_BYTES = 256 * 1024
MAX_OUTPUT_LINES = 1000
MAX_LINE_BYTES = 4096
MAX_PATTERN_BYTES = 4096


def _metadata(store):
    metadata = {name: store.get_meta(name) for name in ('snapshot', 'revision', 'project_root', 'paths')}
    if (not isinstance(metadata['snapshot'], dict) or not isinstance(metadata['revision'], str)
            or not isinstance(metadata['project_root'], str) or not metadata['project_root']):
        raise ValueError('run lacks trusted snapshot metadata')
    canonical_paths(metadata['paths'])
    return metadata


def _verify(store, metadata):
    return verify_snapshot(metadata['snapshot'], expected_run_dir=store.run_dir,
                           expected_project_root=metadata['project_root'], expected_paths=metadata['paths'],
                           expected_revision=metadata['revision'])


def verify_checks(store):
    """Return integrity errors after authorizing inspection of the full run scope."""
    metadata = _metadata(store)
    grant = authorize_task(store, 'inspect', metadata['revision'], metadata['paths'])
    errors = _verify(store, metadata)
    return {'revision': metadata['revision'], 'authorization_id': grant, 'valid': not errors, 'errors': errors}


def _covers(scope, path):
    return scope == '.' or scope == path or path.startswith(scope + '/')


def _read(root, entry):
    path = root
    for part in entry['path'].split('/'):
        path = path / part
        if path.is_symlink():
            raise ValueError('snapshot symlink: ' + entry['path'])
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(descriptor, 'rb') as handle:
        status = os.fstat(handle.fileno())
        if not stat.S_ISREG(status.st_mode):
            raise ValueError('snapshot input is not a regular file: ' + entry['path'])
        data = handle.read(MAX_FILE_BYTES + 1)
    if len(data) <= MAX_FILE_BYTES:
        if len(data) != entry['size'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
            raise ValueError('snapshot changed during inspection: ' + entry['path'])
    return data[:MAX_FILE_BYTES], len(data) > MAX_FILE_BYTES


def inspect_snapshot(store, paths=None, pattern=None):
    """Return bounded file/line evidence; pattern is a case-sensitive literal substring.

    Full run-scope inspect authorization is required before integrity reads; a
    narrower grant raises PermissionError even when displayed paths are narrow.
    Truncation means a search is incomplete. Binary inputs are explicitly skipped.
    No subprocesses run and no review records or working-tree files are changed.
    """
    metadata = _metadata(store)
    scopes = canonical_paths(metadata['paths'] if paths is None else paths)
    if not all(any(_covers(parent, scope) for parent in metadata['paths']) for scope in scopes):
        raise ValueError('inspection paths exceed pinned run scope')
    if pattern is not None and (not isinstance(pattern, str) or not pattern
                                or len(pattern.encode('utf-8')) > MAX_PATTERN_BYTES or '\n' in pattern or '\r' in pattern):
        raise ValueError('pattern must be a nonempty single-line literal of bounded size')
    grant = authorize_task(store, 'inspect', metadata['revision'], metadata['paths'])
    errors = _verify(store, metadata)
    if errors:
        raise ValueError('invalid snapshot: ' + '; '.join(errors))
    manifest = metadata['snapshot']
    entries = [entry for entry in manifest['files'] if any(_covers(scope, entry['path']) for scope in scopes)]
    if any(not any(_covers(scope, entry['path']) for entry in entries) for scope in scopes):
        raise ValueError('inspection path contains no pinned files')
    result = {'revision': metadata['revision'], 'authorization_id': grant, 'paths': scopes,
              'pattern': pattern, 'files': [], 'truncated': False}
    output_bytes = 0
    output_lines = 0
    for entry in entries:
        if (len(result['files']) >= MAX_FILES or output_bytes >= MAX_OUTPUT_BYTES
                or output_lines >= MAX_OUTPUT_LINES):
            result['truncated'] = True
            break
        data, truncated = _read(Path(manifest['root']), entry)
        file_result = {'path': entry['path'], 'lines': [], 'truncated': truncated, 'binary': False}
        try:
            if b'\x00' in data:
                raise UnicodeDecodeError('utf-8', data, 0, 1, 'binary input')
            content = codecs.getincrementaldecoder('utf-8')().decode(data, final=not truncated)
        except UnicodeDecodeError:
            file_result['binary'] = True
            result['files'].append(file_result)
            result['truncated'] |= truncated
            continue
        for number, line in enumerate(content.splitlines(), 1):
            if pattern is not None and pattern not in line:
                continue
            remaining = min(MAX_LINE_BYTES, MAX_OUTPUT_BYTES - output_bytes)
            if remaining <= 0 or output_lines >= MAX_OUTPUT_LINES:
                file_result['truncated'] = True
                break
            encoded = line.encode('utf-8')
            text = encoded[:remaining].decode('utf-8', errors='ignore')
            clipped = len(encoded) > remaining
            file_result['lines'].append({'number': number, 'text': text, 'truncated': clipped})
            output_bytes += len(text.encode('utf-8'))
            output_lines += 1
            file_result['truncated'] |= clipped
        result['files'].append(file_result)
        result['truncated'] |= file_result['truncated']
    return result
