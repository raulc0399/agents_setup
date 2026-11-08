"""Transactional checkpoints and explicit recovery of expired task leases."""
from contextlib import contextmanager
import math
from pathlib import Path
import os
import socket
import time


OUTCOMES = {'completed', 'failed', 'cancelled', 'retry'}


def _now(value):
    value = time.time() if value is None else value
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise ValueError('now must be finite Unix seconds')
    return value


def _process_may_live(data):
    execution = data.get("execution")
    if not execution or execution.get("terminated") is True:
        return False
    if execution.get("host") != socket.gethostname():
        return True
    pid = execution.get("pid")
    token = execution.get("start_token")
    if isinstance(pid, int) and pid > 0 and token:
        try:
            actual = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
            if str(token) != actual:
                return False
        except (OSError, IndexError):
            pass
    process_group = execution.get("process_group")
    pid = execution.get("pid")
    if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
        return True
    try:
        if isinstance(process_group, int) and not isinstance(process_group, bool) and process_group > 0:
            os.killpg(process_group, 0)
        else:
            os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except OSError:
        return True
    # A live or reused PID/group is left for operator inspection.
    return True


@contextmanager
def _checkpoint_transaction(store):
    # BEGIN is a snapshot read transaction and also works on read-only connections.
    owned = not store.connection.in_transaction
    if owned:
        store.connection.execute('BEGIN')
    try:
        yield
    finally:
        if owned:
            store.connection.rollback()


def _worker_tasks(tasks, worker, revision):
    current = [task for task in tasks if task['data']['payload']['revision'] == revision]
    identities = {name for task in current for name in (task['data'].get('worker'),
                  task['data']['payload'].get('worker'), task['data']['payload'].get('reviewer')) if name}
    if not isinstance(worker, str) or not worker or worker not in identities:
        raise ValueError('Unknown handoff worker')
    visible = []
    for task in current:
        data = task['data']
        assigned = data.get('worker') or data['payload'].get('worker') or data['payload'].get('reviewer')
        if assigned == worker:
            visible.append(task)
            continue
        # Results, attempts, progress and errors can all contain private peer findings.
        visible.append(dict(id=task['id'], data=dict(kind=data['kind'], status=data['status'],
            worker=assigned, payload=dict(revision=revision))))
    return visible


def handoff(store, now=None, worker=None):
    """Read a consistent checkpoint without changing the run."""
    now = _now(now)
    with _checkpoint_transaction(store):
        tasks = store.list_records('task')
        spent = store.get_meta('spent_seconds', 0)
        reserved = sum(t['data'].get('reserved_seconds', 0) for t in tasks)
        budget = store.get_meta('budget_seconds', 0)
        return {'run_id': store.run_dir.name, 'run_dir': str(store.run_dir),
                'project_root': store.get_meta('project_root'), 'paths': store.get_meta('paths'),
                'approval_scope': store.get_meta('approval_scope'),
                'status': store.get_meta('status'), 'revision': store.get_meta('revision'),
                'tasks': tasks if worker is None else _worker_tasks(tasks, worker, store.get_meta('revision')),
                'unfinished_task_ids': [t['id'] for t in tasks
                    if t['data']['status'] not in ('completed', 'failed', 'cancelled')],
                'operator_decisions': [t['id'] for t in tasks if t['data']['status'] == 'uncertain' or
                    (t['data']['status'] == 'running' and t['data']['lease_expires'] <= now
                     and _process_may_live(t['data']))],
                'budget_seconds': budget, 'spent_seconds': spent,
                'reserved_seconds': reserved, 'remaining_seconds': budget - spent - reserved}


