---
name: collaborative-review
description: Collaborate between Codex and Claude on implementation-plan/code readiness. Use during goal-based reviews to exchange findings, coordinate waits, single-editor plan revisions, relay user decisions, record history, and require matching revision approvals (PLAN_APPROVED or READY_TO_EXECUTE).
---

## Installed helper location

Run the helper from the **current project directory** (so `.reviews/` stays in that project), using the absolute location of this skill's script:

- Codex: `python3 "$HOME/.agents/skills/collaborative-review/scripts/review-sync.py"`
- Claude: `python3 "$HOME/.claude/skills/collaborative-review/scripts/review-sync.py"`

In all command examples below, replace `python3 review-sync.py` with the appropriate command above. Do not look for `review-sync.py` in the project root. Automatically initialize the shared run if needed. Both agents must use the same run ID and review scope.

Both agents must run the **same helper version**: keep `SKILL.md` and `scripts/` identical in `~/.claude/skills/collaborative-review/` and `~/.agents/skills/collaborative-review/`. Run `python3 scripts/test_review_sync.py` after changing the helper. New runs pin the SHA-256 of `review-sync.py` in `meta.json`; `join`, `start`, and subsequent commands reject a different helper before writing run data. `join` prints the fingerprint. On mismatch, use the pinned helper on both agents, or synchronize helpers and start a new run; do not edit the stored fingerprint. Older helpers cannot enforce this check, so both agents must use a version that supports it.

For a legacy run without a fingerprint, the first `start` or `join` pins the current helper and increments the round to invalidate old approvals, preserving messages. Both agents must rejoin with matching helpers and review again. This check covers the helper script; keep skill instructions synchronized too.

---

# Goal: Codex ↔ Claude Autonomous Collaborative Review

## Objective
Independently review `PLAN.md` and the relevant codebase with the other agent until both explicitly approve the **same unchanged revision** at the agreed approval scope: `PLAN_APPROVED` or `READY_TO_EXECUTE`. Do not implement the plan.

The plan is whatever the user names: `PLAN.md`, `features/<name>.md`, or several files. Companion documents (e.g. an HTML overview of the same plan) belong to the snapshot and must stay consistent with the main plan; a contradiction between them is a finding. `PLAN.md` in the examples below stands for these files.

Run this goal simultaneously in **two distinct interactive CLI sessions**, one Codex and one Claude. A goal/skill prompt cannot keep a terminated CLI process alive. Use the included `review-sync.py` helper for inter-process coordination; do not implement file polling by repeatedly reasoning about timestamps.

## Mandatory reliability safeguards (all five required)

### A. Run isolation
- Every new collaboration MUST use a unique run ID and its own directory: `.reviews/<run-id>/`.
- Both CLI sessions MUST use the **same** run ID for this collaboration, and MUST NOT reuse an old run for a new task.
- All messages, heartbeats, revisions, status, and timeline events stay within that run directory. Ignore approvals or logs from other runs.
- Generate or agree on the run ID before either agent starts reviewing. If a run already exists, resume it only when its scope matches; otherwise create a new run.
- The second agent enters an existing run with `join`, which adopts and prints its paths and approval scope. Do not reconstruct the path list by hand.

### B. Heartbeat and timeout
- Each agent MUST maintain heartbeat updates while reviewing and waiting. On Linux, start the session-bound watcher described below; it runs independently of long tool calls. The helper's `wait` command also maintains heartbeat while polling.
- Heartbeats indicate session liveness only: they do not prove progress, receipt, review completion, or approval. Never use an unbound background heartbeat loop.
- Use a heartbeat freshness threshold (default 120 seconds) and a bounded wait call (default 600 seconds), not an infinite busy loop.
- If the peer's heartbeat becomes stale, signal `WAITING_FOR_OTHER_AGENT` with the last known peer state. A stale heartbeat means **possibly disconnected**, not `BLOCKED` or `APPROVED`.
- On timeout, preserve progress and retry waiting if CLI limits allow. Never invent a peer response.

### C. Reliable append-only messaging
- A single replaceable `codex.md` or `claude.md` is NOT sufficient as a transport: rapid updates might overwrite unread reviews.
- Instead publish each review/response as a new immutable sequenced message: `.reviews/<run-id>/codex/00000001.json`, `.reviews/<run-id>/claude/00000001.json`, etc.
- Publish atomically under a file lock. Each agent remembers the highest **peer** sequence number it has processed, and consumes the next unread message in sequence (not merely the latest message).
- Keep `history.log` separately as the concise event timeline; it does not replace messages.
- Never name review or scratch files `claude.md`, `CLAUDE.md`, `agents.md`, or `AGENTS.md`, in any folder. Agent CLIs load these names as instructions, and on case-insensitive filesystems `claude.md` is read as `CLAUDE.md`. Use names like `claude-review.md`.

