"""Command-line entrypoint and integration for local review runs."""

import argparse
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import sys
import time

from .protocol import freeze_protocol, verify_protocol
from .snapshots import create_snapshot, verify_snapshot, changed_paths
from .store import Store, StoreError


APPROVAL_SCOPES = ('technical-plan', 'implementation-ready', 'report-ready')
RUN_ID = re.compile(r'[A-Za-z0-9][A-Za-z0-9_.-]*')
DEFAULT_BUDGET_SECONDS = 3600
DEFAULT_TASK_SECONDS = 300
READ_COMMANDS = frozenset(('check', 'status', 'inspect', 'export', 'handoff', 'notifications'))


def _positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError('must be finite and positive')
    return number


def _parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path('.reviews'), help='run collection directory')
    parser.add_argument('--run', help='unique run id')
    commands = parser.add_subparsers(dest='command', required=True)
    create = commands.add_parser('create', help='capture inputs and pin executable protocol')
    create.add_argument('--project', type=Path, default=Path.cwd())
    create.add_argument('--paths', nargs='+', required=True)
    create.add_argument('--approval-scope', choices=APPROVAL_SCOPES, default='technical-plan')
    create.add_argument('--budget-seconds', type=_positive, default=DEFAULT_BUDGET_SECONDS)
    create.add_argument('--max-workers', type=int, choices=range(1, 33), default=2)
    create.add_argument('--participants', nargs='+', help='explicit participants for task-independent messaging')
    authorize = commands.add_parser('authorize', help='record chat approval or confirm scoped actions interactively')
    authorize.add_argument('--chat-approval', help='user approval JSON file, or - for stdin')
    authorize.add_argument('--actions', nargs='+')
    authorize.add_argument('--paths', nargs='+')
    authorize.add_argument('--expires-at', type=_positive)
    authorize.add_argument('--revoke')
    authorize.add_argument('--reason', default='operator revoked authorization')
    plan = commands.add_parser('plan', help='enqueue two independent reviews')
    plan.add_argument('--risk', action='append', default=[])
    plan.add_argument('--specialist', action='append', default=[])
    work = commands.add_parser('work', help='run one task: helper JSON input, formal AASM output')
    work.add_argument('--worker', required=True)
    work.add_argument('--dialect', default='aasm-1')
    work.add_argument('--timeout-seconds', type=_positive, default=DEFAULT_TASK_SECONDS)
    work.add_argument('worker_command', nargs=argparse.REMAINDER, help='-- executable [arguments]')
    dispatch = commands.add_parser('dispatch', help='run ready workers and follow-ups without routine output')
    dispatch.add_argument('--dialect', default='aasm-1')
    dispatch.add_argument('--timeout-seconds', type=_positive, default=DEFAULT_TASK_SECONDS)
    dispatch.add_argument('--recipient', default='coordinator', help='recipient of the final queued update')
    dispatch.add_argument('worker_command', nargs=argparse.REMAINDER, help='-- executable [arguments]')
    poll = commands.add_parser('poll', help='wait for peer messages and assigned work in an active agent chat')
    poll.add_argument('--worker', required=True)
    poll.add_argument('--after', type=int, default=0)
    poll.add_argument('--wait-seconds', type=_positive, default=30)
    poll.add_argument('--timeout-seconds', type=_positive, default=DEFAULT_TASK_SECONDS,
                      help='follow-up execution budget, independent of the polling wait')
    task = commands.add_parser('task', help='manual task lifecycle operations')
    task.add_argument('action', choices=('enqueue', 'claim', 'submit', 'fail', 'cancel', 'renew', 'retry', 'progress'))
    task.add_argument('--data', help='administrative JSON request file, or - for stdin')
    task.add_argument('--body', help='AASM task submission file, or - for stdin')
    task.add_argument('--worker')
    task.add_argument('--task-id')
    task.add_argument('--lease-id')
    task.add_argument('--dialect', default='aasm-1')
    message = commands.add_parser('message', help='send or read formal agent messages')
    message_actions = message.add_subparsers(dest='message_action', required=True)
    send = message_actions.add_parser('send')
    send.add_argument('--worker', required=True)
    send.add_argument('--to', required=True)
    send.add_argument('--body', required=True)
    send.add_argument('--task-id')
    send.add_argument('--lease-id')
    send.add_argument('--reply-to')
    send.add_argument('--id')
    send.add_argument('--dialect', default='aasm-1')
    inbox = message_actions.add_parser('inbox')
    inbox.add_argument('--worker', required=True)
    inbox.add_argument('--after', type=int, default=0)
    discussion = commands.add_parser('discussion', help='defer user-facing updates while discussion is active')
    discussion.add_argument('action', choices=('begin', 'end'))
    discussion.add_argument('--recipient', required=True)
    notifications = commands.add_parser('notifications', help='read queued human summaries without waking a chat')
    notifications.add_argument('--recipient', required=True)
    notifications.add_argument('--after', type=int, default=0)
    dialect = commands.add_parser('dialect', help='define, test and agree immutable language extensions')
    dialect_actions = dialect.add_subparsers(dest='dialect_action', required=True)
    propose = dialect_actions.add_parser('propose')
    propose.add_argument('--worker', required=True)
    propose.add_argument('--manifest', required=True)
    test = dialect_actions.add_parser('test')
    test.add_argument('--worker', required=True)
    test.add_argument('--id', required=True)
    test.add_argument('--readback', required=True)
    ack = dialect_actions.add_parser('ack')
    ack.add_argument('--worker', required=True)
    ack.add_argument('--id', required=True)
    for action in ('show', 'export'):
        show = dialect_actions.add_parser(action)
        show.add_argument('--id', required=True)
    record = commands.add_parser('record', help='validate and persist a structured record')
    record.add_argument('kind', choices=('finding', 'finding-update', 'question', 'answer', 'option',
                                       'question-update', 'option-update', 'coverage', 'coverage-update',
                                       'assumption', 'assumption-verify', 'source',
                                       'claim', 'claim-verify', 'recommendation', 'review'))
    record.add_argument('--data', required=True, help='JSON object file, or - for stdin')
    inspect = commands.add_parser('inspect', help='read or search immutable input files')
    inspect.add_argument('--paths', nargs='+')
    inspect.add_argument('--pattern')
    commands.add_parser('challenge', help='enqueue a final independent integration review')
    commands.add_parser('check', help='show approval and integrity blockers')
    commands.add_parser('finish', help='complete a run only after all approval checks pass')
    commands.add_parser('export', help='regenerate reports from stored records and events')
    status = commands.add_parser('status', help='show decisions, work, coverage and budgets')
    status.add_argument('--json', action='store_true')
    pause = commands.add_parser('pause')
    pause.add_argument('--reason', required=True)
    commands.add_parser('resume', help='recover expired work without repeating uncertain writes')
    recover = commands.add_parser('recover', help='operator resolves uncertain execution effects')
    recover.add_argument('--data', required=True, help='JSON task_id, outcome and evidence')
    recover.add_argument('--confirm-termination', action='store_true',
                         help='confirm an unobservable worker stopped before retrying it')
    handoff = commands.add_parser('handoff', help='print a checkpoint for a replacement worker')
    handoff.add_argument('--worker', help='show this worker\'s details and only peer task status')
    commands.add_parser('revise', help='capture changed inputs and invalidate prior approvals')
    proposal = commands.add_parser('protocol-proposal', help='record a change for a future protocol')
    proposal.add_argument('--text', required=True)
    proposal.add_argument('--evidence', action='append', default=[])
    doctor = commands.add_parser('doctor', help='check local runtime prerequisites')
    doctor.add_argument('--project', type=Path, default=Path.cwd())
    doctor.add_argument('worker_command', nargs=argparse.REMAINDER)
    evaluate = commands.add_parser('evaluate', help='score supplied runs against known cases')
    evaluate.add_argument('--cases', type=Path, required=True)
    evaluate.add_argument('--results', type=Path, required=True)
    return parser


