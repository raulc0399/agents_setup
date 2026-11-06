#!/usr/bin/env python3
"""File-based transport for two locally running review agents. Python 3, Unix (fcntl)."""
import argparse
import contextlib
import datetime as dt
import fcntl
import hashlib
import html
import json
import os
import re
import shutil
import subprocess
from pathlib import Path
import sys
import time
import uuid

ROOT = Path('.reviews')
AGENTS = ('agent-1', 'agent-2')
DEFAULT_APPROVAL_SCOPE = 'implementation-ready'
APPROVAL_RESULTS = {'technical-plan': 'PLAN_APPROVED', 'implementation-ready': 'READY_TO_EXECUTE',
                    'report-ready': 'REPORT_APPROVED'}
CONTROL_KINDS = ('info', 'scope', 'stop', 'decision', 'logoff')
HEARTBEAT_INTERVAL = 5
PEER_STALE_AFTER = 120
WAKE_PREFIX = 'collaborative-review wake:'
WAKE_ENTER_DELAY = 0.5
WAKE_MODES = ('guarded', 'pane', 'none')
WAKE_QUIET_SECS = 4
AUTO_WAKE_SECS = 60
NUDGE_SECS = 300
SNAPSHOT_EXCLUDES = {'.git', '.reviews', '__pycache__'}
WORK_FOLDERS = ('reviews', 'research')
ROLE_KINDS = {'decision': 'reporter', 'scope': 'reporter', 'logoff': 'reporter'}
FINDING_ID = re.compile(r'\b([12][A-Z]+\d+)\b')
TRANSCRIPT_STYLE = ('body{font:14px/1.5 system-ui,sans-serif;margin:24px;max-width:1100px;color:#1d2228;background:#fff}'
                    'h2{margin-top:32px}section{border:1px solid #d8d5cc;border-radius:6px;padding:8px 12px;margin:10px 0}'
                    'pre{margin:4px 0;white-space:pre-wrap;font:12.5px/1.45 ui-monospace,monospace}'
                    '.m{color:#5d6670;font-size:12px}.a1{border-left:4px solid #0f7f78}.a2{border-left:4px solid #2457a6}'
                    'table{border-collapse:collapse}td{padding:2px 8px;vertical-align:top;font-size:13px}')
ANSWER_OPCODES = ('Q', 'RUN')


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec='seconds')


def atomic_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name('.' + path.name + '.' + uuid.uuid4().hex + '.tmp')
    with tmp.open('w', encoding='utf-8') as f:
        json.dump(data, f, indent=2)
        f.write('\n')
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def read_json(path, default=None):
    if not path.exists():
        return default
    return json.loads(path.read_text(encoding='utf-8'))