def resume_run(store, coordinator, now=None):
    """Recover expired leases; explicit resume never reopens a completed run."""
    now = _now(coordinator.clock() if now is None else now)
    if coordinator.store is not store:
        raise ValueError('Coordinator must use the supplied store')
    with store.transaction():
        for task in store.list_records('task'):
            data = task['data']
            if data['status'] != 'running' or data['lease_expires'] > now:
                continue
            if data['kind'] != 'execute' and _process_may_live(data):
                continue
            remaining = max(0, data['payload']['timeout_seconds'] - data.get('spent_seconds', 0))
            timeout = min(remaining, data.get('attempt_budget_seconds', remaining))
            elapsed = timeout if data['kind'] == 'execute' else min(timeout, max(0, now - data['started_at']))
            data['spent_seconds'] = data.get('spent_seconds', 0) + elapsed
            store.set_meta('spent_seconds', store.get_meta('spent_seconds', 0) + elapsed)
            status = 'uncertain' if data['kind'] == 'execute' else (
                'cancelled' if data.get('cancellation_requested') else 'pending')
            reason = data.get('cancellation_requested') or 'Lease expired before acknowledgement'
            if status == 'pending' and data['spent_seconds'] >= data['payload']['timeout_seconds']:
                status = 'failed'
                reason = 'Task budget exhausted during expired lease'
            history = data.setdefault('attempts', [])
            if history:
                history[-1].update(status='uncertain' if status == 'uncertain' else 'expired',
                                   finished_at=now, error=reason, spent_seconds=elapsed)
            data.update(status=status, reserved_seconds=0, error=reason,
                        finished_at=None if status == 'pending' else now,
                        lease_id=None, lease_expires=None, worker=None, started_at=None)
            store.put_record('task', task['id'], data, actor='operator', event='task.recovered',
                             expected_version=task['version'])
        # Unresolved effects pause the entire run, including unrelated execution.
        uncertain = any(t['data']['status'] == 'uncertain' for t in store.list_records('task'))
        status = store.get_meta('status')
        if uncertain and status != 'complete':
            store.set_meta('status', 'paused')
        elif status == 'paused':
            store.set_meta('status', 'running')
        store.append_event('run.recovered', 'run', {'at': now, 'uncertain': uncertain}, actor='operator')
        return handoff(store, now=now)


def resolve_uncertain_task(store, task_id, outcome, evidence, actor='operator', now=None,
                           process_termination_evidence=None):
    """Record an operator's observed effects before permitting a retry or completion."""
    if outcome not in OUTCOMES:
        raise ValueError('outcome must be completed, failed, cancelled, or retry')
    if not isinstance(evidence, str) or not evidence.strip():
        raise ValueError('Recovery requires nonempty outcome evidence')
    if not isinstance(actor, str) or not actor.strip():
        raise ValueError('actor must be nonempty')
    now = _now(now)
    with store.transaction():
        task = store.get_record('task', task_id)
        if task is None or task['data']['status'] != 'uncertain':
            raise ValueError('Task does not require an operator decision')
        data = task['data']
        if outcome == 'retry' and data.get('spent_seconds', 0) >= data['payload']['timeout_seconds']:
            raise ValueError('Task budget exhausted; create a new task or run after operator review')
        if process_termination_evidence is not None:
            _confirm_unknown_process(data, process_termination_evidence)
        if _process_may_live(data):
            raise ValueError('Worker process may still be alive; terminate and verify it first')
        decision = {'outcome': outcome, 'evidence': evidence, 'actor': actor, 'at': now}
        data.setdefault('recovery_decisions', []).append(decision)
        if outcome == 'retry':
            data.pop('cancellation_requested', None)
        data.update(status='pending' if outcome == 'retry' else outcome,
                    finished_at=None if outcome == 'retry' else now,
                    error=None if outcome in ('retry', 'completed') else evidence,
                    result={'recovery_evidence': evidence} if outcome == 'completed' else None)
        return store.put_record('task', task_id, data, actor=actor, event='task.recovery_resolved',
                                expected_version=task['version'])


def _confirm_unknown_process(data, evidence):
    if not isinstance(evidence, str) or not evidence.strip():
        raise ValueError('Process termination requires nonempty evidence')
    execution = data.get('execution')
    if not execution or execution.get('terminated') is True:
        raise ValueError('No unknown process requires confirmation')
    pid = execution.get('pid')
    if (execution.get('host') == socket.gethostname() and isinstance(pid, int)
            and not isinstance(pid, bool) and pid > 0 and _process_may_live(data)):
        raise ValueError('A local live process cannot be overridden')
    execution.update(terminated=True, termination_evidence=evidence)


def confirm_process_terminated(store, task_id, evidence, actor='operator', now=None):
    """Record verified termination for an unreachable host or interrupted launch."""
    now = _now(now)
    if not isinstance(actor, str) or not actor.strip():
        raise ValueError('actor must be nonempty')
    with store.transaction():
        task = store.get_record('task', task_id)
        if task is None or task['data']['status'] not in ('running', 'uncertain'):
            raise ValueError('Task has no unfinished worker to recover')
        data = task['data']
        if data['status'] == 'running' and data['lease_expires'] > now:
            raise ValueError('An active lease cannot be overridden')
        _confirm_unknown_process(data, evidence)
        return store.put_record('task', task_id, data, actor=actor,
            event='task.process_termination_confirmed', expected_version=task['version'])
