"""Quiet bounded dispatch of trusted local workers; approval remains a separate gate."""
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
import fcntl
import hashlib
import json
import math
import os
import time

from .store import Store


STORE_POLL_SECONDS = 0.1


def _worker(run_dir, worker, task_id, command, timeout_seconds, dialect_id):
    from .cli import _coordinator
    from .runner import run_next_task
    with Store(run_dir) as store:
        return run_next_task(_coordinator(store), worker, command, timeout_seconds,
                             dialect_id, task_id=task_id)


def _blocker(code, reason, action, task_id=None):
    return dict(code=code, reason=reason, action=action, owner='operator', task_id=task_id)


def dispatch_run(run_dir, command, timeout_seconds=300, dialect_id='aasm-1', recipient='coordinator'):
    """Drain current work or return an explicit blocker; never grant, retry or recover."""
    from .challenge import current_context_fingerprint, schedule_challenge
    from .cli import _coordinator, readiness
    from .discussions import schedule_discussions
    from .notifications import queue_notification
    if not isinstance(command, list) or not command or not all(isinstance(part, str) and part for part in command):
        raise ValueError('command must be a nonempty list of nonempty strings')
    if (isinstance(timeout_seconds, bool) or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds) or timeout_seconds <= 0):
        raise ValueError('timeout_seconds must be positive and finite')
    with Store(run_dir) as store:
        # flock survives threads and releases on process exit; no stale state lease.
        descriptor = os.open(store.run_dir / 'dispatch.lock', os.O_CREAT | os.O_APPEND | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, 'a') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                coordinator = _coordinator(store)
                return dict(state='blocked', completed_task_ids=[], availability=coordinator.availability(),
                            blockers=[_blocker('dispatcher', 'Another dispatch controller owns this run',
                                               'Wait for that controller to finish')])
            coordinator = _coordinator(store)
            completed = []
            futures = {}
            failed_starts = set()
            blockers = []
            with ThreadPoolExecutor(max_workers=max(1, store.get_meta('max_workers', 1))) as pool:
                while True:
                    scheduling_blockers = []
                    for future in list(futures):
                        if not future.done():
                            continue
                        task_id = futures.pop(future)
                        try:
                            result = future.result()
                            if result and result['data']['status'] == 'completed':
                                completed.append(task_id)
                        except Exception as error:
                            failed_starts.add(task_id)
                            blockers.append(_blocker('worker', str(error), 'Inspect worker setup before dispatching again', task_id))
                    try:
                        schedule_discussions(store, coordinator, timeout_seconds=timeout_seconds)
                    except (ValueError, PermissionError) as error:
                        scheduling_blockers.append(_blocker('discussion', str(error), 'Resolve the discussion scheduling requirement'))
                    revision = store.get_meta('revision')
                    tasks = [task for task in store.list_records('task')
                             if task['data']['payload']['revision'] == revision]
                    plans = [plan for plan in store.list_records('review_plan')
                             if plan['data']['request']['revision'] == revision]
                    if plans and store.get_meta('challenge_required', True):
                        required_ids = {task_id for plan in plans for task_id in plan['data']['task_ids']}
                        required_ids.update(task['id'] for task in tasks if task['data']['kind'] == 'discussion'
                                            and not (task['data']['status'] == 'cancelled'
                                                     and task['data'].get('discussion_superseded') is True))
                        by_id = {task['id']: task for task in tasks}
                        if all(task_id in by_id and by_id[task_id]['data']['status'] == 'completed' for task_id in required_ids):
                            try:
                                schedule_challenge(store, coordinator)
                            except ValueError as error:
                                scheduling_blockers.append(_blocker('challenge', str(error), 'Resolve the challenge scheduling requirement'))
                    available = coordinator.availability()
                    scheduled = set(futures.values())
                    workers = {store.get_record('task', task_id)['data']['payload'].get('worker') or
                               store.get_record('task', task_id)['data']['payload'].get('reviewer') or 'worker-' + task_id
                               for task_id in scheduled}
                    slots = max(0, store.get_meta('max_workers', 1) - len(futures))
                    for row in available['tasks']:
                        if slots <= 0:
                            break
                        if not row['ready'] or row['task_id'] in scheduled or row['task_id'] in failed_starts or row['worker'] in workers:
                            continue
                        future = pool.submit(_worker, store.run_dir, row['worker'], row['task_id'],
                                             command, timeout_seconds, dialect_id)
                        futures[future] = row['task_id']
                        workers.add(row['worker'])
                        slots -= 1
                    if futures:
                        wait(futures, timeout=STORE_POLL_SECONDS, return_when=FIRST_COMPLETED)
                        continue
                    available = coordinator.availability()
                    tasks = [task for task in store.list_records('task')
                             if task['data']['payload']['revision'] == revision]
                    live = any(task['data']['status'] == 'running' and
                               coordinator.clock() < task['data']['lease_expires'] for task in store.list_records('task'))
                    if live:
                        time.sleep(STORE_POLL_SECONDS)
                        continue
                    blockers.extend(scheduling_blockers)
                    fingerprint = current_context_fingerprint(store)
                    replacements = {task['data']['payload'].get('budget_plan_id') for task in tasks
                                    if task['data']['kind'] == 'challenge' and task['data']['status'] == 'completed'
                                    and task['data'].get('completion_context_fingerprint',
                                        task['data']['payload'].get('context_fingerprint')) == fingerprint}
                    for row in available['tasks']:
                        blockers.extend(dict(item, task_id=row['task_id']) for item in row['blockers'])
                    for task in tasks:
                        if task['data']['status'] in ('failed', 'cancelled'):
                            if (task['data']['status'] == 'cancelled' and task['data']['kind'] == 'discussion'
                                    and task['data'].get('discussion_superseded') is True):
                                continue
                            if (task['data']['status'] == 'failed' and task['data']['kind'] == 'challenge'
                                    and not coordinator._challenge_is_current(task['data'])
                                    and task['data']['payload'].get('budget_plan_id') in replacements):
                                continue
                            # Superseded challenges are deliberate context replacement.
                            if task['data']['status'] == 'cancelled' and task['data']['kind'] == 'challenge' and not coordinator._challenge_is_current(task['data']):
                                continue
                            blockers.append(_blocker(task['data']['status'],
                                'Task ' + task['id'] + ' is ' + task['data']['status'] + ': ' + str(task['data'].get('error')),
                                'Inspect and make an explicit retry or replacement decision', task['id']))
                    break
            blockers = list({json.dumps(item, sort_keys=True): item for item in blockers}.values())
            approval = readiness(store, coordinator)
            state = 'blocked' if blockers else 'complete'
            result = dict(state=state, completed_task_ids=completed, blockers=blockers,
                          availability=available, approval=approval, plan_exists=bool(plans))
            summary = 'Review dispatch completed.' if state == 'complete' else 'Review dispatch blocked: ' + blockers[0]['reason']
            if plans and approval['result'] == 'BLOCKED':
                summary += ' Review approval remains blocked.'
            identity = json.dumps(dict(recipient=recipient, revision=revision, state=state, blockers=blockers,
                tasks=[(task['id'], task['data']['status'], task['data']['attempt']) for task in tasks],
                approval=approval), sort_keys=True)
            notification_id = 'dispatch-' + hashlib.sha256(identity.encode()).hexdigest()
            queue_notification(store, recipient, summary.strip(), kind='blocked' if state == 'blocked' else 'completion',
                               notification_id=notification_id)
            return result