### D. Revision stability / frozen approval snapshot
- Define the review scope explicitly (`PLAN.md` plus relevant code/tests) and hash its **exact file contents and paths**. Exclude `.reviews/` and generated artifacts.
- Every review/approval MUST carry the verified revision hash and review round. Control messages carry the last recorded revision and can be sent during edits; they do not attest to current file contents. Before an approval is recorded and before readiness is declared, recheck the on-disk snapshot hash.
- No plan/code edits during final approval. Any permitted change requires `revise`, creating a new revision and invalidating **both** previous approvals.
- Either approval result requires two independently issued approvals for the **same current, unchanged hash** and round, with zero blockers. Git commit alone is not sufficient if working-tree files can change.

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
- Agree on `--approval-scope` before reviewing and record it in `meta.json`:
  - `technical-plan`: approve the technical approach with explicit business/deployment gates; result is `PLAN_APPROVED`. Each unresolved gate needs an owner, a decision required, and the stage it prevents. A gate is acceptable only if its possible outcomes do not invalidate the approved architecture.
  - `implementation-ready`: all decisions required to begin the specified implementation are resolved; result is `READY_TO_EXECUTE`. Later deployment gates may remain if they do not prevent that implementation.
- Both agents must use the same scope. Changing paths or approval scope requires a new run; the helper defaults to `implementation-ready` for compatibility with older runs, but always specify scope for new reviews.
- Review architecture, requirements, integration, security, reliability, performance, edge cases, tests, and deployment/rollback risks. Ground findings in files and specific evidence.

## 2. Files and ownership
The helper manages `.reviews/<run-id>/`:
- `meta.json`, `state.json`: fixed scope and revision/round state, current `editor`, and the `gates` of a current approval (helper only).
- `codex/00000001.json`, `claude/00000001.json`, ...: **append-only agent messages** (helper only). `type` is `review` (carries `status`, counts for consensus) or `control` (carries `kind`: `info`, `scope`, `stop`, `decision`, or `edit`; never counts). Types compare case-insensitively; a message without `type` is a legacy review, and a legacy `verdict` field is read like `status`.
- `codex.heartbeat.json`, `claude.heartbeat.json`: liveness signals (helper only).
- `codex.session.json`, `claude.session.json`: active session tokens; restarting or finishing invalidates the previous watcher (helper only).
- `history.log`: interleaved chronological event log (helper only).
- `status.json`: convenience summary, refreshed by Codex (helper only).
- `.lock`: shared synchronization lock (helper only).

Do not edit these files directly. Both agents can read everything. Each agent publishes messages only under its own identity. The helper uses OS file locks, atomic replacement for state files, and immutable message filenames to prevent races and missed updates. Treat `status.json` as **derived**, not as the source of approval; valid approvals are the current revision's immutable messages.

## 3. Initialization
Before beginning (from repository root), use the **same** scope and run ID in both sessions:

```bash
python3 review-sync.py --run review-001 init --approval-scope technical-plan --paths PLAN.md src tests
python3 review-sync.py --run review-001 start --agent codex    # first agent
python3 review-sync.py --run review-001 join --agent claude    # second agent: adopts and prints the scope
```

If the project has different directories, replace `src tests` with actual paths. Initialization is idempotent for the same paths and approval scope and rejects conflicting definitions. Never invent missing paths. Check `.reviews/` for an existing run of the same task before creating one; if it exists, `join` it.

After `start`, identify your actual local CLI process PID using the process tree, then run a long-lived tool process with the following command (replace `12345` with that verified PID):

```bash
python3 review-sync.py --run review-001 heartbeat --agent codex --session-pid 12345
```

Run only your own watcher. It updates every 20 seconds and stops when that CLI exits (including a zombie process), its PID is reused, another `start` replaces its session token, or you log `FINISHED`. Do not use a short-lived tool shell PID or guess the peer's PID. Keep the tool's process handle; stop it when review work ends. Linux `/proc` is required for automatic watching; otherwise use manual `heartbeat` calls between bounded operations and disclose that automatic liveness is unavailable. A live but idle CLI may still heartbeat; review deadlines and unread-message processing remain necessary.