@contextlib.contextmanager
def locked(run):
    run.mkdir(parents=True, exist_ok=True)
    with (run / '.lock').open('a+') as file:
        fcntl.flock(file, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(file, fcntl.LOCK_UN)


def event(run, agent, kind, details=''):
    with (run / 'history.log').open('a', encoding='utf-8') as f:
        f.write(f'{now()} | {agent.upper():6} | {kind:9} | {details}\n')
        f.flush()
        os.fsync(f.fileno())


def last_event(run, agent):
    """Latest history.log line of a slot, for the peer's wait report."""
    path = run / 'history.log'
    if not path.exists():
        return None
    label = f'| {agent.upper():6} |'
    for line in reversed(path.read_text(encoding='utf-8').splitlines()):
        if label not in line:
            continue
        at, _, kind, details = (part.strip() for part in line.split(' | ', 3))
        age = time.time() - dt.datetime.fromisoformat(at).timestamp()
        return {'kind': kind, 'details': details, 'at': at, 'age_secs': round(age)}
    return None


def wait_report(run, agent, result):
    return {'result': result, 'peer': peer_of(agent), 'peer_last_event': last_event(run, peer_of(agent))}


def message_time(message):
    # Older messages lack the float 'ts'; their second-resolution timestamp still orders them.
    if message.get('ts') is not None:
        return message['ts']
    try:
        return dt.datetime.fromisoformat(message['timestamp']).timestamp()
    except (KeyError, ValueError):
        return 0


def all_messages(run):
    messages = []
    for agent in AGENTS:
        if (run / agent).exists():
            messages += [read_json(path) for path in sorted((run / agent).glob('*.json'))]
    return sorted(messages, key=lambda m: (message_time(m), m['agent'], m['seq']))


def message_label(message):
    verdict_or_kind = message.get('status') or message.get('kind') or message.get('type', 'review')
    return (f'{message["agent"]} #{message["seq"]} · {verdict_or_kind} · round {message.get("round", "?")} '
            f'· rev {str(message.get("revision", ""))[:12]} · {message.get("timestamp", "")}')


def finding_threads(messages):
    """Follow explicit Q/A references without joining unrelated lines in the same message."""
    entries = []
    references = {}
    for message in messages:
        for line in message.get('body', '').splitlines():
            ids = list(dict.fromkeys(FINDING_ID.findall(line)))
            if not ids:
                continue
            entries.append((message, line, ids))
            fields = line.split(maxsplit=2)
            if len(fields) > 1 and fields[0] in ('Q', 'A') and fields[1] == ids[0]:
                references.setdefault(ids[0], set()).update(ids[1:])

    threads = {}
    for message, line, ids in entries:
        related = set()
        pending = ids.copy()
        while pending:
            finding = pending.pop()
            if finding in related:
                continue
            related.add(finding)
            pending.extend(references.get(finding, ()))
        for finding in related:
            threads.setdefault(finding, []).append((message, line))
    return threads


def render_transcript(run, out):
    """Self-contained HTML: threads per finding id, controls and decisions, all messages, history."""
    meta = read_json(run / 'meta.json')
    state = read_json(run / 'state.json')
    validate_transcript_output(meta['paths'], run.name, out)
    snapshot_error = None
    try:
        actual = snapshot(meta['paths'], run.name)
    except (OSError, SystemExit) as error:
        actual = None
        snapshot_error = str(error)
    refresh_consensus(run, state, actual)

    messages = all_messages(run)
    threads = finding_threads(messages)
    controls = []
    for message in messages:
        for line in message.get('body', '').splitlines():
            if message.get('type') == 'control' and message.get('kind') != 'info' or re.match(r'^(KICK|ACK|IMP|OFF)\b', line.strip()):
                controls.append((message, line))
    roles = state.get('roles') or {}
    parts = ['<!doctype html><html><head><meta charset="utf-8"><title>Transcript ' + html.escape(run.name) + '</title>',
             '<style>' + TRANSCRIPT_STYLE + '</style></head><body>',
             f'<h1>Transcript {html.escape(run.name)}</h1>',
             '<table>' + ''.join(f'<tr><td>{html.escape(k)}</td><td>{html.escape(str(v))}</td></tr>' for k, v in (
                 ('scope', meta.get('approval_scope', DEFAULT_APPROVAL_SCOPE)), ('paths', ' '.join(meta['paths'])),
                 ('revision', state['revision']), ('round', state['round']), ('consensus', state.get('consensus')),
                 ('executor', roles.get('executor', 'not pinned')), ('reporter', roles.get('reporter', 'not pinned')),
                 ('rendered', now()))) + '</table>']
    if snapshot_error:
        parts.append('<p>Snapshot could not be verified: ' + html.escape(snapshot_error) + '</p>')
    summary_path = out.parent / 'SUMMARY.md'
    if summary_path.exists():
        parts.append('<h2>Summary</h2><section><pre>' + html.escape(summary_path.read_text(encoding='utf-8')) + '</pre></section>')
    parts.append('<h2>Threads by finding id</h2>')
    if not threads:
        parts.append('<p>No messages with finding ids yet.</p>')
    for finding in sorted(threads, key=lambda f: (int(re.sub(r'\D', '', f[1:]) or 0), f)):
        parts.append(f'<section id="thread-{html.escape(finding)}"><h3>{html.escape(finding)}</h3>')
        for message, line in threads[finding]:
            parts.append(f'<div class="a{message["agent"][-1]}"><span class="m">{html.escape(message_label(message))}</span>'
                         f'<pre>{html.escape(line)}</pre></div>')
        parts.append('</section>')
    parts.append('<h2>Controls and decisions</h2>')
    parts.append('<section>' + (''.join(f'<span class="m">{html.escape(message_label(m))}</span><pre>{html.escape(line)}</pre>'
                                        for m, line in controls) or '<p>None.</p>') + '</section>')
    parts.append('<h2 id="messages">All messages</h2>')
    if not messages:
        parts.append('<p>No messages.</p>')
    for message in messages:
        parts.append(f'<section class="a{message["agent"][-1]}"><span class="m">{html.escape(message_label(message))}'
                     f'{" · responding to " + str(message["responding_to"]) if message.get("responding_to") else ""}</span>'
                     f'<pre>{html.escape(message.get("body", ""))}</pre></section>')
    history = (run / 'history.log').read_text(encoding='utf-8') if (run / 'history.log').exists() else ''
    parts.append('<h2>history.log</h2><pre>' + html.escape(history) + '</pre></body></html>')
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text('\n'.join(parts), encoding='utf-8')


def require_run(run):
    if not (run / 'meta.json').exists():
        sys.exit(f'Unknown run {run}. Create it with init first.')


def helper_fingerprint():
    return hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


def verify_helper(run, allow_legacy=False):
    """Called under the run lock before mutations; legacy adoption revokes approvals."""
    meta = read_json(run / 'meta.json')
    actual = helper_fingerprint()
    expected = meta.get('helper_sha256')
    if expected == actual:
        return
    if expected is not None:
        sys.exit(f'Helper version mismatch: run expects {expected}, local helper is {actual}. '
                 'Use the pinned helper on both agents, or synchronize helpers and start a new run.')
    if not allow_legacy:
        sys.exit('Legacy run has no helper fingerprint. Use start or join to adopt it first.')
    state = read_json(run / 'state.json')
    meta['helper_sha256'] = actual
    state.update(round=state['round'] + 1, consensus='PENDING', gates=[])
    atomic_json(run / 'meta.json', meta)
    atomic_json(run / 'state.json', state)
    event(run, 'system', 'HELPER', f'Legacy helper fingerprint pinned; old approvals invalidated; round={state["round"]}')


def is_work_folder(relative_parts, run_id):
    """reviews/<run-id> and research/<run-id> hold the run's own files (diffs, transcript, options, notes)."""
    return any(relative_parts[i:i + 2] == (folder, run_id) for folder in WORK_FOLDERS
               for i in range(len(relative_parts) - 1))


def snapshot_includes(root, item, run_id):
    relative = (root.name, *item.relative_to(root).parts)
    return not (any(x in SNAPSHOT_EXCLUDES for x in item.parts) or is_work_folder(relative, run_id))


def same_existing_file(path, candidate):
    try:
        return candidate.is_file() and path.samefile(candidate)
    except FileNotFoundError:
        # Another run may have atomically replaced a temporary state file.
        return False


def validate_transcript_output(paths, run_id, out):
    try:
        destination = out.resolve()
    except (OSError, RuntimeError):
        destination = Path(os.path.abspath(out))
    review_root = ROOT.resolve()
    if destination == review_root or review_root in destination.parents:
        sys.exit(f'Transcript output overlaps helper review state: {out}')
    if out.exists():
        for item in ROOT.rglob('*'):
            if same_existing_file(out, item):
                sys.exit(f'Transcript output aliases helper review state: {out}')

    for raw in paths:
        root = Path(raw)
        unresolved_root = False
        try:
            resolved_root = root.resolve()
        except (OSError, RuntimeError):
            resolved_root = Path(os.path.abspath(root))
            unresolved_root = True
        if destination == resolved_root:
            included = snapshot_includes(root, root, run_id)
        elif (unresolved_root or root.is_dir() or not root.exists()) and resolved_root in destination.parents:
            item = root / destination.relative_to(resolved_root)
            included = snapshot_includes(root, item, run_id)
        else:
            included = False
        if included:
            sys.exit(f'Transcript output overlaps snapshot path: {out}')

        # Unresolvable roots still block lexical overlap; snapshot reports their error.
        if unresolved_root:
            continue

        # An outside hard link still overwrites the reviewed inode.
        if out.exists():
            choices = root.rglob('*') if root.is_dir() else [root]
            for item in choices:
                if snapshot_includes(root, item, run_id) and same_existing_file(out, item):
                    sys.exit(f'Transcript output aliases snapshot file: {out}')


def snapshot(paths, run_id):
    """Hash exact contents of explicit files/directories; under a directory, the run's work folders are skipped."""
    all_files = set()
    for raw in paths:
        p = Path(raw)
        for item in (p, *p.parents):
            if item.is_symlink():
                sys.exit(f'Symlink snapshot path is unsupported: {item}')
        if not p.exists():
            sys.exit(f'Missing snapshot path: {raw}')
        choices = p.rglob('*') if p.is_dir() else [p]
        for item in choices:
            if not snapshot_includes(p, item, run_id):
                continue
            if item.is_symlink():
                sys.exit(f'Symlink snapshot path is unsupported: {item}')
            if item.is_file():
                all_files.add(item)
    if not all_files:
        sys.exit('Snapshot has no files.')
    h = hashlib.sha256()
    for p in sorted(all_files, key=lambda p: str(p)):
        h.update(str(p).encode() + b'\0')
        h.update(hashlib.sha256(p.read_bytes()).digest())
    return h.hexdigest()


def is_review(message):
    """Types compare case-insensitively; a missing type is a legacy review."""
    return (message.get('type') or 'review').lower() == 'review'


def verdict(message):
    """Older helper versions stored the verdict as status or verdict."""
    return (message.get('status') or message.get('verdict') or '').upper()


def touch_heartbeat(run, agent):
    """Refresh the agent's heartbeat unless a watcher owns it; only the watcher can tell the CLI is alive."""
    path = run / f'{agent}.heartbeat.json'
    if (read_json(path) or {}).get('session_pid') is not None:
        return
    atomic_json(path, {'time': time.time(), 'at': now()})


def append_message(run, agent, state, fields, body, responding_to):
    entries = sorted((run / agent).glob('*.json')) if (run / agent).exists() else []
    seq = (int(entries[-1].stem) + 1) if entries else 1
    msg = {'agent': agent, 'seq': seq, 'responding_to': responding_to,
           'revision': state['revision'], 'round': state['round'],
           'approval_scope': read_json(run / 'meta.json').get('approval_scope', DEFAULT_APPROVAL_SCOPE),
           **fields, 'timestamp': now(), 'ts': time.time(), 'body': body}
    atomic_json(run / agent / f'{seq:08d}.json', msg)
    touch_heartbeat(run, agent)
    return msg


def refresh_consensus(run, state, actual):
    """Derive consensus from latest reviews, never from cached readiness."""
    scope = read_json(run / 'meta.json').get('approval_scope', DEFAULT_APPROVAL_SCOPE)
    current = []
    gates = []
    for agent in AGENTS:
        entries = sorted((run / agent).glob('*.json'), reverse=True)
        latest = None
        for path in entries:
            message = read_json(path)
            if is_review(message):
                latest = message
                break
        if (latest and latest['revision'] == state['revision']
                and latest['round'] == state['round']
                and latest.get('approval_scope', DEFAULT_APPROVAL_SCOPE) == scope):
            current.append(verdict(latest))
            gates += latest.get('gates', [])
    consensus = 'PENDING'
    if 'BLOCKED' in current:
        consensus = 'BLOCKED'
    elif current == ['APPROVED', 'APPROVED']:
        consensus = APPROVAL_RESULTS[scope]
    # An announced edit makes the current snapshot provisional.
    if state.get('editor'):
        consensus = 'PENDING'
    if actual != state['revision']:
        consensus = 'STALE_REVISION'
    previous = state.get('consensus')
    approved = consensus in APPROVAL_RESULTS.values()
    state.update(consensus=consensus, approval_scope=scope, gates=list(dict.fromkeys(gates)) if approved else [])
    atomic_json(run / 'state.json', state)
    if consensus != previous:
        event(run, 'system', 'CONSENSUS', consensus)
    return state


def peer_of(agent):
    return AGENTS[1] if agent == AGENTS[0] else AGENTS[0]


def control_lines(body, opcode):
    return [line.strip() for line in body.splitlines()
            if re.match(rf'^{opcode}(?:\s|$)', line.strip())]


def startup_kick(run):
    kicks = {agent: [] for agent in AGENTS}
    for agent in AGENTS:
        agent_dir = run / agent
        entries = sorted(agent_dir.glob('*.json')) if agent_dir.exists() else []
        for path in entries:
            message = read_json(path)
            body = message.get('body', '')
            if message.get('type') == 'control' and message.get('kind') == 'info':
                kicks[agent].extend((message, line) for line in control_lines(body, 'KICK'))
    source = AGENTS[0] if kicks[AGENTS[0]] else AGENTS[1]
    candidates = kicks[source]
    if not candidates:
        return None
    message, line = candidates[-1]
    if message.get('agent') != source or not isinstance(message.get('seq'), int):
        return None
    slot_pattern = '|'.join(re.escape(slot) for slot in AGENTS)
    match = re.fullmatch(rf'KICK\s+run=([A-Za-z0-9][A-Za-z0-9_-]{{0,63}})\s+prob="((?:[^"\\\r\n]|\\.)+)"\s+exe=({slot_pattern})\s+rep=({slot_pattern})', line)
    if not match or match.group(1) != run.name:
        return None
    return {'agent': message.get('agent'), 'seq': message.get('seq'),
            'executor': match.group(3), 'reporter': match.group(4)}


def ack_submission_error(run, agent, body):
    lines = control_lines(body, 'ACK')
    if not lines:
        return None
    peer = peer_of(agent)
    for line in lines:
        match = re.fullmatch(r'ACK\s+(\d+)', line)
        if not match:
            return 'ACK must name one existing peer message sequence.'
        seq = int(match.group(1))
        path = run / peer / f'{seq:08d}.json'
        message = read_json(path)
        if not message or message.get('agent') != peer or message.get('seq') != seq:
            return 'ACK must reference an existing peer message.'
    return None


def startup_go_error(run, agent):
    for peer in AGENTS:
        session = read_json(run / f'{peer}.session.json')
        heartbeat = read_json(run / f'{peer}.heartbeat.json')
        if not session or session.get('active') is not True:
            return 'Startup gate requires both agents to have active start/join sessions.'
        if not heartbeat or not isinstance(heartbeat.get('time'), (int, float)):
            return 'Startup gate requires fresh heartbeats from both agents.'
        age = time.time() - heartbeat['time']
        if age < 0 or age > PEER_STALE_AFTER:
            return 'Startup gate requires fresh heartbeats from both agents.'
    kick = startup_kick(run)
    if not kick:
        return 'Startup gate requires one valid KICK for this run.'
    if agent != kick['reporter']:
        return f'Only the KICK reporter ({kick["reporter"]}) can relay GO.'
    peer = peer_of(kick['agent'])
    peer_dir = run / peer
    entries = sorted(peer_dir.glob('*.json')) if peer_dir.exists() else []
    for path in entries:
        message = read_json(path)
        if message.get('type') == 'control' and message.get('kind') == 'info' and message.get('agent') == peer:
            if any(re.fullmatch(rf'ACK\s+{kick["seq"]}', line) for line in control_lines(message.get('body', ''), 'ACK')):
                return None
    return 'Startup gate requires the KICK peer to ACK its exact sequence.'


def is_go_request(body):
    return any(re.match(r'^USR\s+"(?:[^"\\\r\n]|\\.)*"\s*=>\s*GO\b', line)
               for line in control_lines(body, 'USR'))


def valid_go_body(body):
    lines = control_lines(body, 'USR')
    return len(lines) == 1 and bool(re.fullmatch(r'USR\s+"(?:[^"\\\r\n]|\\.)+"\s*=>\s*GO', lines[0]))


def require_startup_ready(state):
    if not state.get('startup_ready', False):
        sys.exit('Startup gate is closed; complete the KICK, peer ACK, and reporter GO first.')


def require_role(state, agent, role):
    """Roles are pinned at GO; runs started before that have none and stay unrestricted."""
    owner = (state.get('roles') or {}).get(role)
    if owner and owner != agent:
        sys.exit(f'Only the {role} ({owner}) may do this.')


def require_active_session(run, agent):
    session = read_json(run / f'{agent}.session.json')
    if not session or session.get('active') is not True:
        sys.exit(f'Slot {agent} has no active session; run start or join first.')


def pending_wake(run, agent, session_id, auto_wake_secs=AUTO_WAKE_SECS, nudge_secs=NUDGE_SECS):
    """Return an unread-message or first stale-peer wake for the idle agent."""
    idle = read_json(run / f'{agent}.idle.json')
    peer = peer_of(agent)
    entries = sorted((run / peer).glob('*.json')) if (run / peer).exists() else []
    heartbeat = read_json(run / f'{peer}.heartbeat.json')
    outage_path = run / f'{agent}.peer-stale.json'
    outage = read_json(outage_path)
    stale = heartbeat and time.time() - heartbeat['time'] > PEER_STALE_AFTER
    if not stale:
        outage_path.unlink(missing_ok=True)
        outage = None
    elif not outage or outage['heartbeat_time'] != heartbeat['time']:
        outage = {'heartbeat_time': heartbeat['time'], 'notified': False}
        atomic_json(outage_path, outage)
    # Typed text remains in this session's pane even when its idle marker changes.
    last = read_json(run / f'{agent}.wake.json')
    if (last and last.get('session_id') == session_id and not last['delivered'] and last['text_sent']):
        activity = read_json(run / f'{agent}.activity.json') or {'time': 0}
        if idle or time.time() - activity['time'] >= auto_wake_secs:
            return last
    if not idle:
        return auto_wake(run, agent, session_id, entries, auto_wake_secs, nudge_secs)
    progress = idle.get('wake_progress')
    if progress and progress['session_id'] == session_id:
        return progress
    wake = {'idle_token': idle.get('token'), 'text_sent': False}
    if entries and int(entries[-1].stem) > idle['after']:
        return {**wake, 'source': 'message', 'seq': int(entries[-1].stem),
                'text': f'{WAKE_PREFIX} run={run.name} peer={peer} after={idle["after"]}'}
    if stale and not outage['notified']:
        return {**wake, 'source': 'stale', 'heartbeat_time': heartbeat['time'],
                'text': f'{WAKE_PREFIX} run={run.name} peer={peer} unavailable after={idle["after"]}'}
    return None


def awaiting_answer(run, agent, latest):
    """True while the peer's latest message asks (Q/RUN) and the slot has not written since."""
    if not latest or not any(control_lines(latest.get('body', ''), op) for op in ANSWER_OPCODES):
        return False
    own = sorted((run / agent).glob('*.json')) if (run / agent).exists() else []
    return not own or message_time(read_json(own[-1])) < message_time(latest)


def auto_wake(run, agent, session_id, entries, auto_wake_secs, nudge_secs):
    """Wake a slot that ended its turn without idle: unread peer messages, or an unanswered question."""
    cursor = read_json(run / f'{agent}.cursor.json')
    if not cursor or not entries:
        return None
    activity = read_json(run / f'{agent}.activity.json') or {'time': 0}
    if time.time() - activity['time'] < auto_wake_secs:
        return None
    last = read_json(run / f'{agent}.wake.json')
    if last and (last.get('session_id') != session_id or last['source'] not in ('unread', 'unanswered')):
        last = None
    # Finish the text already in this session's pane before considering a newer peer message.
    if last and not last['delivered'] and last['text_sent']:
        return last

    latest = read_json(entries[-1])
    unread = latest['seq'] > cursor['after']
    if not unread and not awaiting_answer(run, agent, latest):
        return None
    source = 'unread' if unread else 'unanswered'
    text = (f'{WAKE_PREFIX} run={run.name} peer={peer_of(agent)} after={cursor["after"]}' if unread else
            f'{WAKE_PREFIX} run={run.name} peer={peer_of(agent)} unanswered seq={latest["seq"]} after={cursor["after"]}')
    wake = {'source': source, 'seq': latest['seq'], 'text': text, 'text_sent': False, 'session_id': session_id}
    if not last or last['seq'] != latest['seq'] or last['source'] != source:
        return wake
    if not last['delivered']:
        return {**wake, 'text_sent': last['text_sent']}
    # Delivered but still pending: repeat after the nudge interval.
    if time.time() - last['time'] >= nudge_secs:
        return wake
    return None


def user_typing(pane, quiet_secs):
    """True while a client typed into the pane's session within quiet_secs (tmux session_activity)."""
    shown = subprocess.run(['tmux', 'display-message', '-p', '-t', pane, '#{session_activity}'],
                           capture_output=True, text=True)
    try:
        return time.time() - int(shown.stdout.strip()) < quiet_secs
    except ValueError:
        # Unknown activity must not suppress wakes forever.
        return False


def wake_pane(pane, text, text_sent=False, guard_secs=None):
    # Enter is a separate key press so the CLI does not read it as part of a paste.
    if not text_sent:
        sent = subprocess.run(['tmux', 'send-keys', '-t', pane, '-l', text])
        if sent.returncode:
            print(f'tmux send-keys to {pane} failed', file=sys.stderr)
            return False, False
    time.sleep(WAKE_ENTER_DELAY)
    # The user may start typing during the delay; keep the text and retry Enter once they stop.
    if guard_secs is not None and user_typing(pane, guard_secs):
        return True, False
    entered = subprocess.run(['tmux', 'send-keys', '-t', pane, 'Enter'])
    if entered.returncode:
        print(f'tmux send-keys to {pane} failed', file=sys.stderr)
        return True, False
    return True, True


def acknowledge_wake(run, agent, session_id, wake, text_sent, delivered):
    with locked(run):
        session = read_json(run / f'{agent}.session.json')
        if not session or not session['active'] or session['id'] != session_id:
            return
        if wake['source'] in ('unread', 'unanswered'):
            atomic_json(run / f'{agent}.wake.json', {**wake, 'session_id': session_id, 'time': time.time(),
                                                      'text_sent': text_sent, 'delivered': delivered})
            return
        # Keep partial delivery independent of idle markers, which ordinary commands replace or remove.
        atomic_json(run / f'{agent}.wake.json', {**wake, 'session_id': session_id, 'time': time.time(),
                                                  'text_sent': text_sent, 'delivered': delivered})
        idle_path = run / f'{agent}.idle.json'
        idle = read_json(idle_path)
        if idle and idle.get('token') == wake['idle_token']:
            if delivered:
                idle_path.unlink()
            elif text_sent:
                idle['wake_progress'] = {**wake, 'session_id': session_id, 'text_sent': True}
                atomic_json(idle_path, idle)
        if delivered and wake['source'] == 'message':
            # The auto wake must not repeat a wake the idle marker already delivered.
            atomic_json(run / f'{agent}.wake.json', {'source': 'unread', 'seq': wake['seq'], 'time': time.time(),
                                                      'session_id': session_id, 'text_sent': True, 'delivered': True})
        if delivered and wake['source'] == 'stale':
            outage_path = run / f'{agent}.peer-stale.json'
            outage = read_json(outage_path)
            if outage and outage['heartbeat_time'] == wake['heartbeat_time']:
                outage['notified'] = True
                atomic_json(outage_path, outage)


def process_identity(pid):
    """Linux process start time prevents PID reuse from extending a heartbeat."""
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return fields[19] if fields[0] not in ('Z', 'X') else None
    except (OSError, IndexError):
        return None


def watch_heartbeat(run, agent, pid, interval, pane=None, wake_mode='guarded', quiet_secs=WAKE_QUIET_SECS,
                    auto_wake_secs=AUTO_WAKE_SECS, nudge_secs=NUDGE_SECS):
    identity = process_identity(pid)
    if identity is None:
        sys.exit('Session PID must identify a live local Linux CLI process.')
    # A newer start replaces the session token and stops an older watcher.
    with locked(run):
        session = read_json(run / f'{agent}.session.json')
        if not session or not session['active']:
            sys.exit('Run start for this agent before starting its heartbeat watcher.')
    while process_identity(pid) == identity:
        with locked(run):
            current = read_json(run / f'{agent}.session.json')
            if current != session:
                return
            atomic_json(run / f'{agent}.heartbeat.json',
                        {'time': time.time(), 'at': now(), 'session_pid': pid,
                         'session_id': session['id'], 'tmux_pane': pane})
            wake = (pending_wake(run, agent, session['id'], auto_wake_secs, nudge_secs)
                    if pane and wake_mode != 'none' else None)
        guard_secs = quiet_secs if wake_mode == 'guarded' else None
        # Defer the wake text and any Enter retry while the user types; the next interval retries it.
        if wake and guard_secs is not None and user_typing(pane, guard_secs):
            wake = None
        # Never hold the run lock while typing into the pane.
        if wake:
            text_sent, delivered = wake_pane(pane, wake['text'], wake['text_sent'], guard_secs)
            acknowledge_wake(run, agent, session['id'], wake, text_sent, delivered)
        time.sleep(interval)


def main():
    ap = argparse.ArgumentParser(description='Dual-agent review communication helper')
    ap.add_argument('--run', required=True, help='Run id; all agents must use the same id')
    sub = ap.add_subparsers(dest='cmd', required=True)
    init = sub.add_parser('init')
    init.add_argument('--approval-scope', choices=APPROVAL_RESULTS, default=DEFAULT_APPROVAL_SCOPE)
    init.add_argument('--paths', nargs='+', required=True, help='Explicit plan + code paths')
    start = sub.add_parser('start')
    start.add_argument('--agent', choices=AGENTS, required=True)
    join = sub.add_parser('join', help='Start in an existing run and print its scope')
    join.add_argument('--agent', choices=AGENTS, required=True)
    pub = sub.add_parser('publish')
    pub.add_argument('--agent', choices=AGENTS, required=True)
    pub.add_argument('--status', choices=['BLOCKED', 'APPROVED'], required=True)
    pub.add_argument('--text-file', required=True)
    pub.add_argument('--expected-revision', required=True, help='Snapshot hash recorded before reviewing')
    pub.add_argument('--expected-round', type=int, required=True, help='Round recorded before reviewing')
    pub.add_argument('--responding-to', type=int, default=0)
    pub.add_argument('--gate', action='append', default=[], help='Open gate of an APPROVED review; repeatable')
    control = sub.add_parser('control')
    control.add_argument('--agent', choices=AGENTS, required=True)
    control.add_argument('--kind', choices=CONTROL_KINDS, required=True)
    control.add_argument('--text-file', required=True)
    control.add_argument('--responding-to', type=int, default=0)
    wait = sub.add_parser('wait')
    wait.add_argument('--agent', choices=AGENTS, required=True)
    wait.add_argument('--after', type=int, default=0, help='Last processed sequence FROM the peer')
    wait.add_argument('--timeout', type=int, default=600)
    wait.add_argument('--interval', type=float, default=2)
    wait.add_argument('--stale-after', type=int, default=PEER_STALE_AFTER)
    wait.add_argument('--ignore-stale', action='store_true',
                      help='Do not exit 3 on a stale peer; wait for a message or the timeout (recovery wait)')
    hb = sub.add_parser('heartbeat')
    hb.add_argument('--agent', choices=AGENTS, required=True)
    hb.add_argument('--session-pid', type=int, help='Watch this local CLI PID (Linux /proc required)')
    hb.add_argument('--interval', type=float, default=HEARTBEAT_INTERVAL)
    hb.add_argument('--tmux-pane', help='Own tmux pane ($TMUX_PANE) to wake when idle; needs --session-pid')
    hb.add_argument('--wake-mode', choices=WAKE_MODES, default='guarded',
                    help='guarded: type the wake only after the user stops typing; pane: type at once; '
                         'none: never type (the agent waits with a background wait instead)')
    hb.add_argument('--quiet-secs', type=float, default=WAKE_QUIET_SECS,
                    help='Seconds without user keystrokes before a guarded wake is typed')
    hb.add_argument('--auto-wake-secs', type=float, default=AUTO_WAKE_SECS,
                    help='Seconds without own helper commands before unread or unanswered peer messages wake the slot')
    hb.add_argument('--nudge-secs', type=float, default=NUDGE_SECS,
                    help='Seconds between repeated wakes while a message stays unread or unanswered')
    idle = sub.add_parser('idle', help='Mark this agent idle until the peer publishes after --after')
    idle.add_argument('--agent', choices=AGENTS, required=True)
    idle.add_argument('--after', type=int, required=True, help='Last processed sequence FROM the peer')
    check = sub.add_parser('check')
    check.add_argument('--agent', choices=AGENTS, required=True)
    begin_edit = sub.add_parser('begin-edit', help='Claim the single editor role until revise')
    begin_edit.add_argument('--agent', choices=AGENTS, required=True)
    begin_edit.add_argument('--text-file', required=True, help='What will be edited and why')
    revise = sub.add_parser('revise')
    revise.add_argument('--agent', choices=AGENTS, required=True)
    render = sub.add_parser('render', help='Write a threaded HTML transcript of the run so far')
    render.add_argument('--agent', choices=AGENTS, required=True)
    render.add_argument('--out', help='Output path; default reviews/<run-id>/TRANSCRIPT.html')
    ev = sub.add_parser('event')
    ev.add_argument('--agent', choices=AGENTS, required=True)
    ev.add_argument('--kind', choices=['REVIEWING', 'PROGRESS', 'FINISHED'], required=True)
    ev.add_argument('--details', default='')
    args = ap.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]{0,63}', args.run):
        sys.exit('Invalid run id: use letters, numbers, hyphens, underscores only.')
    run = ROOT / args.run

    if args.cmd == 'init':
        with locked(run):
            if (run / 'meta.json').exists():
                existing = read_json(run / 'meta.json')
                if (existing['paths'] != args.paths
                        or existing.get('approval_scope', DEFAULT_APPROVAL_SCOPE) != args.approval_scope):
                    sys.exit('Existing run has different paths or approval scope; use a new run id.')
                verify_helper(run)
            else:
                revision = snapshot(args.paths, args.run)
                atomic_json(run / 'meta.json', {'paths': args.paths, 'approval_scope': args.approval_scope, 'helper_sha256': helper_fingerprint(), 'created': now()})
                atomic_json(run / 'state.json', {'revision': revision, 'round': 1, 'consensus': 'PENDING', 'approval_scope': args.approval_scope, 'startup_ready': False})
                event(run, 'system', 'STARTED', f'run={args.run}')
        print(json.dumps(read_json(run / 'state.json')))
        return

    require_run(run)
    with locked(run):
        verify_helper(run, allow_legacy=args.cmd in ('start', 'join'))
        # Rendering observes the run; any other own command means the agent is active again.
        # A stale marker must not wake an active agent later.
        if args.cmd not in ('heartbeat', 'idle', 'start', 'join', 'render'):
            (run / f'{args.agent}.idle.json').unlink(missing_ok=True)
            atomic_json(run / f'{args.agent}.activity.json', {'time': time.time(), 'at': now(), 'cmd': args.cmd})
    if args.cmd == 'event':
        with locked(run):
            if args.kind == 'REVIEWING':
                require_startup_ready(read_json(run / 'state.json'))
                require_active_session(run, args.agent)
            if args.kind == 'FINISHED':
                session = read_json(run / f'{args.agent}.session.json')
                if session:
                    session['active'] = False
                    atomic_json(run / f'{args.agent}.session.json', session)
                # A finished slot must not keep the peer locked out of publish and revise.
                state = read_json(run / 'state.json')
                if state.get('editor') == args.agent:
                    state.pop('editor', None)
                    state.pop('editing_since', None)
                    atomic_json(run / 'state.json', state)
            event(run, args.agent, args.kind, args.details.replace('\n', ' ')[:160])
        return

    if args.cmd == 'idle':
        with locked(run):
            atomic_json(run / f'{args.agent}.idle.json',
                        {'after': args.after, 'token': uuid.uuid4().hex, 'at': now()})
            atomic_json(run / f'{args.agent}.cursor.json', {'after': args.after, 'at': now()})
            event(run, args.agent, 'IDLE', f'after={args.after}')
        return

    if args.cmd == 'heartbeat':
        if args.tmux_pane and args.session_pid is None:
            sys.exit('--tmux-pane requires --session-pid.')
        if args.session_pid is not None and args.wake_mode != 'none' and not args.tmux_pane:
            sys.exit(f'--wake-mode {args.wake_mode} requires --tmux-pane; use --wake-mode none for a slot that waits in the background.')
        if not 0 <= args.quiet_secs < float('inf'):
            sys.exit('--quiet-secs must be a finite number >= 0.')
        if args.tmux_pane and not shutil.which('tmux'):
            sys.exit('--tmux-pane requires tmux on PATH.')
        # Fail at launch, not as a silent missed wake later.
        if args.tmux_pane and subprocess.run(['tmux', 'display-message', '-p', '-t', args.tmux_pane, '#{pane_id}'],
                                             capture_output=True).returncode:
            sys.exit(f'tmux pane {args.tmux_pane} is not reachable from this shell.')
        if args.session_pid is not None:
            if not 0 < args.interval <= HEARTBEAT_INTERVAL:
                sys.exit(f'Heartbeat interval must be > 0 and <= {HEARTBEAT_INTERVAL} seconds.')
            watch_heartbeat(run, args.agent, args.session_pid, args.interval, args.tmux_pane,
                            args.wake_mode, args.quiet_secs, args.auto_wake_secs, args.nudge_secs)
            return
        with locked(run):
            atomic_json(run / f'{args.agent}.heartbeat.json', {'time': time.time(), 'at': now()})
        return

    if args.cmd in ('start', 'join'):
        with locked(run):
            session_path = run / f'{args.agent}.session.json'
            heartbeat = read_json(run / f'{args.agent}.heartbeat.json')
            if (read_json(session_path) or {}).get('active') is True and heartbeat:
                heartbeat_time = heartbeat.get('time')
                if (isinstance(heartbeat_time, (int, float))
                        and time.time() - heartbeat_time <= PEER_STALE_AFTER):
                    sys.exit(f'Agent slot {args.agent} is already active.')
            (run / f'{args.agent}.idle.json').unlink(missing_ok=True)
            atomic_json(run / f'{args.agent}.session.json', {'id': uuid.uuid4().hex, 'active': True})
            atomic_json(run / f'{args.agent}.heartbeat.json', {'time': time.time(), 'at': now()})
            event(run, args.agent, 'STARTED', f'run={args.run}' + (' joined' if args.cmd == 'join' else ''))
            if args.cmd == 'join':
                meta = read_json(run / 'meta.json')
                print(json.dumps({'paths': meta['paths'], 'helper_sha256': meta['helper_sha256'],
                                  'approval_scope': meta.get('approval_scope', DEFAULT_APPROVAL_SCOPE),
                                  **read_json(run / 'state.json')}))
        return

    if args.cmd == 'begin-edit':
        body = Path(args.text_file).read_text(encoding='utf-8')
        with locked(run):
            state = read_json(run / 'state.json')
            require_startup_ready(state)
            require_active_session(run, args.agent)
            require_role(state, args.agent, 'executor')
            editor = state.get('editor')
            if editor and editor != args.agent:
                sys.exit(f'{editor} is editing since {state["editing_since"]}; wait for its revise.')
            state.update(editor=args.agent, editing_since=now(), consensus='PENDING', gates=[])
            atomic_json(run / 'state.json', state)
            msg = append_message(run, args.agent, state, {'type': 'control', 'kind': 'edit'}, body, 0)
            event(run, args.agent, 'EDITING', f'seq={msg["seq"]}')
        print(json.dumps(state))
        return

    if args.cmd == 'revise':
        with locked(run):
            state = read_json(run / 'state.json')
            require_active_session(run, args.agent)
            require_role(state, args.agent, 'executor')
            editor = state.get('editor')
            if editor and editor != args.agent:
                sys.exit(f'{editor} is editing; only the editor runs revise.')
            state.pop('editor', None)
            state.pop('editing_since', None)
            new = snapshot(read_json(run / 'meta.json')['paths'], args.run)
            if new != state['revision']:
                state.update(revision=new, round=state['round']+1, consensus='PENDING', gates=[])
                event(run, args.agent, 'REVISION', f'round={state["round"]} hash={new[:12]}')
            # Without changes, revise only releases the editor role.
            atomic_json(run / 'state.json', state)
            print(json.dumps(state))
        return

    if args.cmd in ('publish', 'control'):
        body = Path(args.text_file).read_text(encoding='utf-8')
        with locked(run):
            state = read_json(run / 'state.json')
            meta = read_json(run / 'meta.json')
            if args.cmd == 'publish':
                require_startup_ready(state)
            # A finished slot may still answer with control info (the logoff A); nothing else.
            if args.cmd == 'publish' or args.kind != 'info':
                require_active_session(run, args.agent)
            if args.cmd == 'control' and args.kind in ROLE_KINDS and state.get('startup_ready', False):
                require_role(state, args.agent, ROLE_KINDS[args.kind])
            startup_go = args.cmd == 'control' and args.kind == 'decision' and is_go_request(body)
            roles = None
            if startup_go:
                if not valid_go_body(body):
                    sys.exit('GO must be relayed as USR "<verbatim user text>" => GO.')
                if not state.get('startup_ready', False):
                    error = startup_go_error(run, args.agent)
                    if error:
                        sys.exit(error)
                    kick = startup_kick(run)
                    roles = {'executor': kick['executor'], 'reporter': kick['reporter']}
            if args.cmd == 'control' and args.kind == 'info':
                error = ack_submission_error(run, args.agent, body)
                if error:
                    sys.exit(error)
            if args.responding_to and not read_json(run / peer_of(args.agent) / f'{args.responding_to:08d}.json'):
                sys.exit('--responding-to must name an existing peer message sequence (crash recovery reads it).')
            if args.cmd == 'publish' and state.get('editor'):
                sys.exit(f'{state["editor"]} is editing; wait for its revise. Use control for other messages.')
            if args.cmd == 'publish' and (args.expected_revision != state['revision'] or args.expected_round != state['round']):
                sys.exit('Review revision or round changed. Re-review before publishing.')
            if args.cmd == 'publish' and args.gate and args.status != 'APPROVED':
                sys.exit('--gate is only valid with APPROVED.')
            actual = snapshot(meta['paths'], args.run) if args.cmd == 'publish' else state['revision']
            if actual != state['revision']:
                sys.exit('Reviewed code/plan changed. Run revise and re-review before publishing.')
            if args.cmd == 'control' and args.kind in ('scope', 'stop'):
                state.update(round=state['round'] + 1, consensus='PENDING', gates=[])
                atomic_json(run / 'state.json', state)
            if args.cmd == 'publish':
                fields = {'type': 'review', 'status': args.status, 'gates': args.gate}
            else:
                fields = {'type': 'control', 'kind': args.kind}
            msg = append_message(run, args.agent, state, fields, body, args.responding_to)
            if startup_go:
                state['startup_ready'] = True
                if roles:
                    state['roles'] = roles
                    event(run, 'system', 'ROLES', f'executor={roles["executor"]} reporter={roles["reporter"]}')
                atomic_json(run / 'state.json', state)
            event(run, args.agent, 'WROTE', f'seq={msg["seq"]} type={msg["type"]} verdict={msg.get("status", msg.get("kind"))} round={state["round"]} rev={actual[:12]}')
            if args.cmd == 'publish':
                refresh_consensus(run, state, actual)
        print(json.dumps(msg))
        return

    if args.cmd == 'render':
        out = Path(args.out) if args.out else Path('reviews') / args.run / 'TRANSCRIPT.html'
        with locked(run):
            render_transcript(run, out)
            event(run, args.agent, 'RENDERED', str(out))
        print(json.dumps({'path': str(out)}))
        return

    if args.cmd == 'check':
        with locked(run):
            state = read_json(run / 'state.json')
            actual = snapshot(read_json(run / 'meta.json')['paths'], args.run)
            refresh_consensus(run, state, actual)
            atomic_json(run / 'status.json', state)
            print(json.dumps(state))
        return

    if args.cmd == 'wait':
        peer = peer_of(args.agent)
        begin = time.monotonic()
        with locked(run):
            # A drain (timeout 0) runs at every turn start; logging each one buries the user's tail.
            if args.timeout > 0:
                event(run, args.agent, 'WAITING', f'for={peer} after={args.after}')
            atomic_json(run / f'{args.agent}.cursor.json', {'after': args.after, 'at': now()})
        while True:
            with locked(run):
                peer_dir = run / peer
                entries = sorted(peer_dir.glob('*.json')) if peer_dir.exists() else []
                newer = [p for p in entries if int(p.stem) > args.after]
                if newer:
                    message = read_json(newer[0])
                    event(run, args.agent, 'RESUMED', f'{peer} seq={message["seq"]} type={message.get("type", "review")}')
                    print(json.dumps(message))
                    return
                heartbeat = read_json(run / f'{peer}.heartbeat.json')
                if not args.ignore_stale and heartbeat and time.time()-heartbeat['time'] > args.stale_after:
                    print(json.dumps(wait_report(run, args.agent, 'PEER_STALE')), file=sys.stderr)
                    sys.exit(3)
                touch_heartbeat(run, args.agent)
            if time.monotonic()-begin >= args.timeout:
                print(json.dumps(wait_report(run, args.agent, 'WAIT_TIMEOUT')), file=sys.stderr)
                sys.exit(2)
            time.sleep(max(0.5, args.interval))


if __name__ == '__main__':
    main()