def _read_json(path):
    def invalid_constant(value):
        raise ValueError('nonfinite JSON number: ' + value)

    text = sys.stdin.read() if str(path) == '-' else Path(path).read_text(encoding='utf-8')
    return json.loads(text, parse_constant=invalid_constant)


def _object(path):
    value = _read_json(path)
    if not isinstance(value, dict):
        raise ValueError('request must be a JSON object')
    return value


def _read_body(path):
    return sys.stdin.read() if str(path) == '-' else Path(path).read_text(encoding='utf-8')


def _run_dir(args):
    if not args.run or not RUN_ID.fullmatch(args.run):
        raise ValueError('--run must be a simple unique run id')
    root = args.root.absolute()
    run = root / args.run
    if run.resolve() != run or root.resolve() != root:
        raise ValueError('run paths must not contain symlinks')
    return run


def _snapshot_errors(store):
    return verify_snapshot(store.get_meta('snapshot'), expected_run_dir=store.run_dir,
                           expected_project_root=store.get_meta('project_root'),
                           expected_paths=store.get_meta('paths'),
                           expected_revision=store.get_meta('revision'))


def _require_snapshot(store):
    errors = _snapshot_errors(store)
    if errors:
        raise ValueError('snapshot integrity: ' + '; '.join(errors))