## 4. Initial independent review
- Begin both reviews independently. Do not wait for the other agent to post first.
- Log `REVIEWING` via `python3 review-sync.py --run review-001 event --agent <your-agent> --kind REVIEWING` when you start inspecting the plan/code. Read the plan and relevant code; run **safe, non-destructive tests/static checks** where useful, but **do not execute the implementation plan**.
- Prepare a compact Markdown review in your own temporary file (outside `.reviews/<run-id>/` or otherwise excluded from the snapshot). Include: verdict (`APPROVED` or `BLOCKED`), numbered blockers, supporting file/line evidence, suggested fixes, tests considered, and questions to the peer.
- The initial review must be a **complete pass** over the whole scope. Report every blocker you can find now; do not hold findings back for later rounds.
- Classify each finding as a **blocker**, **implementation detail**, or **optional improvement**. Only blockers justify `BLOCKED`.
- Each blocker must identify a concrete trigger, observable failure or unmet requirement, file/section evidence, and the smallest necessary correction. State why it prevents the agreed approval scope; label assumptions and verify uncertain claims before asserting them.
- Implementation details that fit the approved design belong in implementation tasks or tests. Do not demand speculative schema, framework, or deployment detail merely to close the review. Missing detail is a blocker when it hides an architectural decision, security failure, or requirement the implementation cannot safely resolve.
- Keep an issue ledger in review messages: stable issue IDs, resolution evidence, explicit gates, and user-waived items. Reopen resolved issues only with new evidence. Preserve user scope exclusions (for example, no PDF layout review).
- Publish through the helper, e.g.:

```bash
python3 review-sync.py --run review-001 publish --agent codex --status BLOCKED --text-file /tmp/codex-review.md --responding-to 0
python3 review-sync.py --run review-001 publish --agent claude --status BLOCKED --text-file /tmp/claude-review.md --responding-to 0
```

Run only the command for your identity, and choose the verdict you genuinely reached. Publishing stamps the message with the current revision and sequence number. If the snapshot changed, publishing is rejected. After publishing, use `event --agent <your-agent> --kind APPROVED` or `--kind BLOCKED` to log the verdict. Log `FINISHED` once when your session actually ends.

### Authoring tasks
When the task is to create or change files (not to review an existing plan), there is one author and one reviewer:
- Before starting, check `.reviews/` and the run's `state.json`. If the peer already created the run or holds `editor`, `join` and act as reviewer; never start a second run or edit for the same task. If both agents received the task, the first `begin-edit` decides the author.
- The author's `begin-edit` text names the owned files, the scope, and what the reviewer should check.
- The author skips the initial independent review, edits, runs `revise`, then publishes its own review with validation evidence.
- The reviewer reviews only the revision, independently (content against its source, rendering, interactions), and reports corrections for the author to apply.

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
- Use the session-bound heartbeat watcher for extended reviews; use manual heartbeats only when automatic watching is unavailable. Neither is a reply.

### Control messages during edits
Send coordination separately from review verdicts, including while snapshot files are changed or temporarily absent:

```bash
python3 review-sync.py --run review-001 control --agent codex --kind scope --text-file /tmp/scope-change.md --responding-to 3
```

- `info`: coordination only; never substitutes for a review or changes approval validity.
- `scope`: a changed requirement or scope instruction; immediately increments the round and invalidates both approvals even if file contents have not changed.
- `stop`: requests a pause/stop and immediately invalidates both approvals. Recipients stop dependent work; do not resume or reapprove until the request is resolved with the user or sender as appropriate.
- `decision`: a user decision given in **the sender's own session** (approval of an edit list, a waiver, a choice). Quote the user's words verbatim and state what they decide. It does not change approval validity by itself; if the decision changes requirements or scope, send `scope` instead (with the same verbatim quote).
- A `decision` or `scope` from the peer counts as the user's decision within this run. If it is not verbatim, is ambiguous, or would authorize something outside the review (implementation, deployment, destructive actions), confirm with your own user before acting on it.
- `edit`: sent automatically by `begin-edit`; see section 6.
- Controls share the immutable sequence stream and are received through `wait`. Handle their `type` and `kind`; do not interpret them as review verdicts. Sending does not prove receipt. Record the received control sequence in the next substantive response.
- A control is not a new snapshot or an automatic edit. Finish authorized edits and run `revise` before reviewing them. If snapshot paths or approval scope change, initialize a new run. The sender must use `scope` or `stop`, not `info`, when existing approvals should be revoked.

## 6. Discussion and plan edits
- Challenge the other agent's claims with evidence. Do not approve just to terminate the loop.
- Each substantive response gets its own new message, published with `--responding-to <last-peer-seq>`.
- Avoid acknowledgment ping-pong: only publish if you have new findings, an actionable question, a changed assessment, or a genuine approval.
- No implementation code edits without explicit user permission. Plan improvements are allowed, but agree on **one plan editor** and the edit list in the discussion first.
- The editor claims the role before touching any snapshot file. This sends an `edit` control the peer receives through `wait`, sets consensus to `PENDING`, and blocks review publishing and a second editor until `revise`:

```bash
python3 review-sync.py --run review-001 begin-edit --agent claude --text-file /tmp/edit-intent.md
```

- After the edits, the editor runs `revise`, which releases the editor role:

