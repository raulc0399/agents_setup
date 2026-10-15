# Goal: Codex ↔ Claude Autonomous Collaborative Review

## Objective
Independently review `PLAN.md` and the relevant codebase with the other agent until both explicitly approve the **same unchanged revision** as READY_TO_EXECUTE. Do not implement the plan.

Run this goal simultaneously in **two distinct interactive CLI sessions**, one Codex and one Claude. A goal/skill prompt cannot keep a terminated CLI process alive. Use the included `review-sync.py` helper for inter-process coordination; do not implement file polling by repeatedly reasoning about timestamps.

## Mandatory reliability safeguards (all five required)

### A. Run isolation
- Every new collaboration MUST use a unique run ID and its own directory: `.reviews/<run-id>/`.
- Both CLI sessions MUST use the **same** run ID for this collaboration, and MUST NOT reuse an old run for a new task.
- All messages, heartbeats, revisions, status, and timeline events stay within that run directory. Ignore approvals or logs from other runs.
- Generate or agree on the run ID before either agent starts reviewing. If a run already exists, resume it only when its scope matches; otherwise create a new run.

### B. Heartbeat and timeout
- Each agent MUST issue heartbeat updates while actively reviewing and while waiting. The helper's `wait` command maintains heartbeat while it is polling; for long review operations call `heartbeat` periodically.
- Use a heartbeat freshness threshold (default 120 seconds) and a bounded wait call (default 600 seconds), not an infinite busy loop.
- If the peer's heartbeat becomes stale, signal `WAITING_FOR_OTHER_AGENT` with the last known peer state. A stale heartbeat means **possibly disconnected**, not `BLOCKED` or `APPROVED`.
- On timeout, preserve progress and retry waiting if CLI limits allow. Never invent a peer response.

### C. Reliable append-only messaging
- A single replaceable `codex.md` or `claude.md` is NOT sufficient as a transport: rapid updates might overwrite unread reviews.
- Instead publish each review/response as a new immutable sequenced message: `.reviews/<run-id>/codex/00000001.json`, `.reviews/<run-id>/claude/00000001.json`, etc.
- Publish atomically under a file lock. Each agent remembers the highest **peer** sequence number it has processed, and consumes the next unread message in sequence (not merely the latest message).
- Keep `history.log` separately as the concise event timeline; it does not replace messages.

### D. Revision stability / frozen approval snapshot
- Define the review scope explicitly (`PLAN.md` plus relevant code/tests) and hash its **exact file contents and paths**. Exclude `.reviews/` and generated artifacts.
- Every message/approval MUST carry the revision hash and review round. Before an approval is recorded and before readiness is declared, recheck the on-disk snapshot hash.
- No plan/code edits during final approval. Any permitted change requires `revise`, creating a new revision and invalidating **both** previous approvals.
- `READY_TO_EXECUTE` requires two independently issued approvals for the **same current, unchanged hash** and round, with zero blockers. Git commit alone is not sufficient if working-tree files can change.

### E. Deadlock and failure recovery
- Both agents publish initial reviews without waiting for the other, preventing the initial two-way wait deadlock.
- While exchanging messages, never hold a file lock during thinking, reviewing, polling, or waiting. Process already-arrived unread messages before blocking.
- When an agent or CLI crashes, restart its `/goal` session with the **same run ID**, read preserved messages, find the last processed peer sequence, and resume; do not erase or reset the log.
- If Codex crashes, Claude continues publishing its own messages. Codex-owned `status.json` may be stale and is only a convenience view: consensus must be computed from immutable current-revision approvals. Codex refreshes status on restart.
- If both agents are `WAITING`, inspect sequence counters and pending messages; if neither has pending work, use bounded waits and heartbeat checks to identify an unavailable session or unresolved human decision. Do not fabricate activity or enter endless acknowledgment ping-pong.
- If the CLI runtime expires, persist state and return `WAITING_FOR_OTHER_AGENT`. The skill cannot revive a terminated process; a human or process supervisor must restart it.