def _pin_runtime(store, argv):
    errors = verify_protocol(store.run_dir)
    if errors:
        raise ValueError('protocol integrity: ' + '; '.join(errors))
    manifest = _read_json(store.run_dir / 'protocol' / 'manifest.json')
    if manifest != store.get_meta('protocol') or manifest['policies'] != store.get_meta('policies'):
        raise ValueError('protocol does not match the run\'s pinned manifest')
    pinned = store.run_dir / 'protocol' / 'review.py'
    if Path(__file__).resolve().parent.parent == pinned.parent:
        return
    store.close()
    os.execv(sys.executable, [sys.executable, '-B', str(pinned), *argv])


def _bootstrap(argv):
    """Choose a run's protocol before interpreting its command grammar."""
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--root', type=Path, default=Path('.reviews'))
    parser.add_argument('--run')
    parser.add_argument('command', nargs='?')
    parser.add_argument('command_args', nargs=argparse.REMAINDER)
    args, _ = parser.parse_known_args(argv)
    if args.run is None or args.command in ('create', 'doctor', 'evaluate'):
        return
    with Store(_run_dir(args), readonly=True) as store:
        _pin_runtime(store, argv)


def _create(args):
    run = _run_dir(args)
    project = args.project.absolute()
    policies = {'approval_scope': args.approval_scope, 'budget_seconds': args.budget_seconds,
                'max_workers': args.max_workers, 'challenge_required': True}
    metadata = {**policies, 'project_root': str(project), 'paths': args.paths,
                'status': 'initializing', 'created_at': time.time(), 'spent_seconds': 0}
    if args.participants is not None:
        if len(set(args.participants)) != len(args.participants) or any(
                not name.strip() or name != name.strip() or name == 'coordinator' for name in args.participants):
            raise ValueError('participants must be unique nonempty worker names')
        metadata['participants'] = args.participants
    with Store(run, create=True, metadata=metadata) as store:
        try:
            snapshot = create_snapshot(project, args.paths, run.parent, run.name)
            protocol = freeze_protocol(run, Path(__file__).resolve().parent.parent, policies)
            with store.transaction():
                store.set_meta('snapshot', snapshot)
                store.set_meta('revision', snapshot['revision'])
                store.set_meta('paths', snapshot['paths'])
                store.set_meta('policies', policies)
                store.set_meta('protocol', protocol)
                store.set_meta('status', 'ready')
                store.append_event('run.created', run.name, {'revision': snapshot['revision'], 'policies': policies})
            return {'run': str(run), 'revision': snapshot['revision'], 'status': 'ready',
                    'next': 'user approval in chat or an operator terminal is required before work can be claimed'}
        except (ValueError, OSError, StoreError):
            store.set_meta('status', 'setup_failed')
            raise


def _authorize(store, args):
    from .authorization import record_authorization, record_chat_authorization, revoke_authorization

    if args.chat_approval is not None:
        if (args.actions is not None or args.paths is not None or args.expires_at is not None
                or args.revoke is not None or args.reason != 'operator revoked authorization'):
            raise ValueError('--chat-approval cannot be combined with terminal authorization options')
        return record_chat_authorization(store, _object(args.chat_approval))

    details = {'revision': store.get_meta('revision'), 'actions': args.actions or ['review', 'inspect'],
               'paths': args.paths or store.get_meta('paths'), 'expires_at': args.expires_at}
    if args.revoke:
        details = {'revoke': args.revoke, 'reason': args.reason}
    if not sys.stdin.isatty():
        raise PermissionError('use --chat-approval to record user consent, or authorize in an operator terminal')
    print(json.dumps(details, indent=2), file=sys.stderr)
    print('Type APPROVE to record this operator decision: ', end='', file=sys.stderr, flush=True)
    if input().strip() != 'APPROVE':
        raise PermissionError('operator did not confirm authorization')
    if args.revoke:
        return revoke_authorization(store, args.revoke, reason=args.reason)
    return record_authorization(store, **details)


