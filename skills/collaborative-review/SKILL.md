---
name: collaborative-review
description: Review plans or code with two independent reviewers and an integration challenger against a pinned revision, recording findings, evidence, coverage, and approval gates. Use for collaborative readiness reviews; review approval does not authorize implementation.
---

# Collaborative review

Use `scripts/review.py` from this skill's installed directory, running from the project directory. Codex installs normally use `~/.agents/skills/collaborative-review/`; Claude installs use `~/.claude/skills/collaborative-review/`. Use an absolute script path; do not assume it exists in the reviewed project. Detailed CLI recipes and schemas are in the repository `README.md`; command `--help` is authoritative for installed flags.

## Scope and authority

Agree on explicit existing input paths and approval scope: `technical-plan`, `implementation-ready`, or `report-ready`. Review does not authorize implementing recommendations, deployment, migrations, or destructive actions. Report necessary human decisions and verified blockers without inventing evidence or treating silence as approval.

`create` captures immutable inputs and pins the executable protocol and policies in `.reviews/<run-id>/`. Never hand-edit its database, snapshots, workspaces, or generated exports. Use a new run ID for a different scope. The local SQLite coordinator requires a reliable local filesystem; it is not an OS sandbox for trusted worker commands.

Before claiming work, present the review ID, captured revision, input paths, approval scope, and required actions in chat. Accept the user's explicit chat approval, including an unambiguous "go" replying to that scope; reuse approval already given for the same scope and revision. Record that consent through `authorize --chat-approval FILE` rather than asking the user to run a terminal command. Request only required actions (`review`, `inspect`, or separately authorized `execute`); never invent user consent. A new captured revision needs new authorization.

## Chat approval and a second agent

The approval JSON requires exactly `run_id`, `revision`, `approval_scope`, `actions`, `paths`, `message`, and `reference`. Copy the actual approval message into `message`; use a real conversation/message reference, or an accurate description of the approving exchange if no message ID is available. Take the run, revision and scope from `handoff` or the creation result, not memory. `paths` must match the captured paths. The helper records consent relayed by a trusted agent; it cannot authenticate chat history or determine whether text means approval.

Run `python3 /absolute/path/to/scripts/review.py --root <run-collection> --run <review-id> authorize --chat-approval <approval.json>`; `-` reads the JSON from stdin. The existing terminal `authorize` flow remains optional. This records review permission, not a review verdict or permission to implement recommendations.

After recording approval, run `plan` and `handoff`. Give the user a copyable joining prompt containing the review ID, absolute project and run directories, pinned revision, assigned role (`reviewer-2`), and a short task/feature brief. The joining agent needs access to that same local run directory; a review ID is not a remote invitation. When the user intends to bring a second agent, provide this handoff instead of silently substituting a spawned agent.

The joining prompt should tell agent 2 to use this skill, read `handoff` for the existing run, confirm its scope/revision, claim only its assigned role with `task claim --data -`, and inspect the pinned inputs. Do not create another run, repeat authorization, or claim the other reviewer's task. If the assigned task is already owned or completed, report its state rather than assuming ownership. Share the user's requirements, but keep initial findings independent until both reviews finish; then discuss and adapt only the separately authorized source files. Use `task submit` with the actual task/lease IDs and the normal AASM coverage/verdict requirements.

## Review workflow

- Run `doctor`, then `create --project <absolute-project> --paths <files-or-directories> --approval-scope <scope> --budget-seconds <seconds> --max-workers <count>`. Put global `--run <id>` and optional `--root <directory>` before the command.
- After recording user authorization, run `plan`. It creates blind whole-scope tasks for `reviewer-1` and `reviewer-2`, plus only requested `--risk` or `--specialist` work, and reserves challenge budget. Keep the initial reviews independent; do not share peer findings before both complete. One worker slot permits sequential independent reviews.
- Prefer `dispatch --timeout-seconds <seconds> --recipient <chat-identity> -- <absolute-adapter> [arguments]` after `plan` for continuous bounded scheduling of reviews, explicit peer questions and the final challenge. It wakes on completed local work and returns concrete blockers when no progress is possible; it never grants authority or retries failures.
- Run `work --worker reviewer-1 --timeout-seconds <seconds> -- <absolute-adapter> [arguments]` and the equivalent for `reviewer-2`. Each command runs one claimable task. Use the assigned identity; never complete another worker's lease. The adapter reviews its isolated snapshot copy, not moving project files.
- When independent reviews and required specialist work finish, run `challenge`, then `work --worker challenger ...`. The challenger checks integration, changes, assumptions, and recovery using current findings; it must be independent of the initial reviewers.
- Run `check` before reporting approval. Its combined result includes current approvals, task completion, coverage, blocking findings/questions, challenge, assumptions, research requirements, and integrity checks. Exit `0` means approved, `1` blocked, `2` invalid/error. Report the exact revision and remaining decisions; process success alone is not approval.
- Run `export` to regenerate reports under `.reviews/<id>/exports/`, and `finish` only after checks approve. Reports are derived from stored records/events, not hand-maintained copies.

