# Goal: Codex ↔ Claude Autonomous Collaborative Review

## Objective

Review the implementation plan and relevant code collaboratively between Codex and Claude Code until both independently agree that the plan is ready for execution.

Both agents run in separate CLI sessions, communicate through shared files, and continuously exchange reviews until consensus is achieved.

**Do not execute the implementation plan. Only review, improve, and approve it.**

## 1. Agent Identity

- Identify whether you are Codex or Claude.
- The other agent is your independent reviewer.
- Both agents operate in separate CLI sessions.
- Coordinate autonomously through shared files without requiring an external orchestrator.
- Each agent manages its own review, waiting, and resuming behavior.
- Do not impersonate the other agent or generate approval on its behalf.

## 2. Shared Files

Use `.reviews/` as the shared communication directory.

```text
.reviews/
├── codex.md
├── claude.md
├── status.md
├── history.log
└── locks/
```

File ownership:

- `codex.md` — Written exclusively by Codex.
- `claude.md` — Written exclusively by Claude.
- `status.md` — Written exclusively by Codex.
- `history.log` — Append-only activity timeline, written by both agents using a lock.
- `locks/` — Lock files for synchronization.

Both agents may read every file.

Create missing directories and files as needed, respecting file ownership.

Never overwrite another agent's review.

## 3. Race Condition Prevention

Follow strict file ownership and synchronization rules.

### Review Files

- Each agent writes only its own review file.
- Write to a temporary file first, then atomically rename it to the final filename.
- Never expose partially written reviews.
- Include a unique message sequence number in each review.
- Process each sequence number only once.
- Never wait for a response that has already arrived.

### Shared Status

Only Codex may modify `status.md`.

Claude communicates its status through `claude.md`.

Codex updates `status.md` after observing agent state changes.

The shared status should contain:

- Current review revision
- Current review round
- Codex status
- Claude status
- Outstanding blockers
- Consensus state

Codex must never fabricate Claude's approval or infer approval from silence.

### Concurrent Activity

- Both agents may review simultaneously.
- Neither agent needs to wait for the other to finish its initial review.
- After publishing a review, wait for a new message from the other agent.
- Use atomic writes and sequence numbers to prevent lost or duplicated responses.
- Do not overwrite or delete another agent's communication.

## 4. Review Responsibilities

Read `PLAN.md` and inspect the relevant codebase.

Independently evaluate:

- Architecture and technical feasibility
- Implementation correctness
- Edge cases and failure scenarios
- Dependencies and integration risks
- Security, reliability, and performance
- Testing and validation strategy
- Compatibility with existing code
- Missing requirements or unclear assumptions
- Potential simplifications and improvements

Identify concrete blockers with technical evidence and actionable recommendations.

Do not approve based solely on the other agent's conclusions.

Review the actual codebase, not just the other agent's summary.

## 5. Autonomous Coordination Protocol

### Initial Startup

When `/goal` is invoked:

1. Identify your agent identity.
2. Create required directories if missing.
3. Record a `STARTED` event in `history.log`.
4. Read `PLAN.md` and relevant code.
5. Record a `REVIEWING` event.
6. Perform your independent review.
7. Atomically publish your findings to your own review file.
8. Record a `WROTE` event.
9. Check whether the other agent has already published a response.
10. If no new response exists, enter the waiting state.

### Waiting Behavior

After publishing a review:

- Wait for the other agent's review file to change.
- Poll periodically using modification times or message sequence numbers.
- Prefer sequence numbers to timestamps for detecting new messages.
- Do not terminate merely because the other agent has not responded.
- Remain in `WAITING_FOR_OTHER_AGENT`.
- Log `WAITING` once when entering the waiting state.
- Do not log individual polling attempts.
- Never interpret silence as approval.
- Never repeatedly process the same response.

When a new response arrives:

