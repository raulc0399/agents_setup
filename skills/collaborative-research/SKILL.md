---
name: collaborative-research
description: Research a question for the user's case with captured sources, independently verified claims, two independent concern teams, automatic peer exchange and polling, and a final challenge; export an evidence-backed recommendation without implementing it.
---

# Collaborative research

Use the installed `collaborative-review/SKILL.md` for the shared coordinator and worker adapter contract. Codex installs skills under `~/.agents/skills`; Claude installs them under `~/.claude/skills`. New runs use that skill's `scripts/review.py`, SQLite records, and generated exports.

Read its [concern teams and automatic communication](../collaborative-review/references/teams.md) reference before starting or joining. Each lead must use its own concern subagents, consolidate their evidence, and automatically process peer messages and follow-up tasks. Keep the two teams independent until both initial reviews are submitted.

Research approval covers a recommendation for the user's question, context, and constraints. It does not authorize implementing the recommendation, deployment, destructive actions, installations, or experiments. Keep project inputs read-only. The adapter executes trusted local code in an isolated snapshot copy; authorization checks are not an operating-system sandbox.

## Scope and capture

Establish the question, case, exclusions, expected deliverable, and source scope before research. Reuse an existing brief and relevant project files as captured inputs; do not create an empty `REPORT.md`. If a brief must be written, obtain the required project approval first. A later change to captured inputs requires `revise` and new revision-bound authorization/reviews.

Use an absolute helper path appropriate to the installation. The examples run from the project directory; replace `research/BRIEF.md` and `src` with existing inputs, and choose a budget appropriate to the work.

```bash
REVIEW="$HOME/.agents/skills/collaborative-review/scripts/review.py"
python3 "$REVIEW" --run research-001 create --project "$PWD" --paths research/BRIEF.md src --approval-scope report-ready --budget-seconds 1800 --max-workers 2
```

Present the captured scope in chat and record the user's explicit approval with `authorize --chat-approval FILE`, following the shared review skill's approval JSON contract. Reuse approval already given for that scope and revision; do not require a terminal confirmation or invent consent. Obtain the captured `revision` from creation or `handoff --worker <own-role>` after planning; record creation requires that exact value, not a branch name or working-tree hash. After authorization and `plan`, provide the review ID, shared run directory, revision, assigned role, and a copyable joining prompt for the second agent as described in the shared skill.

## Sources and claims

Prefer official documentation, repositories, papers, and changelogs. Open each cited source. For prices, releases, policies, and other changing facts, check freshness and record the access date; explain outdated or conflicting evidence. The helper does not fetch URLs or prove facts automatically.

Use AASM bodies through the shared skill's `task submit` or `message send`; read its [AASM reference](../collaborative-review/references/aasm.md) for named operands and dialect agreement. The helper generates JSON transport metadata, validates typed effects, and persists SQLite records. Administrative `record KIND --data FILE` remains available for explicit local tooling; agents do not construct task/message JSON envelopes.

Capture sources with `SRC id=<id> locator="<actual URL or path:line>" date="YYYY-MM-DD" content="<captured text>"`, optionally `excerpt="<exact substring>"`. Dates must be actual access dates. Stored sources preserve text, date, locator and content hash; hashes prove identity, not truth. The helper does not fetch sources.

Record an unverified claim with `F id=<id> text="<precise claim>" sources=["<source-id>"]`; defaults are `impact=decision-changing`, `inference=false`, `contradictions=[]`. Mark deductions with `inference=true`; verification cannot turn an inference into a direct statement. Unsourced claims remain unsupported. All references belong to the current revision.

An independent verifier opens the captured source, checks its support for this case, then uses `CONFIRM id=<claim-id> evidence=["<actual support>"]`, optionally `sources=[...]` and `contradictions=[...]`. Only replace contradiction links with `[]` after evidence resolves them; incoming links must be resolved on their declaring claims. Decision-changing claims need a verifier other than their author. Use `GAP id=<id> text="<missing evidence>"` for blocking gaps and explicit questions/findings for disputes; never fabricate verification identities or evidence.

Use `REC id=<id> text="<recommendation>" claims=["<claim-id>"] case="<user constraints and reasoning>"`. Every supporting claim requires independent verification and captured support regardless of impact. Missing, disputed, unverified, or contradictory decision-changing support blocks report readiness. Resolve every distinct actionable gap without a fixed finding count.

## Independent review and challenge

Two distinct team leads each delegate every relevant concern to their own subagents and consolidate whole-scope reviews before reading the other team's findings. Choose concerns from the user's case, sources, claims, alternatives and risks; there is no fixed four-way split or random reporter. Additional coordinator specialist tasks do not replace either team's subagents. Keep the user informed concisely about findings, disagreements, and decisions needed.

```bash
python3 "$REVIEW" --run research-001 plan
python3 "$REVIEW" --run research-001 dispatch --timeout-seconds 300 -- /absolute/team-capable-adapter
python3 "$REVIEW" --run research-001 check
```

Adapters consume helper-generated task/snapshot/language JSON on stdin and emit UTF-8 AASM on stdout; logs go to stderr. Follow the shared skill's `COV` and final `VOTE` requirements. Workers must review the immutable captured inputs and report genuine coverage, findings, questions, and `APPROVED` or `BLOCKED` verdicts; a successful process exit is insufficient. Collect and cross-check research evidence before scheduling the final challenge.

Reviewers must understand the user's case, trace how supporting claims lead to the recommendation, and examine conflicting evidence, failure cases and assumptions. Name actual verification/check results in `COV.evidence`, mark partial/unreviewed coverage and untested uncertainty explicitly, and explain each finding's condition, observed behavior and impact with concrete support and fix.

Use the shared skill's `dispatch` command with a team-capable adapter, or each active lead's automatic `poll` loop in separate chats. Both paths exchange the submitted reviews after the blind gate opens and require both teams to cross-check the other's evidence before the challenge. Route source-verification questions to the relevant concern subagents and preserve independent claim verification. Keep routine peer messages internal; further explicit unanswered `Q`/`GAP`/`DISPUTE` messages schedule follow-up work. In human discussion, restate the stable item ID, subject, consequence and decision question. Use the shared `discussion begin/end` and durable `notifications --after` queue for background updates; the lead/host adapter must explicitly manage the hold and delivery cursor. This does not detect typing, suppress host tool traces or pause the research workers.

The final challenger has a distinct identity and receives the structured research records, source captures, verification state, and reviews in its task brief. Approval binds both the captured input revision and this structured context fingerprint. Changing a claim, source, recommendation, or other challenge input invalidates the old challenge; schedule and complete a fresh challenge. Do not describe the generated report as a separately snapshotted, exact-text approved artifact.

## Deliver and resume

After `check` returns `APPROVED`, run `finish` and `export`. Deliver `.reviews/research-001/exports/SUMMARY.md` as the derived report, with its source/claim/recommendation records and approval context; `OPTIONS.md`, `COVERAGE.md`, `DECISIONS.md`, and `TRANSCRIPT.html` provide supporting views. Exports are regenerated from Store records and events; never hand-edit them. Report the current check result as `APPROVED` or `BLOCKED`, not an invented `REPORT_APPROVED` status.

During blind review use `handoff --worker <own-role>` so peer results stay private; afterward use `status --json` for work/blockers and `handoff` for a replacement session. Use `resume` to recover expired work. Uncertain execution effects require an operator recovery decision. A user request to dig deeper requires new evidence and renewed applicable checks; prior approval covers only its previous captured inputs and structured context. Copy an export elsewhere only when the user authorized that destination, and obtain permission before overwriting an existing file.