def _record(store, coordinator, kind, data):
    from . import findings
    from .assumptions import add_assumption, verify_assumption
    from .research import add_source, add_claim, verify_claim, add_recommendation

    writers = {'finding': findings.create_finding, 'finding-update': findings.update_finding,
               'question': findings.create_question, 'answer': findings.answer_question,
               'question-update': findings.update_question, 'option': findings.create_option,
               'option-update': findings.update_option, 'coverage': findings.create_coverage,
               'coverage-update': findings.update_coverage,
               'assumption': add_assumption, 'assumption-verify': verify_assumption,
               'source': add_source, 'claim': add_claim, 'claim-verify': verify_claim,
               'recommendation': add_recommendation}
    if kind == 'review':
        return coordinator.record_review(**data)
    return writers[kind](store, **data)


def _coordinator(store):
    from .coordinator import Coordinator
    from .aasm_records import prepare_result, apply_operations
    from .challenge import current_context_fingerprint
    from .messages import send_message

    def ingest(task, result):
        data = task['data']
        revision = data['payload']['revision']
        author = data['worker']
        challenge = data['kind'] == 'challenge' and 'context_fingerprint' in data['payload']
        if challenge and data['payload']['context_fingerprint'] != current_context_fingerprint(store):
            raise ValueError('Challenge context changed during the task')
        prepared = prepare_result(store, author, result.get('aasm'), result.get('dialect_id'))
        if json.dumps(prepared, sort_keys=True) != json.dumps(result, sort_keys=True):
            raise ValueError('Worker result differs from its parsed AASM body')
        if data['kind'] == 'discussion' and _supersede_discussion_result(store, task):
            return
        if data['kind'] == 'consensus':
            from .consensus import supersede_result, validate_result
            if supersede_result(store, task):
                return
            validate_result(task, prepared['operations'])
        review_task = data['kind'] in ('review', 'challenge', 'specialist')
        if review_task and 'verdict' not in prepared:
            raise ValueError('Review tasks require a final VOTE')
        apply_operations(store, author, revision, prepared['operations'], task_id=task['id'],
                         allow_vote=review_task, dialect_id=prepared['dialect_id'])
        if data['kind'] == 'discussion':
            _discussion_reply(store, task, prepared)
        if data['kind'] == 'consensus':
            from .consensus import publish_result
            publish_result(store, task, prepared)
        if data['kind'] == 'review':
            coordinator.record_review(author, revision, result.get('verdict'),
                                      result.get('evidence_ids', []), task_id=task['id'])
        send_message(store, author, 'coordinator', prepared['aasm'],
                     dialect_id=prepared['dialect_id'], task_id=task['id'], lease_id=data['lease_id'],
                     message_id='result-' + data['lease_id'])
        if challenge:
            # Bind only this atomic result's effects; later evidence still changes the context.
            data['completion_context_fingerprint'] = current_context_fingerprint(store)
            store.put_record('task', task['id'], data, actor=author, event='task.context_bound')

    coordinator = Coordinator(store, result_handler=ingest)
    return coordinator


def _supersede_discussion_result(store, task):
    """Keep a raced response for audit without applying it to changed questions."""
    data = task['data']
    assigned = data['payload'].get('question_ids', [])
    versions = data['payload'].get('question_versions')
    if (not isinstance(assigned, list) or not assigned or not isinstance(versions, dict)
            or set(versions) != set(assigned)
            or any(isinstance(version, bool) or not isinstance(version, int) or version < 1
                   for version in versions.values())):
        raise ValueError('Discussion requires versioned assigned questions')
    questions = [store.get_record('question', identity) for identity in assigned]
    if any(question is None or question['data']['revision'] != data['payload']['revision']
           for question in questions):
        raise ValueError('Discussion question is missing or stale')
    if all(question['data']['status'] == 'unanswered'
           and question['data']['content_version'] == versions[question['id']] for question in questions):
        return False
    reason = 'Discussion assignment changed before the response completed'
    data.update(status='cancelled', error=reason, discussion_superseded=True, result_disposition='superseded')
    if data.get('attempts'):
        data['attempts'][-1].update(status='cancelled', error=reason)
    store.put_record('task', task['id'], data, actor=data['worker'], event='discussion.superseded')
    return True


