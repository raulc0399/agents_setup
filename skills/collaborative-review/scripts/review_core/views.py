"""Read-only, coherent operator views of a pinned review run."""
import json
import socket
import time
import unicodedata


def status_view(store, coordinator=None):
    """Return JSON-compatible state and concrete next actions without mutations."""
    from .coordinator import Coordinator
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
        availability = (coordinator or Coordinator(store)).availability()
        available = {item['task_id']: item for item in availability['tasks']}
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
                item = available[row['id']]
                if item['ready']:
                    decisions.append({'action': 'run', 'id': row['id'], 'reason': f"Run worker {item['worker']}"})
                for blocker in item['blockers']:
                    decisions.append({'action': blocker['action'], 'id': row['id'],
                                      'reason': blocker['reason'], 'owner': blocker['owner']})
        decisions.extend({'action': 'respond', 'id': row['id'], 'reason': row['data']['text']} for row in questions)
        decisions.extend({'action': 'resolve gaps', 'id': reviewer, 'reason': ', '.join(paths)}
                         for reviewer, paths in uncovered.items() if paths)
        if status == 'paused':
            decisions.append({'action': 'resume', 'id': 'run', 'reason': 'Run is paused; resolve operator decisions before resuming'})
        budget = store.get_meta('budget_seconds', 0)
        spent = store.get_meta('spent_seconds', 0)
        reserved = sum(row['data'].get('reserved_seconds', 0) for row in tasks)
        transitions = {}
        for event in store.events():
            data = event['payload'].get('data', {})
            if event['payload'].get('kind') != 'task' or 'status' not in data:
                continue
            previous = transitions.get(event['record_id'])
            if previous is None or previous['state'] != data['status']:
                transitions[event['record_id']] = {'state': data['status'], 'at': event['at'], 'event': event['kind']}
        details = []
        task_ids = [row['id'] for row in current]
        for row in current:
            task = row['data']
            item = available.get(row['id'], {})
            blockers = item.get('blockers', []) if task['status'] == 'pending' else []
            state = task['status']
            if state == 'pending':
                state = 'queued' if item.get('ready') else 'blocked'
            elif state == 'uncertain' or (state == 'running' and task.get('lease_expires', float('inf')) <= now):
                state = 'recovery required'
                blockers = [{'reason': decision['reason'], 'action': decision['action'], 'owner': 'operator'}
                            for decision in decisions if decision['id'] == row['id']
                            and decision['action'] in ('recover', 'inspect termination', 'resume')]
            progress = task.get('progress')
            if progress and progress.get('attempt') == task.get('attempt'):
                progress = {**progress, 'age_seconds': max(0, now - progress['at'])}
            else:
                progress = None
            short_id = row['id']
            if len(short_id) > 24:
                length = 12
                while length < len(row['id']) and any(
                        identity != row['id'] and identity.startswith(row['id'][:length]) for identity in task_ids):
                    length += 1
                short_id = row['id'][:length]
            details.append({'id': row['id'], 'short_id': short_id, 'role': task['payload'].get('reviewer') or task.get('worker') or item.get('worker'),
                            'kind': task['kind'], 'state': state, 'scope': task['payload'].get('scope_paths', []),
                            'reason': '; '.join(blocker['reason'] for blocker in blockers),
                            'next_action': '; '.join(blocker['action'] for blocker in blockers) or
                                           ('Run worker ' + item['worker'] if state == 'queued' else ''),
                            'wait_owner': ', '.join(dict.fromkeys(blocker['owner'] for blocker in blockers)),
                            'reported_progress': progress, 'last_transition': transitions.get(row['id'])})
        return {'status': status, 'revision': revision, 'tasks': tasks,
                'active_tasks': [row for row in current if row['data']['status'] == 'running'],
                'queued_tasks': [row for row in current if row['data']['status'] == 'pending' and available[row['id']]['ready']],
                'blocked_tasks': [row for row in current if row['data']['status'] == 'pending' and not available[row['id']]['ready']],
                'completed_tasks': [row for row in current if row['data']['status'] == 'completed'],
                'task_details': details, 'availability': availability,
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
    working = sum(task['state'] == 'running' for task in view['task_details'])
    lines = [f"Run {view['status']} | revision {view['revision']}",
             f"Work: {working} working, {len(view['queued_tasks'])} queued, {len(view['blocked_tasks'])} blocked, {len(view['failed_tasks'])} failed, {len(view['uncertain_tasks'])} uncertain",
             f"Budget seconds: {budget['remaining_seconds']} remaining, {budget['reserved_seconds']} reserved, {budget['spent_seconds']} spent / {budget['budget_seconds']}"]
    for task in view['task_details']:
        if task['state'] in ('completed', 'cancelled'):
            continue
        lines.append(f"Task {task['short_id']}: {task['state']} ({task['role']}; {task['kind']}; {', '.join(task['scope'])})")
        if task['reason']:
            lines.append(f"Waiting: {task['reason']} | owner: {task['wait_owner']} | next: {task['next_action']}")
        progress = task['reported_progress']
        lines.append(f"Last reported progress: {progress['summary']} ({int(progress['age_seconds'])}s ago)"
                     if progress else 'No progress reported')
    described = {task['id'] for task in view['task_details'] if task['reason']}
    for decision in view['operator_decisions']:
        if decision['id'] in described and decision['action'] != 'resolve gaps':
            continue
        lines.append(f"Action: {decision['action']} {decision['id']} — {decision['reason']}")
    approval = view['approval']
    lines.append(f"Approval: {approval['result']}")
    for blocker in approval['blockers']:
        if isinstance(blocker, str):
            lines.append('Reason: ' + blocker)
            continue
        subject = blocker.get('reviewer') or blocker.get('id') or ', '.join(blocker.get('task_ids', []) or blocker.get('paths', []))
        lines.append('Reason: ' + blocker.get('reason', blocker.get('kind', 'Approval gap')) +
                     (' (' + subject + ')' if subject else ''))
    return '\n'.join(_terminal(line) for line in lines)
