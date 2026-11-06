"""Read-only, coherent operator views of a pinned review run."""
import json
import socket
import time
import unicodedata


def status_view(store, coordinator=None):
    """Return JSON-compatible state and concrete next actions without mutations."""
    from .authorization import authorize_task
    from .cli import readiness
    from .recovery import _process_may_live

    with store.read_transaction():
        revision = store.get_meta('revision')
        status = store.get_meta('status')
        tasks = store.list_records('task')
        current = [row for row in tasks if row['data']['payload']['revision'] == revision]
        questions = [row for row in store.list_records('question')
                     if row['data']['revision'] == revision and row['data']['status'] != 'answered']
        approval = readiness(store, coordinator)
        files = {entry['path'] for entry in store.get_meta('snapshot', {}).get('files', [])}
        required_tasks = {task_id for plan in store.list_records('review_plan')
                          if plan['data']['request']['revision'] == revision
                          for task_id in plan['data']['task_ids']}
        reviewers = {row['data']['payload'].get('reviewer') or row['data'].get('worker')
                     for row in current if row['data']['kind'] == 'review'
                     and (row['data']['status'] != 'cancelled' or row['id'] in required_tasks)}
        reviewers.update(row['data']['reviewer'] for row in store.list_records('review')
                         if row['data']['revision'] == revision)
        coverage = [row['data'] for row in store.list_records('coverage')
                    if row['data']['revision'] == revision and row['data']['status'] == 'reviewed'
                    and row['data'].get('evidence')]
        uncovered = {reviewer: sorted(files - {path for row in coverage if row['author'] == reviewer
                                               for path in row['paths']})
                     for reviewer in sorted(reviewers - {None})}
        decisions = []
        now = time.time()
        for row in tasks:
            task = row['data']
            worker = task.get('worker') or task['payload'].get('reviewer') or row['id']
            if task['status'] == 'uncertain':
                decisions.append({'action': 'recover', 'id': row['id'], 'reason': 'Resolve uncertain execution effects with evidence'})
            elif task['status'] == 'running' and task.get('lease_expires', float('inf')) <= now:
                if _process_may_live(task):
                    execution = task['execution']
                    pid = execution.get('pid')
                    local_pid = (execution.get('host') == socket.gethostname() and
                                 isinstance(pid, int) and not isinstance(pid, bool) and pid > 0)
                    reason = ('Inspect and terminate the local worker, verify termination, then resume; '
                              'a live local process cannot be overridden' if local_pid else
                              'Inspect and verify worker termination; record evidence with '
                              'recover --confirm-termination --data JSON_FILE, then resume')
                    decisions.append({'action': 'inspect termination', 'id': row['id'], 'reason': reason})
                else:
                    decisions.append({'action': 'resume', 'id': row['id'], 'reason': 'Recover expired task lease'})
            elif row in current and (task['status'] == 'failed' or
                                     (task['status'] == 'cancelled' and row['id'] in required_tasks)):
                decisions.append({'action': 'retry', 'id': row['id'], 'reason': 'Use task retry --data JSON_FILE with task_id and reason; ' +
                                  str(task.get('error') or 'Required task unfinished')})
            elif row in current and task['status'] == 'pending':
                payload = task['payload']
                try:
                    authorize_task(store, payload.get('required_action', 'review'), revision,
                                   payload.get('scope_paths', []), now=now)
                except PermissionError as error:
                    decisions.append({'action': 'authorize', 'id': row['id'], 'reason': str(error)})
                decisions.append({'action': 'run', 'id': row['id'], 'reason': f'Run worker {worker} when dependencies and capacity permit'})
        decisions.extend({'action': 'respond', 'id': row['id'], 'reason': row['data']['text']} for row in questions)
        decisions.extend({'action': 'resolve gaps', 'id': reviewer, 'reason': ', '.join(paths)}
                         for reviewer, paths in uncovered.items() if paths)
        if status == 'paused':
            decisions.append({'action': 'resume', 'id': 'run', 'reason': 'Run is paused; resolve operator decisions before resuming'})
        budget = store.get_meta('budget_seconds', 0)
        spent = store.get_meta('spent_seconds', 0)
        reserved = sum(row['data'].get('reserved_seconds', 0) for row in tasks)
        return {'status': status, 'revision': revision, 'tasks': tasks,
                'active_tasks': [row for row in tasks if row['data']['status'] in ('pending', 'running')],
                'failed_tasks': [row for row in current if row['data']['status'] == 'failed'],
                'uncertain_tasks': [row for row in tasks if row['data']['status'] == 'uncertain'],
                'operator_decisions': decisions, 'uncovered_paths': uncovered,
                'budget': {'budget_seconds': budget, 'spent_seconds': spent,
                           'reserved_seconds': reserved, 'remaining_seconds': budget - spent - reserved},
                'approval': approval}


def _terminal(value):
    text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False, sort_keys=True)
    return ''.join(character if not unicodedata.category(character).startswith('C')
                   else f'\\u{ord(character):04x}' for character in text)


def format_status(view):
    """Render concise plain text; untrusted controls are always visible escapes."""
    budget = view['budget']
    lines = [f"Run {view['status']} | revision {view['revision']}",
             f"Work: {len(view['active_tasks'])} active, {len(view['failed_tasks'])} failed, {len(view['uncertain_tasks'])} uncertain",
             f"Budget seconds: {budget['remaining_seconds']} remaining, {budget['reserved_seconds']} reserved, {budget['spent_seconds']} spent / {budget['budget_seconds']}"]
    for row in view['active_tasks'] + view['failed_tasks'] + view['uncertain_tasks']:
        task = row['data']
        worker = task.get('worker') or task['payload'].get('reviewer') or 'unassigned'
        lines.append(f"Task {row['id']}: {task['status']} ({worker})")
    for decision in view['operator_decisions']:
        lines.append(f"Action: {decision['action']} {decision['id']} — {decision['reason']}")
    approval = view['approval']
    lines.append(f"Approval: {approval['result']}")
    for blocker in approval['blockers']:
        lines.append('Reason: ' + (blocker if isinstance(blocker, str) else json.dumps(blocker, ensure_ascii=False, sort_keys=True)))
    return '\n'.join(_terminal(line) for line in lines)