def _discussion_reply(store, task, result):
    from .messages import send_message

    data = task['data']
    payload = data['payload']
    source = store.get_record('message', payload.get('message_id', 'missing'))
    if (source is None or source['data']['revision'] != payload['revision']
            or source['data']['recipient'] != data['worker']
            or source['data']['sender'] != payload.get('reply_recipient')
            or source['id'] != payload.get('reply_to')):
        raise ValueError('Discussion must reply to its assigned current conversation')
    question_ids = payload.get('question_ids')
    if not isinstance(question_ids, list) or not question_ids:
        raise ValueError('Discussion requires assigned questions')
    questions = [store.get_record('question', identity) for identity in question_ids]
    if any(question is None or question['data']['revision'] != payload['revision'] for question in questions):
        raise ValueError('Discussion question is missing or stale')
    if any(question['data']['status'] != 'answered' for question in questions):
        if not any(operation['op'] == 'GAP' for operation in result['operations']):
            raise ValueError('Answer assigned questions or report an explicit GAP with the missing evidence')
    send_message(store, data['worker'], payload['reply_recipient'], result['aasm'],
                 dialect_id=result['dialect_id'], task_id=task['id'], lease_id=data['lease_id'],
                 reply_to=source['id'], message_id='reply-' + data['lease_id'])


def readiness(store, coordinator=None):
    """Combine independent review decisions with all required approval gates."""
    with store.read_transaction():
        return _readiness(store, coordinator)


def _readiness(store, coordinator):
    from .assumptions import assumption_gaps
    from .challenge import challenge_gaps
    from .research import research_gaps
    from .consensus import consensus_gaps

    coordinator = coordinator or _coordinator(store)
    result = coordinator.readiness()
    blockers = list(result['blockers'])
    unfinished = [task['id'] for task in store.list_records('task')
                  if task['data']['status'] in ('running', 'uncertain') or
                  (task['data']['status'] == 'pending' and
                   task['data']['payload']['revision'] == result['revision'])]
    if unfinished:
        blockers.append({'kind': 'task', 'task_ids': unfinished, 'reason': 'unfinished tasks remain'})
    blockers.extend(_snapshot_errors(store))
    files = {entry['path'] for entry in store.get_meta('snapshot')['files']}
    coverage = [entry['data'] for entry in store.list_records('coverage')
                if entry['data']['revision'] == result['revision'] and entry['data']['status'] == 'reviewed']
    for reviewer in result['approved_reviewers']:
        covered = {path for entry in coverage if entry['author'] == reviewer for path in entry['paths']}
        if files - covered:
            blockers.append({'kind': 'coverage', 'reviewer': reviewer, 'paths': sorted(files - covered),
                             'reason': 'approved reviewer has uncovered snapshot files'})
    blockers.extend(assumption_gaps(store))
    blockers.extend(consensus_gaps(store))
    if store.get_meta('challenge_required', True):
        blockers.extend(challenge_gaps(store))
    if store.get_meta('approval_scope') == 'report-ready':
        blockers.extend(research_gaps(store))
    result['blockers'] = blockers
    result['result'] = 'BLOCKED' if blockers else 'APPROVED'
    result['approval_scope'] = store.get_meta('approval_scope')
    return result


