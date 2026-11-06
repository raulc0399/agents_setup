---
name: collaborative-research
description: Research a question for the user's case with captured sources, independently verified claims, two independent reviews, and a final challenge; export an evidence-backed recommendation without implementing it.
---

# Collaborative research

Use the installed `collaborative-review/SKILL.md` for the shared coordinator and worker adapter contract. Codex installs skills under `~/.agents/skills`; Claude installs them under `~/.claude/skills`. New runs use that skill's `scripts/review.py`, SQLite records, and generated exports. For an existing `review-sync.py` run (`meta.json` without `run.db`), read [the retained legacy research protocol](references/legacy-protocol.md); never migrate its state implicitly.

Research approval covers a recommendation for the user's question, context, and constraints. It does not authorize implementing the recommendation, deployment, destructive actions, installations, or experiments. Keep project inputs read-only. The adapter executes trusted local code in an isolated snapshot copy; authorization checks are not an operating-system sandbox.

## Scope and capture

Establish the question, case, exclusions, expected deliverable, and source scope before research. Reuse an existing brief and relevant project files as captured inputs; do not create an empty `REPORT.md`. If a brief must be written, obtain the required project approval first. A later change to captured inputs requires `revise` and new revision-bound authorization/reviews.

Use an absolute helper path appropriate to the installation. The examples run from the project directory; replace `research/BRIEF.md` and `src` with existing inputs, and choose a budget appropriate to the work.

```bash
REVIEW="$HOME/.agents/skills/collaborative-review/scripts/review.py"
python3 "$REVIEW" --run research-001 create --project "$PWD" --paths research/BRIEF.md src --approval-scope report-ready --budget-seconds 1800 --max-workers 2
python3 "$REVIEW" --run research-001 authorize --actions review inspect --paths research/BRIEF.md src
```

The operator confirms the displayed exact authorization by typing `APPROVE` in a terminal. Agents cannot grant their own authority. Obtain the captured `revision` from `status --json`; record creation requires that exact value, not a branch name or working-tree hash.

## Sources and claims

Prefer official documentation, repositories, papers, and changelogs. Open each cited source. For prices, releases, policies, and other changing facts, check freshness and record the access date; explain outdated or conflicting evidence. The helper does not fetch URLs or prove facts automatically.

Persist records with `record KIND --data FILE.json` or `--data -` for a JSON object on stdin. JSON files are temporary request inputs, not parallel hand-maintained report state. Exact fields match `review_core/research.py` in the shared skill.

Source example (`record source`):

```json
{"source_id":"source-api","author":"researcher-1","revision":"<captured revision>","locator":"https://docs.example.org/api","access_date":"2026-11-06","content":"<actual captured text>","excerpt":"<exact substring of captured text>"}
```

Replace every placeholder with actual evidence; the example URL is a schema illustration, not a source. Supply exactly one of `content` or `file` (a local UTF-8 file). A locator is an HTTP(S) URL or `path:line`; access dates use `YYYY-MM-DD`. Source records preserve captured text, its SHA-256 hash, excerpt, locator, and date; sources are immutable. A hash proves captured-content identity, not the truth of its claims.

Claim example (`record claim`):

```json
{"claim_id":"claim-api","author":"researcher-1","revision":"<captured revision>","text":"<claim supported by the captured source>","source_ids":["source-api"],"verification":"unverified","impact":"decision-changing","inference":false,"contradictions":[]}
```

Use `impact: "not-decision-changing"` only for facts that do not change the decision. Mark deductions explicitly with `inference: true`; unsourced claims remain labeled assumptions. Verification does not turn an inference into a direct source statement. `contradictions` lists current claim IDs; unresolved contradictions affecting the recommendation block readiness. Sources and referenced claims must belong to the current revision.

An independent verifier opens the source and checks that it supports the precise claim for this case, then records `claim-verify`:

```json
{"claim_id":"claim-api","actor":"researcher-2","evidence":["<source ID/excerpt and explanation of support>"],"verification":"verified","source_ids":["source-api"]}
```

`verification` is `verified` or `disputed`; evidence is mandatory. `source_ids` optionally attaches or replaces captured support, allowing an unsourced claim to gain evidence. `contradictions` optionally replaces outgoing contradiction links; use `[]` only after evidence resolves the disagreement, and resolve incoming links on their declaring claims. Store history retains each change. Decision-changing verification must identify someone other than the claim's author. Never invent verification, reviewer identities, citations, or coverage.

Recommendation example (`record recommendation`):

```json
{"recommendation_id":"recommendation-api","author":"researcher-1","revision":"<captured revision>","text":"<recommendation and relevant alternatives>","claim_ids":["claim-api"],"case_basis":"<user constraints and why this recommendation fits>"}
```

Every recommendation needs current claim references and case-specific reasoning. All its supporting claims require independent verification and captured support regardless of their impact labels. Missing, unverified, disputed, or contradictory decision-changing support blocks report readiness. Resolve every distinct actionable gap; do not stop after a fixed finding count.

## Independent review and challenge

Two distinct reviewers independently cover the whole captured scope before reading each other's reviews. Request specialist tasks only for relevant risks; there is no fixed four-way split or random reporter. Keep the user informed concisely about findings, disagreements, and decisions needed.

```bash
python3 "$REVIEW" --run research-001 plan
python3 "$REVIEW" --run research-001 work --worker reviewer-1 --timeout-seconds 300 -- /absolute/reviewer-adapter
python3 "$REVIEW" --run research-001 work --worker reviewer-2 --timeout-seconds 300 -- /absolute/reviewer-adapter
python3 "$REVIEW" --run research-001 challenge
python3 "$REVIEW" --run research-001 work --worker challenger --timeout-seconds 300 -- /absolute/challenger-adapter
python3 "$REVIEW" --run research-001 check
```

Adapters consume the actual task/snapshot JSON on stdin and emit one result JSON object on stdout; logs go to stderr. Follow the shared skill's evidence-ID and typed coverage schema. Workers must review the immutable captured inputs and report genuine coverage, findings, questions, and `APPROVED` or `BLOCKED` verdicts; a successful process exit is insufficient. Collect and cross-check research evidence before scheduling the final challenge.

The final challenger has a distinct identity and receives the structured research records, source captures, verification state, and reviews in its task brief. Approval binds both the captured input revision and this structured context fingerprint. Changing a claim, source, recommendation, or other challenge input invalidates the old challenge; schedule and complete a fresh challenge. Do not describe the generated report as a separately snapshotted, exact-text approved artifact.

## Deliver and resume

After `check` returns `APPROVED`, run `finish` and `export`. Deliver `.reviews/research-001/exports/SUMMARY.md` as the derived report, with its source/claim/recommendation records and approval context; `OPTIONS.md`, `COVERAGE.md`, `DECISIONS.md`, and `TRANSCRIPT.html` provide supporting views. Exports are regenerated from Store records and events; never hand-edit them. Report the current check result as `APPROVED` or `BLOCKED`, not an invented `REPORT_APPROVED` status.

Use `status --json` for active work and blockers, `handoff` for a replacement session, and `resume` to recover expired work. Uncertain execution effects require an operator recovery decision. A user request to dig deeper requires new evidence and renewed applicable checks; prior approval covers only its previous captured inputs and structured context. Copy an export elsewhere only when the user authorized that destination, and obtain permission before overwriting an existing file.
