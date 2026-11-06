"""Freeze executable review code and policies for a single run."""

import errno
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import tempfile
import uuid


PROTOCOL_DIRECTORY = 'protocol'
MANIFEST_NAME = 'manifest.json'


def _json(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode('utf-8')


def _digest(data):
    return hashlib.sha256(data).hexdigest()


def _safe_root(path):
    path = Path(os.path.abspath(path))
    if any(part.is_symlink() for part in (path, *path.parents)):
        raise ValueError('symlink paths are not allowed')
    return path


def _read_regular(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError('protocol requires regular files: ' + str(path))
        with os.fdopen(descriptor, 'rb', closefd=False) as stream:
            return stream.read()
    finally:
        os.close(descriptor)


def _existing_protocol(run, manifest):
    errors = verify_protocol(run)
    if errors:
        raise ValueError('invalid existing protocol: ' + '; '.join(errors))
    existing = json.loads(_read_regular(run / PROTOCOL_DIRECTORY / MANIFEST_NAME))
    if existing != manifest:
        raise ValueError('protocol or policies changed; create a new run')
    return existing


def _sources(source):
    entry = source / 'review.py'
    package = source / 'review_core'
    if entry.is_symlink() or package.is_symlink():
        raise ValueError('symlink in protocol source')
    if not entry.is_file() or not package.is_dir():
        raise ValueError('source must contain review.py and review_core')
    paths = [entry]
    for root, directories, files in os.walk(package, followlinks=False):
        root = Path(root)
        for name in directories:
            if (root / name).is_symlink():
                raise ValueError('symlink in protocol source')
        directories[:] = sorted(name for name in directories if name not in ('__pycache__', 'tests', 'test'))
        paths.extend(root / name for name in sorted(files)
                     if name.endswith('.py') and not name.startswith('test_') and not name.endswith('_test.py'))
    result = {}
    for path in sorted(paths):
        if path.is_symlink() or not path.is_file():
            raise ValueError('protocol source must contain regular files')
        result[path.relative_to(source).as_posix()] = _read_regular(path)
    if not any(name.startswith('review_core/') for name in result):
        raise ValueError('review_core must contain Python files')
    return result


def freeze_protocol(run_dir, source_dir, policies):
    """Return a pinned manifest; changed input requires a new run."""
    run = _safe_root(run_dir)
    source = _safe_root(source_dir)
    if not isinstance(policies, dict):
        raise ValueError('policies must be a JSON object')
    policies = json.loads(_json(policies))
    contents = _sources(source)
    payload = {'format': 1, 'policies': policies,
               'files': [{'path': name, 'sha256': _digest(data), 'size': len(data)}
                         for name, data in sorted(contents.items())]}
    manifest = dict(payload, digest=_digest(_json(payload)))
    destination = run / PROTOCOL_DIRECTORY
    if destination.exists() or destination.is_symlink():
        return _existing_protocol(run, manifest)
    run.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix='.protocol-', dir=run))
    try:
        for name, data in contents.items():
            target = staging / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            target.chmod(0o444)
        (staging / MANIFEST_NAME).write_bytes(_json(manifest))
        (staging / MANIFEST_NAME).chmod(0o444)
        # Rename publishes a complete directory and refuses an existing nonempty bundle.
        try:
            staging.rename(destination)
        except OSError as error:
            if error.errno not in (errno.EEXIST, errno.ENOTEMPTY):
                raise
            return _existing_protocol(run, manifest)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return manifest


def verify_protocol(run_dir):
    """Return integrity errors without executing any bundled code."""
    try:
        root = _safe_root(run_dir) / PROTOCOL_DIRECTORY
        _safe_root(root)
        manifest_path = root / MANIFEST_NAME
        if manifest_path.is_symlink():
            raise ValueError('symlink manifest')
        manifest = json.loads(_read_regular(manifest_path))
        if set(manifest) != {'format', 'policies', 'files', 'digest'} or manifest['format'] != 1:
            raise ValueError('invalid manifest structure')
        if not isinstance(manifest['policies'], dict) or not isinstance(manifest['files'], list):
            raise ValueError('invalid manifest fields')
        payload = {key: value for key, value in manifest.items() if key != 'digest'}
        if _digest(_json(payload)) != manifest['digest']:
            raise ValueError('manifest digest mismatch')
        expected = {MANIFEST_NAME}
        errors = []
        for entry in manifest['files']:
            name = entry['path']
            path = Path(name)
            if (not isinstance(name, str) or path.is_absolute() or '..' in path.parts
                    or path.as_posix() != name or name in expected
                    or not (name == 'review.py' or (name.startswith('review_core/') and name.endswith('.py')))):
                raise ValueError('invalid bundle path')
            expected.add(name)
            target = _safe_root(root / path)
            data = _read_regular(target)
            if len(data) != entry['size'] or _digest(data) != entry['sha256']:
                errors.append('content mismatch: ' + name)
        if 'review.py' not in expected or not any(name.startswith('review_core/') for name in expected):
            raise ValueError('incomplete executable bundle')
        actual = set()
        for current, directories, files in os.walk(root, followlinks=False):
            for name in directories + files:
                target = Path(current) / name
                if target.is_symlink():
                    raise ValueError('symlink in frozen bundle')
            actual.update((Path(current) / name).relative_to(root).as_posix() for name in files)
        if actual != expected:
            errors.append('bundle file set mismatch')
        return errors
    except (OSError, ValueError, TypeError, KeyError) as error:
        return [str(error)]


def propose_change(store, text, evidence=()):
    """Append a backlog proposal without modifying the active protocol."""
    if not isinstance(text, str) or not text.strip():
        raise ValueError('proposal text is required')
    if isinstance(evidence, (str, bytes)) or not isinstance(evidence, (list, tuple)):
        raise ValueError('evidence must be a list')
    evidence = json.loads(_json(list(evidence)))
    return store.put_record('protocol_proposal', 'protocol_proposal-' + uuid.uuid4().hex,
                            {'text': text, 'evidence': evidence, 'status': 'proposed'},
                            event='protocol.proposed')
