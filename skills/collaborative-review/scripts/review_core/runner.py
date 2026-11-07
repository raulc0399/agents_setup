"""Run trusted worker commands in retained snapshot workspaces (not a sandbox)."""
import hashlib
import json
import math
import os
from pathlib import Path
import selectors
import signal
import socket
import subprocess
import tempfile
import time

from .aasm import BASE_DIALECT, BUILTIN_OPERATIONS
from .aasm_records import prepare_result
from .dialects import get_dialect
from .snapshots import verify_snapshot


MAX_OUTPUT_BYTES = 1024 * 1024
POLL_SECONDS = 0.05
LEASE_SECONDS = 30
DEFAULT_TIMEOUT_SECONDS = 300
MAX_ERROR_BYTES = 2048


def _stop_process(process):
    # A worker may exit while descendants still hold its pipes or keep writing.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def _mark_execution(coordinator, task, execution):
    with coordinator.store.transaction():
        current = coordinator._leased(task['id'], task['data']['worker'], task['data']['lease_id'])
        current['data']['execution'] = execution
        coordinator.store.put_record('task', task['id'], current['data'],
                                     actor=task['data']['worker'], event='task.execution')


def _exchange(coordinator, task, command, workspace, request, timeout):
    data = task['data']
    worker, lease = data['worker'], data['lease_id']
    environment = os.environ.copy()
    environment.update(REVIEW_RUN_DIR=str(coordinator.store.run_dir),
                       TASK_ID=task['id'], LEASE_ID=lease)
    deadline = time.monotonic() + timeout
    output = bytearray()
    diagnostic = bytearray()
    total_output = 0
    pending = memoryview(json.dumps(request, allow_nan=False).encode('utf-8'))
    next_renewal = time.monotonic() + min(LEASE_SECONDS / 3, timeout / 3)
    execution = {'host': socket.gethostname(), 'workspace': str(workspace),
                 'launch_intent': True, 'terminated': False}
    process = None
    # Commit intent before launch. A crash before registration must never cause
    # recovery to repeat a command whose effects cannot be established.
    _mark_execution(coordinator, task, execution)
    try:
        with coordinator.store.transaction():
            current = coordinator._leased(task['id'], worker, lease)
            if current['data'].get('cancellation_requested'):
                raise RuntimeError('Worker cancellation requested before launch')
            process = subprocess.Popen(command, cwd=workspace, env=environment, shell=False,
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, start_new_session=True)
            execution.update(pid=process.pid, process_group=process.pid, start_token=None)
            try:
                execution['start_token'] = Path(f'/proc/{process.pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
            except (OSError, IndexError):
                pass
            _mark_execution(coordinator, task, execution)
        # Worker adapters receive their task only after PID registration commits.
        with selectors.DefaultSelector() as selector:
            for stream in (process.stdin, process.stdout, process.stderr):
                os.set_blocking(stream.fileno(), False)
            selector.register(process.stdin, selectors.EVENT_WRITE, 'input')
            selector.register(process.stdout, selectors.EVENT_READ, 'output')
            selector.register(process.stderr, selectors.EVENT_READ, 'error')
            while selector.get_map() or process.poll() is None:
                now = time.monotonic()
                current = coordinator.store.get_record('task', task['id'])
                if current['data'].get('cancellation_requested'):
                    raise RuntimeError('Worker cancellation requested')
                if now >= deadline:
                    raise TimeoutError('Worker exceeded total timeout')
                if now >= next_renewal:
                    coordinator.renew_task(task['id'], worker, lease, lease_seconds=LEASE_SECONDS)
                    next_renewal = now + min(LEASE_SECONDS / 3, timeout / 3)
                for key, _ in selector.select(min(POLL_SECONDS, deadline - now)):
                    if key.data == 'input':
                        try:
                            count = os.write(key.fd, pending[:65536])
                            pending = pending[count:]
                        except BrokenPipeError:
                            pending = pending[:0]
                        if not pending:
                            selector.unregister(key.fileobj)
                            key.fileobj.close()
                        continue
                    block = os.read(key.fd, 65536)
                    if not block:
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
                        continue
                    total_output += len(block)
                    if total_output > MAX_OUTPUT_BYTES:
                        raise ValueError('Worker output exceeds byte limit')
                    if key.data == 'output':
                        output.extend(block)
                    elif len(diagnostic) < MAX_ERROR_BYTES:
                        diagnostic.extend(block[:MAX_ERROR_BYTES - len(diagnostic)])
            if process.returncode:
                raise RuntimeError(f'Worker exited with status {process.returncode}')
            return prepare_result(coordinator.store, worker, output.decode('utf-8'),
                                  dialect_id=request['language']['id'])
    except Exception as exc:
        if diagnostic:
            detail = diagnostic.decode('utf-8', errors='replace')
            raise RuntimeError(f'{type(exc).__name__}: {exc}; stderr: {detail}') from exc
        raise
    finally:
        if process is not None:
            _stop_process(process)
        execution['terminated'] = True
        # An expired lease still needs a durable process termination marker.
        with coordinator.store.transaction():
            current = coordinator.store.get_record('task', task['id'])
            if current['data']['lease_id'] == lease:
                current['data']['execution'] = execution
                coordinator.store.put_record('task', task['id'], current['data'], actor=worker,
                                             event='task.process_terminated')
        for stream in (() if process is None else (process.stdin, process.stdout, process.stderr)):
            if not stream.closed:
                stream.close()


def run_next_task(coordinator, worker, command, timeout_seconds=DEFAULT_TIMEOUT_SECONDS, dialect_id=BASE_DIALECT, task_id=None):
    """Claim one task and return its terminal envelope, or None when none is claimable.

    stdin: {task: {id, kind, revision, brief, scope_paths, worker, attempt},
            snapshot: {root, revision, files}, language: {id, digest, definitions,
            builtin_schemas, instructions}}. stdout: strict UTF-8 AASM; helper-generated
    JSON is passed to Coordinator.result_handler for typed ingestion.
    """
    if not isinstance(command, list) or not command or not all(isinstance(part, str) and part for part in command):
        raise ValueError('command must be a nonempty list of nonempty strings')
    if isinstance(timeout_seconds, bool) or not isinstance(timeout_seconds, (int, float)) or not math.isfinite(timeout_seconds) or timeout_seconds <= 0:
        raise ValueError('timeout_seconds must be positive and finite')
    dialect = get_dialect(coordinator.store, dialect_id, participant=worker)
    task = coordinator.claim_task(worker, lease_seconds=timeout_seconds, task_id=task_id)
    if task is None:
        return None
    data = task['data']
    started = time.monotonic()
    try:
        _mark_execution(coordinator, task, {'terminated': True, 'launch_intent': False,
                                            'host': socket.gethostname()})
        store = coordinator.store
        manifest = store.get_meta('snapshot')
        if any(store.get_meta(key) is None for key in ('project_root', 'paths', 'revision')):
            raise ValueError('Missing trusted snapshot anchors')
        errors = verify_snapshot(manifest, expected_run_dir=store.run_dir,
            expected_project_root=store.get_meta('project_root'), expected_paths=store.get_meta('paths'),
            expected_revision=store.get_meta('revision'))
        if errors:
            raise ValueError('Snapshot verification failed: ' + '; '.join(errors))
        scope = data['payload']['scope_paths']
        selected = [entry for entry in manifest['files'] if any(
            path == '.' or entry['path'] == path or entry['path'].startswith(path + '/') for path in scope)]
        workspaces = store.run_dir / 'workspaces'
        if workspaces.is_symlink():
            raise ValueError('Workspace directory must not be a symlink')
        workspaces.mkdir(exist_ok=True)
        workspace = Path(tempfile.mkdtemp(prefix='attempt-', dir=workspaces))
        _mark_execution(coordinator, task, {'workspace': str(workspace), 'terminated': True,
                                            'launch_intent': False, 'host': socket.gethostname()})
        for entry in selected:
            source = Path(manifest['root']) / entry['path']
            content = source.read_bytes()
            if source.is_symlink() or hashlib.sha256(content).hexdigest() != entry['sha256']:
                raise ValueError('Snapshot changed during workspace creation')
            destination = workspace / entry['path']
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
            destination.chmod((entry['mode'] & 0o777) | 0o600)
        request = {'task': dict(id=task['id'], kind=data['kind'], revision=data['payload']['revision'],
            brief=data['payload']['brief'], scope_paths=scope, worker=worker, attempt=data['attempt']),
            'snapshot': {'root': str(workspace), 'revision': manifest['revision'], 'files': selected},
            'language': {'id': dialect['id'], 'digest': dialect['digest'],
                'definitions': dialect['definitions'], 'builtin_schemas': BUILTIN_OPERATIONS,
                'instructions': 'Return only AASM commands, one per line, using the supplied schemas. '
                    'Use JSON strings for text with spaces. Do not emit JSON envelopes, prose, or markdown. '
                    'Review and challenge results require exactly one final VOTE referencing coverage IDs. '
                    'Commands never grant authorization or execute shell actions.'}}
        timeout = min(timeout_seconds, data.get('attempt_budget_seconds', data['payload']['timeout_seconds'])) - (time.monotonic() - started)
        if timeout <= 0:
            raise TimeoutError('Workspace preparation exceeded total timeout')
        coordinator.renew_task(task['id'], worker, data['lease_id'], lease_seconds=LEASE_SECONDS)
        result = _exchange(coordinator, task, command, workspace, request, timeout)
        if result['dialect_digest'] != dialect['digest']:
            raise ValueError('Worker result dialect differs from pinned dialect')
        return coordinator.complete_task(task['id'], worker, data['lease_id'], result)
    except Exception as exc:
        try:
            return coordinator.fail_task(task['id'], worker, data['lease_id'],
                                         (type(exc).__name__ + ': ' + str(exc))[:4096])
        except ValueError:
            # A recovered/reassigned lease cannot acknowledge this attempt.
            # Recovery retains responsibility for its reservation and retry policy.
            return coordinator.store.get_record('task', task['id'])
