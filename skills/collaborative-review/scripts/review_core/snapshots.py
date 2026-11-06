"""Capture explicit project inputs as immutable, content-addressed directories."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import tempfile


_SKIPPED = frozenset({'.git', '.reviews', '__pycache__'})
_DIGEST = re.compile(r'[0-9a-f]{64}')
_HASH_CHUNK_SIZE = 1024 * 1024


def _relative(value):
    if not isinstance(value, str) or not value or '\\' in value:
        raise ValueError('paths must be nonempty relative POSIX paths')
    path = PurePosixPath(value)
    if path.is_absolute() or '..' in path.parts or value != path.as_posix():
        raise ValueError('path must be canonical and beneath its root')
    return path


def _safe_path(root, relative):
    candidate = root
    for part in _relative(relative).parts:
        candidate = candidate / part
        if candidate.is_symlink():
            raise ValueError('symlinks are not supported: ' + relative)
    return candidate


def _revision(files):
    encoded = json.dumps(files, sort_keys=True, separators=(',', ':'), ensure_ascii=True)
    return hashlib.sha256(encoded.encode('utf-8')).hexdigest()


def _read_file(path):
    descriptor = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0) | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as handle:
        before = os.fstat(handle.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise ValueError('only regular files are supported')
        data = handle.read()
        after = os.fstat(handle.fileno())
    if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
        raise ValueError('source changed during capture: ' + str(path))
    return data, stat.S_IMODE(before.st_mode)


def _hash_file(path):
    descriptor = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0) | os.O_NONBLOCK)
    digest = hashlib.sha256()
    size = 0
    with os.fdopen(descriptor, 'rb') as handle:
        before = os.fstat(handle.fileno())
        if not stat.S_ISREG(before.st_mode):
            raise ValueError('only regular files are supported')
        while True:
            chunk = handle.read(_HASH_CHUNK_SIZE)
            if not chunk:
                break
            digest.update(chunk)
            size += len(chunk)
        after = os.fstat(handle.fileno())
    if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
        raise ValueError('snapshot changed during verification: ' + str(path))
    return digest.hexdigest(), size, stat.S_IMODE(before.st_mode)


def create_snapshot(project_root, paths, output_root, run_id):
    """Return a manifest; output_root is the run collection, not the run directory."""
    project = Path(project_root).absolute()
    if project.resolve() != project or not project.is_dir():
        raise ValueError('project_root must be a real directory without symlinks')
    if not isinstance(run_id, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', run_id):
        raise ValueError('invalid run_id')
    if isinstance(paths, (str, bytes)):
        raise ValueError('paths must be a sequence')
    selections = sorted(set(str(_relative(str(path))) for path in paths))
    if not selections:
        raise ValueError('at least one input is required')
    output = Path(output_root).absolute()
    if output.resolve() != output:
        raise ValueError('output_root must not contain symlinks')
    snapshots = output / run_id / 'snapshots'
    if snapshots.resolve() != snapshots:
        raise ValueError('snapshot destination must not contain symlinks')
    selected = {}

    def collect(path):
        relative = path.relative_to(project).as_posix()
        _safe_path(project, relative)
        if path == snapshots or snapshots in path.parents:
            raise ValueError('snapshot state cannot be an input')
        if path.is_file():
            selected[relative] = path
            return
        if path == output or output in path.parents:
            raise ValueError('generated output directories cannot be inputs')
        if not path.is_dir():
            raise ValueError('input is missing or is not a regular file: ' + relative)
        for child in sorted(path.iterdir()):
            if child.name in _SKIPPED or child == output or output in child.parents:
                continue
            collect(child)

    for selection in selections:
        collect(_safe_path(project, selection))
    if not selected:
        raise ValueError('inputs contain no files')
    snapshots.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix='.capture-', dir=snapshots))
    try:
        files = []
        for relative, source in sorted(selected.items()):
            data, mode = _read_file(source)
            target = temporary / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            target.chmod(mode & ~0o222)
            files.append({'path': relative, 'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data), 'mode': mode})
        revision = _revision(files)
        destination = snapshots / revision
        manifest = {'revision': revision, 'root': str(destination), 'files': files,
                    'project_root': str(project), 'paths': selections}
        for directory, _, _ in os.walk(temporary, topdown=False):
            Path(directory).chmod(0o555)
        if destination.exists() or destination.is_symlink():
            errors = verify_snapshot(manifest, expected_run_dir=output / run_id,
                                     expected_project_root=project, expected_paths=selections)
            if errors:
                raise ValueError('existing snapshot is invalid: ' + '; '.join(errors))
        else:
            try:
                temporary.rename(destination)
            except OSError:
                if not destination.exists() or verify_snapshot(
                        manifest, expected_run_dir=output / run_id,
                        expected_project_root=project, expected_paths=selections):
                    raise
        return manifest
    finally:
        if temporary.exists():
            for directory, _, _ in os.walk(temporary):
                Path(directory).chmod(0o755)
            shutil.rmtree(temporary)


def verify_snapshot(manifest, *, expected_run_dir=None, expected_project_root=None,
                    expected_paths=None, expected_revision=None):
    """Check integrity; supply trusted anchors to authenticate manifest provenance.

    Without anchors this checks self-consistency only. Run consumers should pass
    all four anchors from their trusted run metadata, never from this manifest.
    """
    errors = []
    try:
        revision = manifest['revision']
        if not isinstance(revision, str) or not _DIGEST.fullmatch(revision):
            raise ValueError('invalid revision')
        if expected_revision is not None and revision != expected_revision:
            raise ValueError('snapshot revision differs from trusted revision')
        project_value = manifest['project_root']
        if not isinstance(project_value, str):
            raise ValueError('invalid project_root')
        project = Path(project_value)
        if not project.is_absolute() or str(project) != project_value or '..' in project.parts:
            raise ValueError('project_root must be a canonical absolute path')
        if expected_project_root is not None and project != Path(expected_project_root).absolute():
            raise ValueError('snapshot project differs from trusted project')
        selections = manifest['paths']
        if not isinstance(selections, list) or not selections:
            raise ValueError('paths must be a nonempty list')
        for selection in selections:
            _relative(selection)
        if selections != sorted(set(selections)):
            raise ValueError('paths must be sorted and unique')
        if expected_paths is not None:
            if isinstance(expected_paths, (str, bytes)):
                raise ValueError('expected_paths must be a sequence')
            trusted_paths = sorted(set(str(_relative(str(path))) for path in expected_paths))
            if selections != trusted_paths:
                raise ValueError('snapshot scope differs from trusted scope')
        root = Path(manifest['root'])
        if not root.is_absolute() or root.resolve() != root or root.name != revision or root.parent.name != 'snapshots':
            raise ValueError('invalid snapshot root')
        if expected_run_dir is not None:
            trusted_run = Path(expected_run_dir).absolute()
            if trusted_run.resolve() != trusted_run or root != trusted_run / 'snapshots' / revision:
                raise ValueError('snapshot root differs from trusted run directory')
        files = manifest['files']
        if not isinstance(files, list) or not files:
            raise ValueError('file manifest must not be empty')
        names = []
        for entry in files:
            relative = str(_relative(entry['path']))
            if relative == '.':
                raise ValueError('file path cannot be root')
            if not any(scope == '.' or relative == scope or relative.startswith(scope + '/')
                       for scope in selections):
                raise ValueError('manifest file is outside selected paths: ' + relative)
            names.append(relative)
            if set(entry) != {'path', 'sha256', 'size', 'mode'}:
                raise ValueError('invalid file entry')
            if not isinstance(entry['sha256'], str) or not _DIGEST.fullmatch(entry['sha256']):
                raise ValueError('invalid file hash')
            if type(entry['size']) is not int or entry['size'] < 0 or type(entry['mode']) is not int or not 0 <= entry['mode'] <= 0o7777:
                raise ValueError('invalid file metadata')
        if names != sorted(set(names)) or _revision(files) != revision:
            raise ValueError('manifest digest or ordering mismatch')
        if not root.is_dir():
            raise ValueError('snapshot root is missing')
        for entry in files:
            try:
                target = _safe_path(root, entry['path'])
                digest, size, mode = _hash_file(target)
                if size != entry['size'] or digest != entry['sha256']:
                    errors.append('content mismatch: ' + entry['path'])
                if mode != entry['mode'] & ~0o222:
                    errors.append('permission mismatch: ' + entry['path'])
            except (OSError, ValueError) as exc:
                errors.append(entry['path'] + ': ' + str(exc))
        actual = set()
        for directory, directories, filenames in os.walk(root, followlinks=False):
            if stat.S_IMODE(Path(directory).stat().st_mode) & 0o222:
                errors.append('writable snapshot directory: ' + str(directory))
            for name in directories + filenames:
                child = Path(directory) / name
                if child.is_symlink():
                    errors.append('snapshot symlink: ' + str(child))
            actual.update((Path(directory) / name).relative_to(root).as_posix() for name in filenames)
        if actual != set(names):
            errors.append('snapshot file set differs from manifest')
    except (KeyError, TypeError, ValueError, OSError) as exc:
        errors.append('invalid snapshot: ' + str(exc))
    return errors


def changed_paths(old, new):
    """Return paths whose content or original permission metadata changed."""
    def index(manifest):
        result = {}
        for entry in manifest['files']:
            path = str(_relative(entry['path']))
            if path == '.' or path in result:
                raise ValueError('invalid or duplicate file path')
            result[path] = (entry['sha256'], entry['size'], entry['mode'])
        return result

    before, after = index(old), index(new)
    return sorted(path for path in before.keys() | after.keys() if before.get(path) != after.get(path))