1. Log `RESUMED`.
2. Read the other agent's latest review.
3. Verify its sequence number and revision identifier.
4. Analyze the findings.
5. Challenge incorrect assumptions or unsupported conclusions.
6. Address valid feedback.
7. Publish your next response.
8. Log `WROTE`.
9. Repeat until consensus or a termination condition is reached.

If the other agent's response was already published before entering the waiting state, process it immediately instead of waiting.

### Avoiding Deadlocks

- Both agents must publish their initial review independently.
- Never require the other agent to write first.
- Never wait while holding a file lock.
- Do not require strict alternation if both agents have new information.
- Track the last processed message sequence from the other agent.
- Increment your own sequence number whenever publishing a substantive response.
- Do not publish a new message solely to acknowledge a message with no new information.
- An approval is a valid response that must be processed by the other agent.

## 6. Disagreement Resolution

When agents disagree:

1. Clearly identify the disagreement.
2. Reference relevant code, requirements, or technical evidence.
3. Explain the reasoning.
4. Propose a concrete resolution.
5. Evaluate the other agent's response.
6. Repeat until the disagreement is resolved.

Prioritize correctness over agreement.

Never approve merely to end the discussion.

If changes to the plan are necessary:

- Propose the changes first.
- Coordinate which agent will modify `PLAN.md`.
- Only one agent may edit `PLAN.md` at a time.
- Use a file lock to prevent concurrent modifications.
- Recalculate the revision identifier after modification.
- Invalidate earlier approvals.
- Restart the review on the updated revision.

Do not modify implementation code without explicit user permission.

## 7. Consensus Rules

Each agent must explicitly declare one of:

- `APPROVED` — No remaining blocking issues.
- `BLOCKED` — Blocking issues remain.
- `WAITING_FOR_OTHER_AGENT` — Waiting for a response.

Consensus requires:

1. Codex explicitly declares `APPROVED`.
2. Claude explicitly declares `APPROVED`.
3. Both approvals reference the exact same plan and code revision.
4. Both agents have reviewed the current revision.
5. All blocking issues have been resolved.
6. Neither agent has outstanding objections.

Any modification to the reviewed plan or code invalidates previous approvals.

Never claim consensus based on an outdated approval.

Only Codex may update `status.md` to `READY_TO_EXECUTE`, after verifying both approval files and revision identifiers.

Claude may independently recognize that consensus has been reached but must not modify `status.md`.

## 8. Revision Tracking

Track the current revision using:

- Git commit identifier
- Content hash of `PLAN.md`
- Content hash or Git tree state of relevant implementation files

Every review must include:

```text
Agent:
Session:
Sequence:
Round:
Revision:
Status:
Blockers:
Findings:
Response-To:
```

Rules:

- Approval must be tied to an exact revision.
- Reject approvals referencing outdated revisions.
- Recalculate revision identifiers after plan or code modifications.
- Restart approval checks when the revision changes.
- Never mistake an earlier approval for approval of the current revision.

## 9. Shared Activity History

Maintain an append-only timeline:

`.reviews/history.log`

The log should show what both parallel agents are doing, when they start, when they publish responses, when they wait, and when they resume.

### Events

- `STARTED` — Agent session started.
- `REVIEWING` — Began reviewing plan/code.
- `WROTE` — Published review or response.
- `WAITING` — Waiting for the other agent.
- `RESUMED` — Detected and processed a response.
- `BLOCKED` — Reported blocking issues.
- `APPROVED` — Approved a revision.
- `REVISION` — Plan revision changed.
- `FINISHED` — Agent completed its work.

### Format

Use one concise line per event:

`YYYY-MM-DDTHH:MM:SS±HH:MM | AGENT | SESSION | EVENT | details`

### Example Timeline

