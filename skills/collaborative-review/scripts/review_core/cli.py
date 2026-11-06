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
READ_COMMANDS = frozenset(('check', 'status', 'inspect', 'export', 'handoff'))


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
    authorize = commands.add_parser('authorize', help='operator confirms scoped actions interactively')
    authorize.add_argument('--actions', nargs='+', default=['review', 'inspect'])
    authorize.add_argument('--paths', nargs='+')
    authorize.add_argument('--expires-at', type=_positive)
    authorize.add_argument('--revoke')
    authorize.add_argument('--reason', default='operator revoked authorization')
    plan = commands.add_parser('plan', help='enqueue two independent reviews')
    plan.add_argument('--risk', action='append', default=[])
    plan.add_argument('--specialist', action='append', default=[])
    work = commands.add_parser('work', help='run one task with a trusted JSON stdin/stdout adapter')
    work.add_argument('--worker', required=True)
    work.add_argument('--timeout-seconds', type=_positive, default=DEFAULT_TASK_SECONDS)
    work.add_argument('worker_command', nargs=argparse.REMAINDER, help='-- executable [arguments]')
    task = commands.add_parser('task', help='manual task lifecycle operations')
    task.add_argument('action', choices=('enqueue', 'claim', 'submit', 'fail', 'cancel', 'renew', 'retry'))
    task.add_argument('--data', required=True, help='JSON object file, or - for stdin')
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
    commands.add_parser('handoff', help='print a checkpoint for a replacement worker')
    commands.add_parser('revise', help='capture changed inputs and invalidate prior approvals')
    proposal = commands.add_parser('protocol-proposal', help='record a change for a future protocol')
    proposal.add_argument('--text', required=True)
    proposal.add_argument('--evidence', action='append', default=[])
    doctor = commands.add_parser('doctor', help='check local runtime prerequisites')
    doctor.add_argument('--project', type=Path, default=Path.cwd())
    doctor.add_argument('--legacy-tmux', action='store_true')
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
                    'next': 'operator authorization is required before work can be claimed'}
        except (ValueError, OSError, StoreError):
            store.set_meta('status', 'setup_failed')
            raise


def _authorize(store, args):
    from .authorization import record_authorization, revoke_authorization

    details = {'revision': store.get_meta('revision'), 'actions': args.actions,
               'paths': args.paths or store.get_meta('paths'), 'expires_at': args.expires_at}
    if args.revoke:
        details = {'revoke': args.revoke, 'reason': args.reason}
    if not sys.stdin.isatty():
        raise PermissionError('authorization requires an operator terminal; unattended approval is unavailable')
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
    from . import findings

    def ingest(task, result):
        data = task['data']
        revision = data['payload']['revision']
        author = data['worker']
        writers = {'findings': findings.create_finding, 'questions': findings.create_question,
                   'options': findings.create_option, 'coverage': findings.create_coverage}
        for kind, writer in writers.items():
            entries = result.get(kind, [])
            if not isinstance(entries, list):
                raise ValueError(kind + ' must be a list')
            for entry in entries:
                if not isinstance(entry, dict):
                    raise ValueError(kind + ' entries must be objects')
                if entry.get('revision', revision) != revision or entry.get('author', author) != author:
                    raise ValueError('worker result identity or revision does not match its lease')
                writer(store, **{**entry, 'author': author, 'revision': revision})
        if data['kind'] == 'review':
            coordinator.record_review(author, revision, result.get('verdict'),
                                      result.get('evidence_ids', []), task_id=task['id'])

    coordinator = Coordinator(store, result_handler=ingest)
    return coordinator


def readiness(store, coordinator=None):
    """Combine independent review decisions with all required approval gates."""
    with store.read_transaction():
        return _readiness(store, coordinator)


def _readiness(store, coordinator):
    from .assumptions import assumption_gaps
    from .challenge import challenge_gaps
    from .research import research_gaps

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
        return run_next_task(coordinator, args.worker, command, timeout_seconds=args.timeout_seconds)
    if args.command == 'task':
        actions = {'enqueue': coordinator.enqueue_task, 'claim': coordinator.claim_task,
                   'submit': coordinator.complete_task, 'fail': coordinator.fail_task,
                   'cancel': coordinator.cancel_task, 'renew': coordinator.renew_task,
                   'retry': coordinator.retry_task}
        return actions[args.action](**_object(args.data))
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
        return handoff(store)
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
            result = diagnose(args.project, worker_command=command or None, legacy_tmux=args.legacy_tmux)
        elif args.command == 'evaluate':
            from .evaluation import evaluate
            result = evaluate(_read_json(args.cases), _read_json(args.results))
        else:
            with Store(_run_dir(args), readonly=args.command in READ_COMMANDS) as store:
                if args.command != 'inspect':
                    _require_snapshot(store)
                result = _dispatch(store, args)
        if args.command == 'status' and not args.json:
            from .views import format_status
            print(format_status(result))
        else:
            print(json.dumps(result, indent=2, allow_nan=False))
        if args.command in ('check', 'finish') and result['result'] != 'APPROVED':
            return 1
        if args.command == 'doctor' and not result['ok']:
            return 1
        return 0
    except (ValueError, TypeError, OSError, StoreError, sqlite3.Error, EOFError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 2
