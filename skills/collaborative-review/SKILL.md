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

The trusted adapter receives one JSON object on stdin containing `task` and `snapshot`; `snapshot.root` is its isolated writable workspace. Emit exactly one JSON object on stdout; send logs to stderr. Include `evidence_ids` and typed `coverage`; review/challenge outputs also require `verdict: APPROVED|BLOCKED`. Findings, questions, and options may accompany the result. Evidence references attest what the worker checked; strings alone do not prove a claim.

Coverage entries name concrete snapshot paths, `status: reviewed|partial|unreviewed`, and nonempty `evidence`. Give challenge coverage explicit stable `coverage_id` values and reference those same IDs in `evidence_ids`; cover every scope path. Verify every claimed blocker against actual files or check output; mark unverified issues accordingly. Report every distinct actionable finding with its location, consequence, and proposed correction.

Use `record <kind> --data <JSON-file-or->` for typed manual records. Use `task` lifecycle commands for existing interactive worker sessions; submission requires the current worker and lease. Never bypass validation by writing directly to the Store. Use `inspect` to read/search authorized pinned inputs without an arbitrary shell-command allowlist.

## Changes, retries, and recovery

Apply separately authorized input changes outside the snapshot, then run `revise`. Review the new revision and renew operator authorization; old approvals do not transfer. Do not implement the reviewed plan as a consequence of approval.

Use `status --json` and `handoff` for current work, decisions, and checkpoints. `pause --reason <text>` prevents new claims; `resume` recovers expired readonly work. Cancellation of running work retains its lease/capacity until termination is acknowledged. Explicit `task retry --data <JSON-file>` takes `task_id` and `reason`, preserving history for failed/cancelled current tasks; do not loop retries without diagnosing the failure. Possibly live processes must terminate first. Uncertain execution effects require operator-confirmed `recover` with evidence; never blindly repeat mutations.

Protocol changes go through `protocol-proposal` for future runs. Active runs execute their pinned bundle; do not weaken their policies in place. For an existing `scripts/review-sync.py` run (`meta.json` without `run.db`), read [the retained legacy protocol](references/legacy-protocol.md) and use that helper; never convert its state or use its random-role/tmux workflow for new runs.
