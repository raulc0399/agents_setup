"""Bounded, local-only prerequisite checks; never run the configured worker."""

import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile


MIN_PYTHON = (3, 10)
PROBE_TIMEOUT_SECONDS = 5
NETWORK_FILESYSTEMS = frozenset(('nfs', 'nfs4', 'cifs', 'smbfs', 'sshfs', 'fuse.sshfs', '9p', 'ceph', 'glusterfs'))
LOCK_PROBE = '''import sqlite3, sys
connection = sqlite3.connect(sys.argv[1], timeout=0.15)
try:
    connection.execute('BEGIN IMMEDIATE')
except sqlite3.OperationalError as error:
    if 'locked' not in str(error).lower():
        raise
    sys.exit(3)
connection.execute('ROLLBACK')
connection.close()
'''


def _filesystem_type(root):
    """Identify known remote mounts on Linux; a probe cannot prove durability."""
    mounts = Path('/proc/mounts')
    if not mounts.exists():
        return None
    best = ('', None)
    for line in mounts.read_text().splitlines():
        fields = line.split()
        if len(fields) < 3:
            continue
        mount = fields[1].replace('\\040', ' ').replace('\\134', '\\')
        if (str(root) == mount or str(root).startswith(mount.rstrip('/') + '/')) and len(mount) > len(best[0]):
            best = (mount, fields[2])
    return best[1]


def _probe_lock(database):
    return subprocess.run([sys.executable, '-B', '-c', LOCK_PROBE, str(database)],
                          capture_output=True, text=True, timeout=PROBE_TIMEOUT_SECONDS,
                          check=False).returncode


def diagnose(project_root, worker_command=None, legacy_tmux=False):
    """Return {ok, project_root, checks}; checks contain name, ok and detail.

    The local filesystem requirement remains an operator responsibility: successful
    WAL/lock probes do not certify storage durability or arbitrary network mounts.
    """
    checks = []

    def add(name, ok, detail):
        checks.append({'name': name, 'ok': bool(ok), 'detail': detail})

    root = Path(project_root).resolve()
    add('python', sys.version_info >= MIN_PYTHON,
        f'Python {sys.version.split()[0]}; requires 3.10 or newer')
    add('platform', os.name == 'posix', 'POSIX process groups are required by worker isolation')
    try:
        if not root.is_dir():
            raise ValueError('project must be an existing directory')
        filesystem = _filesystem_type(root)
        add('filesystem', filesystem not in NETWORK_FILESYSTEMS,
            f'mount type: {filesystem or "unknown"}; use reliable local storage, probes cannot certify durability')
        if filesystem in NETWORK_FILESYSTEMS:
            raise ValueError('storage probes skipped: known network filesystem is unsupported')
        with tempfile.TemporaryDirectory(prefix='.review-doctor-', dir=root) as temporary:
            probe = Path(temporary) / 'write-probe'
            probe.write_bytes(b'local-write-probe')
            add('writable', probe.read_bytes() == b'local-write-probe', 'temporary file write/read succeeded')
            database = Path(temporary) / 'probe.db'
            connection = sqlite3.connect(database, timeout=0.15)
            try:
                mode = connection.execute('PRAGMA journal_mode=WAL').fetchone()[0]
                add('sqlite_wal', mode.lower() == 'wal', f'SQLite {sqlite3.sqlite_version}; journal mode {mode}')
                connection.execute('CREATE TABLE probe (value INTEGER)')
                connection.execute('BEGIN IMMEDIATE')
                blocked = _probe_lock(database)
                connection.rollback()
                available = _probe_lock(database)
                add('sqlite_locking', blocked == 3 and available == 0,
                    f'cross-process writer blocked={blocked == 3}; released lock writable={available == 0}')
            finally:
                connection.close()
    except (OSError, ValueError, sqlite3.Error, subprocess.SubprocessError) as error:
        add('storage_probe', False, str(error))

    if worker_command is not None:
        valid = isinstance(worker_command, (list, tuple)) and bool(worker_command)
        valid = valid and all(isinstance(arg, str) and arg for arg in worker_command)
        executable = shutil.which(worker_command[0]) if valid else None
        add('worker_command', executable is not None,
            f'executable: {executable}' if executable else 'worker command must name an available executable')
    if legacy_tmux:
        executable = shutil.which('tmux')
        add('legacy_tmux', executable is not None, executable or 'tmux executable unavailable')
    return {'ok': all(check['ok'] for check in checks), 'project_root': str(root), 'checks': checks}
