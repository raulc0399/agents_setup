---
name: collaborative-review
description: Review plans or code with two independent reviewers and an integration challenger against a pinned revision, recording findings, evidence, coverage, and approval gates. Use for collaborative readiness reviews; review approval does not authorize implementation.
---

# Collaborative review

Use `scripts/review.py` from this skill's installed directory, running from the project directory. Codex installs normally use `~/.agents/skills/collaborative-review/`; Claude installs use `~/.claude/skills/collaborative-review/`. Use an absolute script path; do not assume it exists in the reviewed project. Detailed CLI recipes and schemas are in the repository `README.md`; command `--help` is authoritative for installed flags.

## Scope and authority

Agree on explicit existing input paths and approval scope: `technical-plan`, `implementation-ready`, or `report-ready`. Review does not authorize implementing recommendations, deployment, migrations, or destructive actions. Report necessary human decisions and verified blockers without inventing evidence or treating silence as approval.

`create` captures immutable inputs and pins the executable protocol and policies in `.reviews/<run-id>/`. Never hand-edit its database, snapshots, workspaces, or generated exports. Use a new run ID for a different scope. The local SQLite coordinator requires a reliable local filesystem; it is not an OS sandbox for trusted worker commands.

Before claiming work, an operator must run `authorize` in a terminal, inspect the exact revision/actions/paths, and type `APPROVE`. Request only required actions (`review`, `inspect`, or separately authorized `execute`); agents must not grant themselves authority or invent unattended approval flags. A new captured revision needs new authorization.

## Review workflow

- Run `doctor`, then `create --project <absolute-project> --paths <files-or-directories> --approval-scope <scope> --budget-seconds <seconds> --max-workers <count>`. Put global `--run <id>` and optional `--root <directory>` before the command.
- After operator authorization, run `plan`. It creates blind whole-scope tasks for `reviewer-1` and `reviewer-2`, plus only requested `--risk` or `--specialist` work, and reserves challenge budget. Keep the initial reviews independent; do not share peer findings before both complete. One worker slot permits sequential independent reviews.
- Run `work --worker reviewer-1 --timeout-seconds <seconds> -- <absolute-adapter> [arguments]` and the equivalent for `reviewer-2`. Each command runs one claimable task. Use the assigned identity; never complete another worker's lease. The adapter reviews its isolated snapshot copy, not moving project files.
- When independent reviews and required specialist work finish, run `challenge`, then `work --worker challenger ...`. The challenger checks integration, changes, assumptions, and recovery using current findings; it must be independent of the initial reviewers.
- Run `check` before reporting approval. Its combined result includes current approvals, task completion, coverage, blocking findings/questions, challenge, assumptions, research requirements, and integrity checks. Exit `0` means approved, `1` blocked, `2` invalid/error. Report the exact revision and remaining decisions; process success alone is not approval.
- Run `export` to regenerate reports under `.reviews/<id>/exports/`, and `finish` only after checks approve. Reports are derived from stored records/events, not hand-maintained copies.

## Worker evidence and records

Read [the AASM reference](references/aasm.md) for the strict grammar, operation schemas, and dialect negotiation. Adapters receive helper-generated JSON containing `task`, `snapshot`, and frozen `language` schemas on stdin; `snapshot.root` is their isolated workspace. Emit only UTF-8 AASM commands on stdout, logs on stderr. The helper generates JSON envelopes and persists validated bodies, metadata, and typed records in SQLite; agents do not author those envelopes.

Use `COV` with actual paths, status, and observed evidence, then exactly one final `VOTE` for review/challenge results. Its evidence IDs reference stored coverage; approved coverage must span the task scope. Use `B` only for verified blockers with actual snapshot `at` locations, claims, fixes and evidence; `D`/`O` are nonblocking. Report every distinct actionable finding without inventing evidence.

For interactive sessions, `task claim --data <JSON-file-or->` takes a worker identity and returns `id` plus `data.lease_id`; submit with `task submit --worker <name> --task-id <id> --lease-id <lease> --body <AASM-file-or->`. Administrative lifecycle and `record` commands retain explicit JSON inputs. Never write the database directly. Use `inspect` for authorized pinned input reads.

Use `message send --worker <name> --to <peer> --task-id <id> --lease-id <lease> --body <AASM-file-or->` and `message inbox --worker <name> --after <sequence>`. Bind discussion to the actual task and lease; peer inboxes remain closed until all initial blind reviews complete. Explicit `create --participants <names...>` enables authorized task-independent participants.

The fixed `aasm-1` grammar permits declarative vocabulary extensions, never executable plugins or authorization changes. Propose an immutable dialect, have every named participant independently test all example readbacks and ACK only after passing, then select the accepted version using `--dialect` on `work`, `task submit`, or `message send`. Preserve exact version/digest meaning. Exported definitions need fresh proposal/tests/ACKs in another run. Conformance proves interpretation consistency, not superior review quality.

## Changes, retries, and recovery

Apply separately authorized input changes outside the snapshot, then run `revise`. Review the new revision and renew operator authorization; old approvals do not transfer. Do not implement the reviewed plan as a consequence of approval.

Use `status --json` and `handoff` for current work, decisions, and checkpoints. `pause --reason <text>` prevents new claims; `resume` recovers expired readonly work. Cancellation of running work retains its lease/capacity until termination is acknowledged. Explicit `task retry --data <JSON-file>` takes `task_id` and `reason`, preserving history for failed/cancelled current tasks; do not loop retries without diagnosing the failure. Possibly live processes must terminate first. Uncertain execution effects require operator-confirmed `recover` with evidence; never blindly repeat mutations.

Protocol changes go through `protocol-proposal` for future runs. Active runs execute their pinned bundle; negotiated dialects add immutable data definitions without weakening code or policies. The retired file-based helper is no longer shipped; leave existing user state untouched and create a new coordinator run.
