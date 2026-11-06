"""Serialized task leases, budget reservations, and revision-bound review decisions."""
import hashlib
import json
import math
import time
import uuid

from .authorization import authorize_task, canonical_paths
from .findings import finding_gaps


ACTIONS = {'review': 'review', 'challenge': 'review', 'specialist': 'review',
           'inspect': 'inspect', 'execute': 'execute'}
TERMINAL = {'completed', 'failed', 'cancelled'}
BUDGET_ROUNDOFF_SECONDS = 1e-9


def _text(value, name):
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f'{name} must be a nonempty string')
    return value


def _positive(value, name):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
        raise ValueError(f'{name} must be positive and finite')
    return value


def _remaining_seconds(data):
    return max(0, data['payload']['timeout_seconds'] - data.get('spent_seconds', 0))


def _object(value):
    if not isinstance(value, dict):
        raise ValueError('Expected a JSON object')
    return json.loads(json.dumps(value, allow_nan=False))


class Coordinator:
    def __init__(self, store, clock=time.time, result_handler=None):
        self.store = store
        self.clock = clock
        self.result_handler = result_handler

    def enqueue_task(self, kind, payload, dependencies=(), task_id=None):
        if kind not in ACTIONS:
            raise ValueError('Unknown task kind')
        payload = _object(payload)
        _text(payload.get('revision'), 'revision')
        _text(payload.get('brief'), 'brief')
        _positive(payload.get('timeout_seconds'), 'timeout_seconds')
        if not isinstance(payload.get('scope_paths'), list):
            raise ValueError('scope_paths must be a list of paths')
        payload['scope_paths'] = canonical_paths(payload['scope_paths'])
        if payload.get('required_action', ACTIONS[kind]) != ACTIONS[kind]:
            raise ValueError('required_action must match task kind')
        payload['required_action'] = ACTIONS[kind]
        dependencies = list(dependencies)
        if len(set(dependencies)) != len(dependencies) or not all(isinstance(d, str) and d for d in dependencies):
            raise ValueError('dependencies must be unique task IDs')
        task_id = _text(task_id, 'task_id') if task_id is not None else 'task-' + uuid.uuid4().hex
        with self.store.transaction():
            old = self.store.get_record('task', task_id)
            if old:
                if all(old['data'][k] == v for k, v in {'kind': kind, 'payload': payload, 'dependencies': dependencies}.items()):
                    return old
                raise ValueError('Task ID already has different input')
            if payload['revision'] != self.store.get_meta('revision'):
                raise ValueError('Task revision is not current')
            for dependency in dependencies:
                if dependency == task_id or not self.store.get_record('task', dependency):
                    raise ValueError('Unknown dependency')
            data = dict(kind=kind, payload=payload, dependencies=dependencies, status='pending', worker=None,
                        lease_id=None, lease_expires=None, attempt=0, attempts=[], created_at=self.clock(),
                        started_at=None, finished_at=None, result=None, error=None, reserved_seconds=0, spent_seconds=0)
            return self.store.put_record('task', task_id, data, event='task.enqueued', expected_version=0)

    def _challenge_is_current(self, data):
        if data['kind'] != 'challenge' or 'context_fingerprint' not in data['payload']:
            return True
        from .challenge import current_context_fingerprint
        return data['payload']['context_fingerprint'] == current_context_fingerprint(self.store)

    def _attempt_seconds(self, data, tasks):
        remaining = _remaining_seconds(data)
        plan_id = data['payload'].get('budget_plan_id')
        if data['kind'] != 'challenge' or plan_id is None:
            return remaining
        plan = self.store.get_record('review_plan', plan_id)
        if not plan or plan['data']['request']['revision'] != data['payload']['revision']:
            return 0
        bound = [task['data'] for task in tasks if task['data']['kind'] == 'challenge' and
                 task['data']['payload'].get('budget_plan_id') == plan_id]
        spent = math.fsum(task.get('spent_seconds', 0) for task in bound)
        reserved = math.fsum(task.get('reserved_seconds', 0) for task in bound
                             if task['status'] == 'running')
        available = max(0, plan['data']['challenge_budget_seconds'] - spent - reserved)
        return min(remaining, available)

    def _earmarked_seconds(self, candidate, tasks):
        """Keep pending planned work funded without reserving active work twice."""
        protected = []
        by_id = {task['id']: task['data'] for task in tasks}
        for plan in self.store.list_records('review_plan'):
            data = plan['data']
            if data['request']['revision'] != self.store.get_meta('revision'):
                continue
            for task_id in data['task_ids']:
                task = by_id.get(task_id)
                if task and task['status'] == 'pending' and task_id != candidate['id']:
                    protected.append(_remaining_seconds(task))
            bound = [task for task in by_id.values() if task['kind'] == 'challenge' and
                     task['payload'].get('budget_plan_id') == plan['id'] and
                     task['payload']['revision'] == data['request']['revision']]
            reserve = max(0, data['challenge_budget_seconds'] -
                          math.fsum(task.get('spent_seconds', 0) for task in bound))
            if any(task['status'] == 'running' for task in bound):
                continue
            if (any(task['status'] == 'completed' for task in bound) and
                    not any(task['status'] == 'pending' for task in bound)):
                continue
            task = candidate['data']
            if (task['kind'] == 'challenge' and task['payload'].get('budget_plan_id') == plan['id']
                    and self._attempt_seconds(task, tasks) <= reserve + BUDGET_ROUNDOFF_SECONDS):
                reserve = max(0, reserve - self._attempt_seconds(task, tasks))
            protected.append(reserve)
        return math.fsum(protected)

    def claim_task(self, worker, capabilities=None, lease_seconds=300):
        _text(worker, 'worker')
        _positive(lease_seconds, 'lease_seconds')
        with self.store.transaction():
            if self.store.get_meta('status') in ('paused', 'complete'):
                return None
            tasks = self.store.list_records('task')
            if any(t['data']['status'] == 'uncertain' for t in tasks):
                return None
            active = [t['data'] for t in tasks if t['data']['status'] == 'running']
            if any(t['worker'] == worker for t in active) or len(active) >= self.store.get_meta('max_workers', 1):
                return None
            budget = self.store.get_meta('budget_seconds', 0) - self.store.get_meta('spent_seconds', 0)
            budget -= math.fsum(t.get('reserved_seconds', 0) for t in active)
            for task in tasks:
                data = task['data']
                payload = data['payload']
                if data['status'] != 'pending' or payload['revision'] != self.store.get_meta('revision'):
                    continue
                if not self._challenge_is_current(data):
                    continue
                intended = payload.get('worker') or payload.get('reviewer')
                if intended is not None and intended != worker:
                    continue
                if capabilities is not None and data['kind'] not in capabilities:
                    continue
                if any(self.store.get_record('task', d)['data']['status'] != 'completed' for d in data['dependencies']):
                    continue
                timeout = self._attempt_seconds(data, tasks)
                available = budget - self._earmarked_seconds(task, tasks)
                if timeout <= 0 or available <= 0 or timeout > available + BUDGET_ROUNDOFF_SECONDS or (data['kind'] == 'execute' and active) or any(t['kind'] == 'execute' for t in active):
                    continue
                timeout = min(timeout, available)
                now = self.clock()
                try:
                    grant = authorize_task(self.store, ACTIONS[data['kind']], payload['revision'], payload['scope_paths'], now=now)
                except PermissionError:
                    continue
                data.update(status='running', worker=worker, lease_id='lease-' + uuid.uuid4().hex,
                            lease_expires=now + min(lease_seconds, timeout), started_at=now, finished_at=None,
                            attempt=data['attempt'] + 1, reserved_seconds=timeout,
                            attempt_budget_seconds=timeout, authorization_id=grant)
                data.setdefault('attempts', []).append(dict(attempt=data['attempt'], worker=worker,
                    lease_id=data['lease_id'], started_at=now, status='running'))
                self.store.set_meta('status', 'running')
                return self.store.put_record('task', task['id'], data, actor=worker, event='task.claimed')
            return None

    def _leased(self, task_id, worker, lease_id, allow_stopped=False):
        task = self.store.get_record('task', task_id)
        if task is None:
            raise ValueError('Unknown task')
        data = task['data']
        if data['worker'] != worker or data['lease_id'] != lease_id:
            raise ValueError('Worker or lease does not match')
        stopped = allow_stopped and data.get('execution', {}).get('terminated') is True
        if data['status'] != 'running' or (self.clock() >= data['lease_expires'] and not stopped):
            raise ValueError('Lease is not active')
        return task

    def _finish(self, task, status, result=None, error=None):
        data = task['data']
        now = self.clock()
        if data['status'] == 'running':
            elapsed = min(data.get('attempt_budget_seconds', data['payload']['timeout_seconds']),
                          max(0, now - data['started_at']))
            data['spent_seconds'] = data.get('spent_seconds', 0) + elapsed
            self.store.set_meta('spent_seconds', self.store.get_meta('spent_seconds', 0) + elapsed)
            history = data.setdefault('attempts', [])
            if history:
                history[-1].update(status=status, finished_at=now, result=result, error=error, spent_seconds=elapsed)
        data.update(status=status, finished_at=now, result=result, error=error, reserved_seconds=0)
        return self.store.put_record('task', task['id'], data, actor=data.get('worker') or 'operator', event='task.' + status)

    def complete_task(self, task_id, worker, lease_id, result):
        result = _object(result)
        with self.store.transaction():
            task = self.store.get_record('task', task_id)
            if task and task['data']['status'] == 'completed' and task['data']['worker'] == worker and task['data']['lease_id'] == lease_id and task['data']['result'] == result:
                return task
            task = self._leased(task_id, worker, lease_id)
            if task['data'].get('cancellation_requested'):
                return self._finish(task, 'cancelled', error=task['data']['cancellation_requested'])
            if task['data']['payload']['revision'] != self.store.get_meta('revision'):
                raise ValueError('Task revision is not current')
            completed = self._finish(task, 'completed', result=result)
            if self.result_handler is not None:
                self.result_handler(completed, result)
            return completed

    def fail_task(self, task_id, worker, lease_id, error):
        _text(error, 'error')
        with self.store.transaction():
            task = self._leased(task_id, worker, lease_id, allow_stopped=True)
            if task['data'].get('cancellation_requested'):
                return self._finish(task, 'cancelled', error=task['data']['cancellation_requested'])
            return self._finish(task, 'failed', error=error)

    def renew_task(self, task_id, worker, lease_id, lease_seconds=300):
        _positive(lease_seconds, 'lease_seconds')
        with self.store.transaction():
            task = self._leased(task_id, worker, lease_id)
            data = task['data']
            data['lease_expires'] = min(self.clock() + lease_seconds, data['started_at'] + data.get('attempt_budget_seconds', data['payload']['timeout_seconds']))
            return self.store.put_record('task', task_id, data, actor=worker, event='task.renewed')

    def cancel_task(self, task_id, reason):
        _text(reason, 'reason')
        with self.store.transaction():
            task = self.store.get_record('task', task_id)
            if not task or task['data']['status'] in TERMINAL | {'uncertain'}:
                raise ValueError('Task is not cancellable')
            if task['data']['status'] == 'running':
                task['data']['cancellation_requested'] = reason
                return self.store.put_record('task', task_id, task['data'], actor='operator',
                                             event='task.cancellation_requested')
            return self._finish(task, 'cancelled', error=reason)

    def retry_task(self, task_id, reason):
        _text(reason, 'reason')
        with self.store.transaction():
            task = self.store.get_record('task', task_id)
            if not task or task['data']['status'] not in ('failed', 'cancelled'):
                raise ValueError('Only failed or cancelled tasks may be retried')
            data = task['data']
            if data['payload']['revision'] != self.store.get_meta('revision'):
                raise ValueError('Task revision is not current')
            if not self._challenge_is_current(data):
                raise ValueError('Challenge context is stale; schedule a replacement')
            if self._attempt_seconds(data, self.store.list_records('task')) <= 0:
                raise ValueError('task budget exhausted; new task/run needed')
            execution = data.get('execution')
            if execution and execution.get('terminated') is not True:
                raise ValueError('Prior worker termination must be confirmed before retry')
            if data.get('attempts') and execution:
                data['attempts'][-1]['execution'] = execution
            data.setdefault('retries', []).append({'reason': reason, 'at': self.clock(),
                                                  'after_attempt': data['attempt']})
            data.update(status='pending', worker=None, lease_id=None, lease_expires=None,
                        started_at=None, finished_at=None, result=None, error=None, reserved_seconds=0)
            data.pop('execution', None)
            data.pop('cancellation_requested', None)
            return self.store.put_record('task', task_id, data, actor='operator', event='task.retried')

    def pause(self, reason):
        _text(reason, 'reason')
        with self.store.transaction():
            self.store.set_meta('status', 'paused')
            self.store.append_event('run.paused', 'run', {'reason': reason}, actor='operator')
        return self.status()

    def resume(self):
        with self.store.transaction():
            if self.store.get_meta('status') != 'paused':
                raise ValueError('Run is not paused')
            self.store.set_meta('status', 'running')
            self.store.append_event('run.resumed', 'run', {}, actor='operator')
        return self.status()

    def status(self):
        return {'status': self.store.get_meta('status'), 'revision': self.store.get_meta('revision'),
                'spent_seconds': self.store.get_meta('spent_seconds', 0),
                'budget_seconds': self.store.get_meta('budget_seconds'), 'tasks': self.store.list_records('task')}

    def record_review(self, reviewer, revision, verdict, evidence_ids=(), task_id=None):
        _text(reviewer, 'reviewer')
        if verdict not in ('APPROVED', 'BLOCKED'):
            raise ValueError('Invalid verdict')
        evidence_ids = list(evidence_ids)
        if not evidence_ids or not all(isinstance(e, str) and e.strip() for e in evidence_ids):
            raise ValueError('Review requires evidence references')
        with self.store.transaction():
            if revision != self.store.get_meta('revision'):
                raise ValueError('Review revision is not current')
            if task_id is not None:
                task = self.store.get_record('task', task_id)
                if not task or task['data']['status'] != 'completed' or task['data']['kind'] != 'review' or task['data']['worker'] != reviewer or task['data']['payload']['revision'] != revision:
                    raise ValueError('Review must bind a completed review task and its worker')
            data = dict(reviewer=reviewer, revision=revision, verdict=verdict, evidence_ids=evidence_ids,
                        task_id=task_id, recorded_at=self.clock())
            identity = json.dumps([reviewer, revision], ensure_ascii=False).encode('utf-8')
            review_id = 'review-' + hashlib.sha256(identity).hexdigest()
            return self.store.put_record('review', review_id, data,
                                         actor=reviewer, event='review.recorded')

    def readiness(self):
        revision = self.store.get_meta('revision')
        blockers = []
        reviews = [r['data'] for r in self.store.list_records('review') if r['data']['revision'] == revision]
        approved = {r['reviewer'] for r in reviews if r['verdict'] == 'APPROVED' and r.get('evidence_ids')}
        if len(approved) < 2:
            blockers.append('Two independent current approved reviews are required')
        if any(r['verdict'] == 'BLOCKED' for r in reviews):
            blockers.append('A current review is blocked')
        if self.store.get_meta('status') not in ('ready', 'running', 'complete'):
            blockers.append('Run is not ready for approval')
        if any((t['data']['status'] == 'running' and t['data']['kind'] == 'execute') or
               (t['data']['status'] in ('pending', 'running') and
                t['data']['payload']['revision'] == revision and
                t['data']['kind'] in ('execute', 'review', 'challenge', 'specialist'))
               for t in self.store.list_records('task')):
            blockers.append('Current execution or review tasks remain active')
        if any(t['data']['status'] == 'uncertain' for t in self.store.list_records('task')):
            blockers.append('Uncertain task effects require operator resolution')
        blockers.extend(finding_gaps(self.store))
        return {'result': 'BLOCKED' if blockers else 'APPROVED', 'revision': revision,
                'blockers': blockers, 'approved_reviewers': sorted(approved)}
