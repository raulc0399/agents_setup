"""Schedule bounded peer work only for visible explicit unanswered questions."""
import hashlib
import json
import math
from pathlib import PurePosixPath

from .messages import exchange_reviews, read_messages


QUESTION_OPERATIONS = ('Q', 'GAP', 'DISPUTE')


def _unanswered(store, question_id, revision):
    question = store.get_record('question', question_id)
    if question is None:
        raise ValueError('Message question has not been applied: ' + question_id)
    return (question['data'].get('revision') == revision
            and question['data'].get('status') == 'unanswered')


def schedule_discussions(store, coordinator, timeout_seconds=300):
    """Atomically reconcile question assignments; never retry failed work."""
    if coordinator.store is not store:
        raise ValueError('Coordinator must use the supplied store')
    if (isinstance(timeout_seconds, bool) or not isinstance(timeout_seconds, (int, float))
            or not math.isfinite(timeout_seconds) or timeout_seconds <= 0):
        raise ValueError('timeout_seconds must be positive and finite')
    with store.transaction():
        revision = store.get_meta('revision')
        snapshot = store.get_meta('snapshot', {})
        if not revision or snapshot.get('revision') != revision:
            raise ValueError('Snapshot must match current revision')
        paths = [entry['path'] for entry in snapshot.get('files', [])]
        if not paths or any(not isinstance(path, str) or not path or '\\' in path
                            or PurePosixPath(path).is_absolute() or '..' in PurePosixPath(path).parts
                            or str(PurePosixPath(path)) != path or path == '.' for path in paths):
            raise ValueError('Snapshot must contain canonical file paths')
        exchanges = {message['id'] for message in exchange_reviews(store)}
        recipients = {record['data']['recipient'] for record in store.list_records('message')
                      if record['data'].get('revision') == revision
                      and record['data']['recipient'] != 'coordinator'}
        tasks = []
        for recipient in sorted(recipients):
            visible = read_messages(store, recipient)
            for message in visible:
                data = message['data']
                if data['sender'] == recipient or data['sender'] == 'coordinator':
                    continue
                sender_task = store.get_record('task', data['task_id']) if data.get('task_id') else None
                discussion_reply = (data.get('reply_to') is not None and sender_task is not None
                                    and sender_task['data']['kind'] == 'discussion')
                questions = [operation for operation in data['operations']
                             if operation['op'] in QUESTION_OPERATIONS
                             and not (discussion_reply and operation['op'] == 'GAP')]
                if not questions:
                    continue
                questions = [operation for operation in questions
                             if _unanswered(store, operation['args']['id'], revision)]
                question_ids = sorted(operation['args']['id'] for operation in questions)
                question_records = [store.get_record('question', question_id) for question_id in question_ids]
                question_versions = {record['id']: record['data']['content_version']
                                     for record in question_records}
                source_tasks = [task for task in store.list_records('task')
                                if task['data']['kind'] == 'discussion'
                                and task['data']['payload'].get('revision') == revision
                                and task['data']['payload'].get('message_id') == message['id']]
                running = [task for task in source_tasks if task['data']['status'] == 'running']
                if running:
                    tasks.extend(running)
                    continue
                failed = [task for task in source_tasks if task['data']['status'] in ('failed', 'uncertain')
                          or (task['data']['status'] == 'cancelled'
                              and not task['data'].get('discussion_superseded'))]
                if failed:
                    tasks.extend(failed)
                    continue
                for task in source_tasks:
                    if (task['data']['status'] == 'pending'
                            and (sorted(task['data']['payload']['question_ids']) != question_ids
                                 or task['data']['payload'].get('question_versions') != question_versions)):
                        cancelled = coordinator.cancel_task(task['id'], 'Assigned questions changed or were answered')
                        store.put_record('task', task['id'],
                                         {**cancelled['data'], 'discussion_superseded': True},
                                         event='discussion.superseded', expected_version=cancelled['version'])
                if not questions:
                    continue
                identity = json.dumps([revision, message['id'], sorted(question_versions.items())], separators=(',', ':'))
                task_id = 'discussion-' + hashlib.sha256(identity.encode()).hexdigest()
                existing = store.get_record('task', task_id)
                if existing:
                    tasks.append(existing)
                    continue
                finding_ids = {finding_id for record in question_records
                               for finding_id in record['data'].get('finding_ids', [])}
                findings = []
                for finding_id in sorted(finding_ids):
                    finding = store.get_record('finding', finding_id)
                    if finding and finding['data'].get('revision') == revision:
                        findings.append(finding)
                context = dict(message_id=message['id'], sender=data['sender'],
                               dialect_id=data['dialect_id'], body=data['body'], questions=questions,
                               question_records=question_records, findings=findings)
                if message['id'] in exchanges:
                    context['review'] = dict(task_id=sender_task['id'], **sender_task['data']['result'])
                brief = ('Route the assigned questions to the relevant subagents in your own concern team; '
                         'reuse their evidence or start replacement subagents with the same concern brief. '
                         'The team lead checks and consolidates their replies. '
                         'Answer only these assigned unanswered questions, or report an explicit blocking GAP. '
                         'Check the intended behavior, relevant callers and failure paths; cite actual evidence '
                         'and disclose untested uncertainty. Return only AASM, using A question=<assigned-id>; '
                         'no VOTE is required. Do not create routine NOTE response loops. Source context: '
                         + json.dumps(context, sort_keys=True))
                timeout = timeout_seconds
                if message['id'] in exchanges:
                    plan = next(plan for plan in store.list_records('review_plan')
                                if data['task_id'] in plan['data']['task_ids'])
                    timeout = min(timeout, plan['data']['peer_review_budget_seconds'])
                payload = dict(worker=recipient, revision=revision, message_id=message['id'],
                               reply_to=message['id'], reply_recipient=data['sender'],
                               question_ids=question_ids, question_versions=question_versions,
                               scope_paths=sorted(set(paths)), brief=brief, timeout_seconds=timeout,
                               required_action='review')
                if message['id'] in exchanges:
                    payload['peer_review_plan_id'] = plan['id']
                tasks.append(coordinator.enqueue_task('discussion', payload, task_id=task_id))
        return tasks