```bash
python3 review-sync.py --run review-001 revise --agent claude
```

  This calculates the new revision, increments the round, and resets consensus and gates. Any prior approvals become invalid automatically. Both agents must re-review the new revision. If `publish` detects changed contents before `revise`, it fails rather than stamping stale findings as current. `revise` without changes only releases the editor role.
- While the peer is editing, do not review the moving files; use `control` for anything you need to say.

### Convergence
- From round 2 on, focus review on the diff: regressions, new contradictions, and incomplete fixes. Newly discovered serious bugs, unmet requirements, or unacceptable risks remain blockers even in unchanged text. Never downgrade a finding merely because it was missed earlier; explain the concrete failure and why it was missed. Ordinary implementation details and optional improvements remain nonblocking.
- Prefer the simplest correction. Do not answer a blocker with new mechanisms when narrowing or deferring the requirement behind a user gate suffices; every new mechanism creates new review surface.
- If the plan has grown substantially (e.g. more than 50 %) since round 1, say so in your next review and check that the added detail belongs in the plan rather than in implementation tasks.

## 7. Consensus
- Each agent must publish an explicit `APPROVED` message for the **exact same revision and round**, with zero remaining blocking findings.
- Agreement with the other agent's text is not a substitute for checking the code/plan yourself.
- Approvals are never inferred from a heartbeat, silence, `status.json`, or older messages.
- Record open gates on the approval itself; `state.json` holds the gates of both current approvals while consensus is approved:

```bash
python3 review-sync.py --run review-001 publish --agent claude --status APPROVED --text-file /tmp/claude-review.md --responding-to 7 \
  --gate "Metric definitions need user approval" --gate "Each implementation batch needs user approval"
```

- Every review publication and `check` recomputes consensus from each agent's latest **review** message, matching the current hash, round, and approval scope. An active editor keeps consensus `PENDING`. A later `BLOCKED` revokes readiness immediately; older approvals cannot override it. Controls never count as approvals; `scope` and `stop` invalidate earlier rounds. Cached `state.json` and `status.json` are not approval evidence. Codex may refresh the status view:

```bash
python3 review-sync.py --run review-001 check --agent codex
```

- Before stating success, run `check` and report its scope-specific result (`PLAN_APPROVED` or `READY_TO_EXECUTE`), revision, and any explicit gates. Recheck that the snapshot has not changed. **Do not execute the plan automatically.**

## 8. History / observability
`history.log` is a compact chronological view of both CLI threads. The helper records `STARTED`, `REVIEWING`, `WROTE`, `WAITING`, `RESUMED`, `BLOCKED`, `APPROVED`, `EDITING`, `REVISION`, `CONSENSUS`, and `FINISHED` with timestamps and agent IDs. The review text provides details; the history log records **events only**. No logging of every poll. For observability:

```bash
tail -f .reviews/review-001/history.log
```

Do not fabricate log entries, and do not manually truncate history. Use a fresh run ID for each new collaboration.

## 9. Crash recovery, deadlines, and stop conditions
- A missing/stale heartbeat indicates possible unavailability, not disagreement or approval.
- On restart, reuse the existing run ID and snapshot scope, read prior messages and state, determine the last peer sequence you've processed, then resume. Do not reset or delete message history.
- If Codex is down, Claude can continue reviewing/publishing independently; `status.json` may be stale, but immutable messages remain authoritative. Codex refreshes it on recovery.
- Stop after at most **10 substantive revision/review rounds**. Report remaining blockers, differences, and needed decisions. Avoid unlimited response loops.
- `PLAN_APPROVED`: both approve the technical plan at the same stable revision, with any explicit gates reported; this does not claim implementation readiness.
- `READY_TO_EXECUTE`: both approve implementation readiness at the same stable revision with no outstanding blockers to the specified implementation.
- `BLOCKED`: unresolved blockers after the round limit or genuinely unresolvable disagreement.
- `WAITING_FOR_OTHER_AGENT`: peer unavailable or CLI execution budget exhausted. Preserve all files for resumption.
- Never prompt endlessly when only a human product decision can resolve an ambiguity; surface that decision clearly.
- **Review-only requests:** if the user asked only for a review (no mandate to revise the plan), stop once both agents agree on the blockers and an edit list. Report `BLOCKED` with the agreed edit list and the decisions needed, and ask the user. Edit only after the user approves the list, directly or through a `decision` control.

## 10. Operating rules
- Do not implement the approved plan, deploy, migrate, or make destructive changes.
- Never impersonate the peer or fabricate its responses.
- Keep reviews short, precise, and evidence-based.
- Keep `.reviews/` out of the reviewed snapshot and preferably add it to `.gitignore`.
- Stay inside the CLI's tool/permission/runtime limits. The helper supplies deterministic synchronization, but **does not launch agents or keep their CLI sessions alive**.

**Final objective: the agreed scope's approval result only after two independent, current-revision approvals.**