## Worker evidence and records

Read [the AASM reference](references/aasm.md) for the strict grammar, operation schemas, and dialect negotiation. Adapters receive helper-generated JSON containing `task`, `snapshot`, and frozen `language` schemas on stdin; `snapshot.root` is their isolated workspace. Emit only UTF-8 AASM commands on stdout, logs on stderr. The helper generates JSON envelopes and persists validated bodies, metadata, and typed records in SQLite; agents do not author those envelopes.

Use `COV` with actual paths, status, and observed evidence, then exactly one final `VOTE` for review/challenge results. Its evidence IDs reference stored coverage; approved coverage must span the task scope. Use `B` only for verified blockers with actual snapshot `at` locations, claims, fixes and evidence; `D`/`O` are nonblocking. Report every distinct actionable finding without inventing evidence.

Understand intended behavior and the user's case before reviewing. Trace relevant callers/data flow, failure paths, conflicting assumptions and blast radius. Run relevant checks when feasible and name exact observed results in `COV.evidence`; explicitly record partial/unreviewed coverage and untested limitations. Each finding claim explains its triggering condition, observed behavior and impact, with an exact target, actual evidence and proposed fix; no fixed finding cap.

For interactive sessions, `task claim --data <JSON-file-or->` takes a worker identity and returns `id` plus `data.lease_id`; submit with `task submit --worker <name> --task-id <id> --lease-id <lease> --body <AASM-file-or->`. Administrative lifecycle and `record` commands retain explicit JSON inputs. Never write the database directly. Use `inspect` for authorized pinned input reads.

Use `message send --worker <name> --to <peer> --task-id <id> --lease-id <lease> --body <AASM-file-or->` and `message inbox --worker <name> --after <sequence>`. Bind discussion to the actual task and lease; peer inboxes remain closed until all initial blind reviews complete. Explicit `create --participants <names...>` enables authorized task-independent participants.

Only visible unanswered `Q`, `GAP` or `DISPUTE` messages create one bounded current discussion assignment per source message; `NOTE` prose and `A` replies never wake another worker. Discussion results answer assigned question IDs with `A` or report an explicit blocking `GAP`, without a `VOTE`. Failed tasks require an explicit recovery/retry decision. Assignments track question content versions: answered or edited questions supersede pending work and invalidate stale responses; schedule changed unanswered assignments only after prior running work finishes. A discussion worker's blocking `GAP` reply does not create an automatic followup; an explicit `Q` can. Peer traffic stays in the internal message transport; do not relay routine arrivals into the user's conversation.

For human discussion, restate the stable finding/question ID, short subject, concrete consequence and the question requiring a decision. Report concise done/doing/blocked/next status; use `status` for explicit user requests, without dumping AASM or every peer report.

To keep a background controller quiet, launch `dispatch` through the host's supported background facility with stdout/stderr redirected. The lead/adapter must call `discussion begin --recipient <chat-identity>` while discussing an item and `discussion end --recipient <chat-identity>` afterward. These commands only hold that recipient's app-consumable queue: they do not pause workers, detect typing or hide the host's own tool trace. Read `notifications --recipient <chat-identity> --after <sequence>` after discussion ends; persist the returned sequence only after delivery succeeds. Holds never consume a cursor, and a fresh revision excludes old updates. The repository does not bundle a host notification adapter or send foreground wake prompts; routine peer events must not resume the foreground assistant. Both one lead with subagents and separate chats use the same explicit recipient identity.

The fixed `aasm-1` grammar permits declarative vocabulary extensions, never executable plugins or authorization changes. Propose an immutable dialect, have every named participant independently test all example readbacks and ACK only after passing, then select the accepted version using `--dialect` on `work`, `task submit`, or `message send`. Preserve exact version/digest meaning. Exported definitions need fresh proposal/tests/ACKs in another run. Conformance proves interpretation consistency, not superior review quality.

## Changes, retries, and recovery

Apply separately authorized input changes outside the snapshot, then run `revise`. Review the new revision and renew user authorization in chat; old approvals do not transfer. Do not implement the reviewed plan as a consequence of approval.

Use `status --json` and `handoff` for current work, decisions, and checkpoints. `pause --reason <text>` prevents new claims; `resume` recovers expired readonly work. Cancellation of running work retains its lease/capacity until termination is acknowledged. Explicit `task retry --data <JSON-file>` takes `task_id` and `reason`, preserving history for failed/cancelled current tasks; do not loop retries without diagnosing the failure. Possibly live processes must terminate first. Uncertain execution effects require operator-confirmed `recover` with evidence; never blindly repeat mutations.

Protocol changes go through `protocol-proposal` for future runs. Active runs execute their pinned bundle; negotiated dialects add immutable data definitions without weakening code or policies. The retired file-based helper is no longer shipped; leave existing user state untouched and create a new coordinator run.