```text
13:00:01 | CODEX  | s1 | STARTED   | round=1
13:00:03 | CLAUDE | s2 | STARTED   | round=1
13:00:04 | CODEX  | s1 | REVIEWING | rev=a13f
13:00:05 | CLAUDE | s2 | REVIEWING | rev=a13f
13:00:35 | CODEX  | s1 | WROTE     | seq=1, blockers=2
13:00:36 | CODEX  | s1 | WAITING   | Claude response
13:00:50 | CLAUDE | s2 | WROTE     | seq=1, blockers=1
13:00:51 | CLAUDE | s2 | RESUMED   | Codex seq=1
13:00:52 | CODEX  | s1 | RESUMED   | Claude seq=1
13:01:10 | CODEX  | s1 | WROTE     | seq=2
13:01:11 | CODEX  | s1 | WAITING   | Claude response
13:01:12 | CLAUDE | s2 | RESUMED   | Codex seq=2
13:02:00 | CODEX  | s1 | APPROVED  | rev=a13f
13:02:08 | CLAUDE | s2 | APPROVED  | rev=a13f
13:02:09 | CODEX  | s1 | FINISHED  | READY_TO_EXECUTE
13:02:10 | CLAUDE | s2 | FINISHED  | READY_TO_EXECUTE
```

### Logging Rules

- Both agents append to the same history file.
- Use timestamps including timezone.
- Include agent name and session identifier.
- Keep each event to one short line.
- Log only meaningful state transitions.
- Never log every polling attempt.
- Never overwrite, rewrite, or truncate history.
- Use an OS-level file lock (`flock` or equivalent) around each append.
- Release the lock immediately after writing.
- Never hold a lock while reviewing or waiting.
- If a logging operation fails, retry safely without corrupting existing entries.
- The log must reflect actual events, not planned or assumed actions.
- The history file is for observability, not agent-to-agent communication.

## 10. Shared Status File

Codex maintains `.reviews/status.md`.

Example:

```text
Revision: a13f
Round: 3

Codex: APPROVED
Claude: WAITING_FOR_OTHER_AGENT

Blockers: 0
Consensus: PENDING

Last Updated: 13:02:00
```

Rules:

- Codex is the sole writer.
- Updates must be atomic.
- Claude communicates its state through `claude.md`.
- Status must reflect the latest observed review files.
- Never report consensus without both valid approvals.
- If Codex is unavailable, Claude continues reviewing and communicating through its own file but does not modify shared status.

## 11. Execution Rules

- Do not execute the implementation plan.
- Reviewing and improving `PLAN.md` is allowed.
- Do not modify implementation code without explicit permission.
- Do not start implementation automatically after approval.
- Only determine whether the plan is ready to execute.
- Do not run destructive commands.
- Do not make unrelated code changes.
- Avoid asking for user input unless a critical product decision cannot be resolved from the available context.

## 12. Termination Conditions

### SUCCESS

Both agents explicitly approve the same revision.

Codex updates:

`Consensus: READY_TO_EXECUTE`

Both agents log `FINISHED`.

Final output:

`READY_TO_EXECUTE`

### UNRESOLVED

A maximum of 10 substantive review rounds is reached without consensus.

Codex updates:

`Consensus: BLOCKED`

Each agent reports unresolved blockers and recommended next actions.

Final output:

`BLOCKED`

### WAITING

The other agent has not responded and the current CLI execution limits prevent further polling.

- Preserve all review files.
- Preserve sequence numbers and revision identifiers.
- Log the waiting state.
- Do not fabricate approval or declare failure merely because the other agent is slow.
- Make the process resumable from existing files.

Final output:

`WAITING_FOR_OTHER_AGENT`

## 13. Final Objective

Produce an implementation-ready plan that:

- Matches the existing codebase.
- Addresses architectural and technical risks.
- Covers important edge cases.
- Contains a clear implementation strategy.
- Includes appropriate testing and validation steps.
- Has no unresolved blocking issues.
- Has been independently approved by Codex and Claude.

**Operate autonomously. Review independently, communicate through files, track all meaningful activity in a shared timeline, prevent race conditions, wait for responses, resolve disagreements, and continue until genuine consensus is achieved. Never execute the implementation plan itself.**