def _revise(store, coordinator):
    from .assumptions import invalidate_assumptions

    with store.transaction():
        tasks = store.list_records('task')
        if any(task['data']['status'] == 'running' for task in tasks):
            raise ValueError('finish or recover running tasks before revising inputs')
        previous = store.get_meta('snapshot')
        snapshot = create_snapshot(store.get_meta('project_root'), store.get_meta('paths'),
                                   store.run_dir.parent, store.run_dir.name)
        changed = changed_paths(previous, snapshot)
        if not changed:
            return {'revision': previous['revision'], 'changed_paths': []}
        for task in tasks:
            if task['data']['status'] == 'pending':
                coordinator.cancel_task(task['id'], 'input revision changed')
        store.set_meta('revision', snapshot['revision'])
        store.set_meta('snapshot', snapshot)
        store.set_meta('changed_paths', changed)
        store.set_meta('status', 'ready')
        invalidate_assumptions(store, changed, snapshot['revision'])
        store.append_event('run.revised', store.run_dir.name,
                           {'previous_revision': previous['revision'], 'revision': snapshot['revision'],
                            'changed_paths': changed}, actor='operator')
        return {'revision': snapshot['revision'], 'changed_paths': changed}


def _dispatch(store, args):
    coordinator = _coordinator(store)
    if args.command == 'authorize':
        return _authorize(store, args)
    if args.command == 'plan':
        from .delegation import plan_review
        return plan_review(store, coordinator, risks=args.risk, specialists=args.specialist)
    if args.command == 'work':
        from .runner import run_next_task
        command = args.worker_command
        if command and command[0] == '--':
            command = command[1:]
        result = run_next_task(coordinator, args.worker, command, timeout_seconds=args.timeout_seconds,
                               dialect_id=args.dialect)
        return result if result is not None else {'state': 'blocked', 'availability': coordinator.availability(args.worker)}
    if args.command == 'dispatch':
        from .dispatcher import dispatch_run
        command = args.worker_command
        if command and command[0] == '--':
            command = command[1:]
        return dispatch_run(store.run_dir, command, timeout_seconds=args.timeout_seconds,
                            dialect_id=args.dialect, recipient=args.recipient)
    if args.command == 'poll':
        from .dispatcher import poll_worker
        return poll_worker(store, args.worker, after=args.after, wait_seconds=args.wait_seconds,
                           timeout_seconds=args.timeout_seconds)
    if args.command == 'discussion':
        from .notifications import set_discussion_active
        return set_discussion_active(store, args.recipient, args.action == 'begin')
    if args.command == 'notifications':
        from .notifications import read_notifications
        return read_notifications(store, args.recipient, after=args.after)
    if args.command == 'task':
        if args.action == 'submit':
            from .aasm_records import prepare_result
            if args.data or not all((args.body, args.worker, args.task_id, args.lease_id)):
                raise ValueError('submit requires --body, --worker, --task-id and --lease-id')
            result = prepare_result(store, args.worker, _read_body(args.body), args.dialect)
            return coordinator.complete_task(args.task_id, args.worker, args.lease_id, result)
        if not args.data or any((args.body, args.worker, args.task_id, args.lease_id)):
            raise ValueError('Administrative task actions require only --data')
        actions = {'enqueue': coordinator.enqueue_task, 'claim': coordinator.claim_task,
                   'fail': coordinator.fail_task,
                   'cancel': coordinator.cancel_task, 'renew': coordinator.renew_task,
                   'retry': coordinator.retry_task, 'progress': coordinator.record_progress}
        request = _object(args.data)
        result = actions[args.action](**request)
        if args.action == 'claim' and result is None:
            return {'state': 'blocked', 'availability': coordinator.availability(
                request.get('worker'), request.get('capabilities'), task_id=request.get('task_id'))}
        return result
    if args.command == 'message':
        from .messages import send_message, read_messages
        from .aasm_records import apply_operations
        if args.message_action == 'inbox':
            return read_messages(store, args.worker, after=args.after)
        def apply(operations, envelope):
            return apply_operations(store, args.worker, envelope['revision'], operations,
                                    task_id=args.task_id, dialect_id=envelope['dialect_id'])
        return send_message(store, args.worker, args.to, _read_body(args.body),
                            dialect_id=args.dialect, reply_to=args.reply_to, message_id=args.id,
                            task_id=args.task_id, lease_id=args.lease_id, apply=apply)
    if args.command == 'dialect':
        from .dialects import (propose_dialect, test_dialect, acknowledge_dialect,
                               dialect_status, export_dialect, get_dialect)
        if args.dialect_action == 'propose':
            manifest = _object(args.manifest)
            fields = {'name', 'version', 'participants', 'definitions', 'examples'}
            if set(manifest) - fields - {'author', 'id', 'base', 'digest'}:
                raise ValueError('Unknown dialect manifest fields')
            if manifest.get('base', 'aasm-1') != 'aasm-1':
                raise ValueError('Unsupported base dialect')
            return propose_dialect(store, author=args.worker, dialect_id=manifest.get('id'),
                                   **{key: manifest[key] for key in fields if key in manifest})
        if args.dialect_action == 'test':
            return test_dialect(store, args.id, args.worker, _read_json(args.readback))
        if args.dialect_action == 'ack':
            return acknowledge_dialect(store, args.id, args.worker)
        if args.dialect_action == 'export':
            return export_dialect(store, args.id)
        manifest = get_dialect(store, args.id) if args.id == 'aasm-1' else export_dialect(store, args.id)
        return {'manifest': manifest, 'agreement': dialect_status(store, args.id)}
    if args.command == 'record':
        return _record(store, coordinator, args.kind, _object(args.data))
    if args.command == 'inspect':
        from .checks import inspect_snapshot
        return inspect_snapshot(store, paths=args.paths, pattern=args.pattern)
    if args.command == 'challenge':
        from .challenge import schedule_challenge
        return schedule_challenge(store, coordinator)
    if args.command == 'check':
        return readiness(store, coordinator)
    if args.command == 'finish':
        with store.transaction():
            result = readiness(store, coordinator)
            if result['result'] != 'APPROVED':
                return result
            if store.get_meta('status') != 'complete':
                store.set_meta('status', 'complete')
                store.append_event('run.completed', store.run_dir.name, result, actor='operator')
            return result
    if args.command == 'export':
        from .exports import export_run
        return export_run(store)
    if args.command == 'status':
        from .views import status_view
        return status_view(store, coordinator)
    if args.command == 'pause':
        return coordinator.pause(args.reason)
    if args.command == 'resume':
        from .recovery import resume_run
        return resume_run(store, coordinator)
    if args.command == 'recover':
        from .recovery import confirm_process_terminated, resolve_uncertain_task
        data = _object(args.data)
        if not sys.stdin.isatty():
            raise PermissionError('uncertain execution recovery requires an operator terminal')
        print(json.dumps(data, indent=2), file=sys.stderr)
        print('Type APPROVE to record this recovery decision: ', end='', file=sys.stderr, flush=True)
        if input().strip() != 'APPROVE':
            raise PermissionError('operator did not confirm recovery')
        if args.confirm_termination:
            return confirm_process_terminated(store, **data)
        return resolve_uncertain_task(store, **data)
    if args.command == 'handoff':
        from .recovery import handoff
        return handoff(store, worker=args.worker)
    if args.command == 'revise':
        return _revise(store, coordinator)
    if args.command == 'protocol-proposal':
        from .protocol import propose_change
        return propose_change(store, args.text, args.evidence)
    raise ValueError('unsupported command')


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    try:
        _bootstrap(argv)
        args = _parser().parse_args(argv)
        if args.command == 'create':
            result = _create(args)
        elif args.command == 'doctor':
            from .doctor import diagnose
            command = args.worker_command
            if command and command[0] == '--':
                command = command[1:]
            result = diagnose(args.project, worker_command=command or None)
        elif args.command == 'evaluate':
            from .evaluation import evaluate
            result = evaluate(_read_json(args.cases), _read_json(args.results))
        else:
            readonly = (args.command in READ_COMMANDS or
                        (args.command == 'message' and args.message_action == 'inbox') or
                        (args.command == 'dialect' and args.dialect_action in ('show', 'export')))
            with Store(_run_dir(args), readonly=readonly) as store:
                if args.command not in ('inspect', 'discussion', 'notifications'):
                    _require_snapshot(store)
                result = _dispatch(store, args)
        if args.command == 'status' and not args.json:
            from .views import format_status
            print(format_status(result))
        else:
            print(json.dumps(result, indent=2, allow_nan=False))
        if args.command in ('check', 'finish') and result['result'] != 'APPROVED':
            return 1
        if args.command == 'dispatch' and (result['state'] == 'blocked' or
                (result.get('plan_exists') and result['approval']['result'] != 'APPROVED')):
            return 1
        if args.command == 'doctor' and not result['ok']:
            return 1
        return 0
    except (ValueError, TypeError, OSError, StoreError, sqlite3.Error, EOFError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 2