## 1. Identity and scope
- Identify yourself as `codex` or `claude`; never write or approve on behalf of the other.
- Use the same run ID in both sessions (provided by the user or explicitly agreed before startup). Default for a fresh run: `review-001`; do not reuse an old run for a different task.
- Use the **same explicit snapshot paths** in both sessions, including `PLAN.md` and the relevant code directories/files. Do not include `.reviews` or other generated artifacts.
- Review architecture, requirements, integration, security, reliability, performance, edge cases, tests, and deployment/rollback risks. Ground findings in files and specific evidence.

## 2. Files and ownership
The helper manages `.reviews/<run-id>/`:
- `meta.json`, `state.json`: fixed scope and revision/round state (helper only).
- `codex/00000001.json`, `claude/00000001.json`, ...: **append-only agent messages** (helper only).
- `codex.heartbeat.json`, `claude.heartbeat.json`: liveness signals (helper only).
- `history.log`: interleaved chronological event log (helper only).
- `status.json`: convenience summary, refreshed by Codex (helper only).
- `.lock`: shared synchronization lock (helper only).

Do not edit these files directly. Both agents can read everything. Each agent publishes messages only under its own identity. The helper uses OS file locks, atomic replacement for state files, and immutable message filenames to prevent races and missed updates. Treat `status.json` as **derived**, not as the source of approval; valid approvals are the current revision's immutable messages.

## 3. Initialization
Before beginning (from repository root), use the **same** scope and run ID in both sessions:

```bash
python3 review-sync.py --run review-001 init --paths PLAN.md src tests
python3 review-sync.py --run review-001 start --agent codex    # Codex only
python3 review-sync.py --run review-001 start --agent claude   # Claude only
```

If the project has different directories, replace `src tests` with actual paths. Initialization is idempotent for the same paths and rejects conflicting scope definitions. Never invent missing paths.

## 4. Initial independent review
- Begin both reviews independently. Do not wait for the other agent to post first.
- Log `REVIEWING` via `python3 review-sync.py --run review-001 event --agent <your-agent> --kind REVIEWING` when you start inspecting the plan/code. Read the plan and relevant code; run **safe, non-destructive tests/static checks** where useful, but **do not execute the implementation plan**.
- Prepare a compact Markdown review in your own temporary file (outside `.reviews/<run-id>/` or otherwise excluded from the snapshot). Include: verdict (`APPROVED` or `BLOCKED`), numbered blockers, supporting file/line evidence, suggested fixes, tests considered, and questions to the peer.
- Publish through the helper, e.g.:

```bash
python3 review-sync.py --run review-001 publish --agent codex --status BLOCKED --text-file /tmp/codex-review.md --responding-to 0
python3 review-sync.py --run review-001 publish --agent claude --status BLOCKED --text-file /tmp/claude-review.md --responding-to 0
```

Run only the command for your identity, and choose the verdict you genuinely reached. Publishing stamps the message with the current revision and sequence number. If the snapshot changed, publishing is rejected. After publishing, use `event --agent <your-agent> --kind APPROVED` or `--kind BLOCKED` to log the verdict. Log `FINISHED` once when your session actually ends.

## 5. Waiting, resuming, and reliable message delivery
After publishing, receive the peer's **next unread message**, using the last peer sequence number you processed (`--after` starts at 0):

```bash
python3 review-sync.py --run review-001 wait --agent codex --after 0 --timeout 600
python3 review-sync.py --run review-001 wait --agent claude --after 0 --timeout 600
```

- `wait` immediately returns an already-existing unread message; otherwise it polls briefly and maintains your heartbeat while waiting.
- `wait` logs `WAITING` and `RESUMED` exactly when a wait call enters or receives a message (not every poll).
- Read the JSON response and **remember the peer sequence number**. Next time, pass that value to `--after`, so unread messages are handled one at a time and none are silently skipped.
- When the CLI allows it, call `wait` again after processing each message. Never treat silence as agreement.
- If `wait` exits with code `2` (timeout), check process/session health and resume waiting; if code `3` (peer stale), report `WAITING_FOR_OTHER_AGENT` and attempt recovery rather than approving. If the CLI session ends, the user must restart it; a prompt cannot restore a stopped process.
- If your review takes an extended time, issue `heartbeat` periodically; a heartbeat is liveness only, not a reply.

## 6. Discussion and plan edits
- Challenge the other agent's claims with evidence. Do not approve just to terminate the loop.
- Each substantive response gets its own new message, published with `--responding-to <last-peer-seq>`.
- Avoid acknowledgment ping-pong: only publish if you have new findings, an actionable question, a changed assessment, or a genuine approval.
- No implementation code edits without explicit user permission. Plan improvements are allowed, but coordinate **one plan editor at a time** in the discussion, and do not edit while the other agent is approving the current snapshot.
- After any permitted plan/code edits, run:

```bash
python3 review-sync.py --run review-001 revise --agent codex
```

  (The editing agent may use its own identity.) This calculates the new revision, increments the round, and resets consensus. Any prior approvals become invalid automatically. Both agents must re-review the new revision. If `publish` detects changed contents before `revise`, it fails rather than stamping stale findings as current.

## 7. Consensus
- Each agent must publish an explicit `APPROVED` message for the **exact same revision and round**, with zero remaining blocking findings.
- Agreement with the other agent's text is not a substitute for checking the code/plan yourself.
- Approvals are never inferred from a heartbeat, silence, `status.json`, or older messages.
- The helper only sets `READY_TO_EXECUTE` when both agents' most recent messages are `APPROVED` on the same revision and round; the reviewed content must remain unchanged. Codex may refresh the status view:

```bash
python3 review-sync.py --run review-001 check --agent codex
```

- Before stating success, recheck that the snapshot has not changed. **Do not execute the plan automatically.**

## 8. History / observability
`history.log` is a compact chronological view of both CLI threads. The helper records `STARTED`, `REVIEWING`, `WROTE`, `WAITING`, `RESUMED`, `BLOCKED`, `APPROVED`, `REVISION`, and `FINISHED` with timestamps and agent IDs. The review text provides details; the history log records **events only**. No logging of every poll. For observability:

```bash
tail -f .reviews/review-001/history.log
```

Do not fabricate log entries, and do not manually truncate history. Use a fresh run ID for each new collaboration.

## 9. Crash recovery, deadlines, and stop conditions
- A missing/stale heartbeat indicates possible unavailability, not disagreement or approval.
- On restart, reuse the existing run ID and snapshot scope, read prior messages and state, determine the last peer sequence you've processed, then resume. Do not reset or delete message history.
- If Codex is down, Claude can continue reviewing/publishing independently; `status.json` may be stale, but immutable messages remain authoritative. Codex refreshes it on recovery.
- Stop after at most **10 substantive revision/review rounds**. Report remaining blockers, differences, and needed decisions. Avoid unlimited response loops.
- `READY_TO_EXECUTE`: both have current explicit approvals, no outstanding blockers, same stable revision.
- `BLOCKED`: unresolved blockers after the round limit or genuinely unresolvable disagreement.
- `WAITING_FOR_OTHER_AGENT`: peer unavailable or CLI execution budget exhausted. Preserve all files for resumption.
- Never prompt endlessly when only a human product decision can resolve an ambiguity; surface that decision clearly.

## 10. Operating rules
- Do not implement the approved plan, deploy, migrate, or make destructive changes.
- Never impersonate the peer or fabricate its responses.
- Keep reviews short, precise, and evidence-based.
- Keep `.reviews/` out of the reviewed snapshot and preferably add it to `.gitignore`.
- Stay inside the CLI's tool/permission/runtime limits. The helper supplies deterministic synchronization, but **does not launch agents or keep their CLI sessions alive**.

**Final objective: `READY_TO_EXECUTE` only after two independent, current-revision approvals.**
