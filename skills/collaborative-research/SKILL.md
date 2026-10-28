---
name: collaborative-research
description: Two agent sessions research any question together (a codebase, a new feature, tools and practices on the internet, or both), cross-check each other's sourced findings, show them to the user, then write and jointly approve a report with a recommendation for the user's case (REPORT_APPROVED). Builds on the collaborative-review protocol and helper.
---

## Base protocol

This skill extends `collaborative-review`. Read its `SKILL.md` first; everything in it applies unless overridden here:

- Claude: `$HOME/.claude/skills/collaborative-review/SKILL.md`
- Codex: `$HOME/.agents/skills/collaborative-review/SKILL.md`

`rs` is the same helper (`scripts/review-sync.py` of that skill), run from the **project directory**. Section numbers like "review §1" refer to that `SKILL.md`.

# Goal: collaborative research between two agent sessions

Answer the user's research question with evidence, for the user's own case, until both agents approve the **same unchanged revision** of `research/<run-id>/REPORT.md`. Do not implement the recommendation.

## Rules (in addition to the review Rules)

- Every finding cites a source: a URL, or `file:line` in the project. Unsourced claims are labeled `assumption`.
- Prefer primary sources (official docs, repositories, papers, changelogs) over blogs; note the date of time-sensitive facts (versions, prices, model names).
- The project codebase is read-only. No installs, experiments, or code changes without explicit user permission (`USR`).
- Both slots and their research subagents run read-only research tools themselves (web search and fetch, file search, `git log`), overriding review §2; tests, experiments, and snapshot edits stay with the executor.
- Keep the agent's own context small: delegate research, source verification, cross-checks, and findings/report reviews to subagents, which return only `F`/verdict lines; the parent owns every verdict. Store long subagent results in `research/<id>/<slot>-scratch/`, not in messages.
- Files in `research/<run-id>/` are owned as listed in section 2; `<slot>-*` files and folders belong to that slot. Never write a file you do not own.
- The review skill's `reviews/<run-id>/proposed-changes.diff` and `reviews/<run-id>/TRANSCRIPT.html` live in `research/<run-id>/` instead, outside the `REPORT.md` snapshot.

## 0. Setup

As review §0, with these differences:

```bash
mkdir -p research/review-001 && touch research/review-001/REPORT.md
rs --run review-001 init --approval-scope report-ready --paths research/review-001/REPORT.md   # first agent
rs --run review-001 start --agent agent-1                                                       # first slot
rs --run review-001 join --agent agent-2                                                        # second slot
```

- Only `REPORT.md` is the snapshot; notes and findings live beside it, outside the snapshot.
- Approval scope is always `report-ready` → `REPORT_APPROVED`.

## 1. Kickoff (Brief)

As review §1, but `prob` is the research question, and the reporter's kickoff message is:

```
Run: review-001 · Scope: report-ready · Report: research/review-001/REPORT.md
Question: <one line>
Case: <the user's context: project, constraints, goals>
Sources: <codebase paths | web | both> · Out of scope: <…>
Exclusions: <criteria that may rule options out (licence, cost, vendor data terms, hardware, maturity) | none: tradeoffs only>
Deliverable: <first output the user needs, if the question has one> · Save copy to: <path | none>
Slots: agent-1=<CLI>, agent-2=<CLI> · Roles: executor=<slot>, reporter=<slot> (random)
Reply "go" to start, or tell me what to change.
```

If the question, case, sources, or exclusions are unclear, the slot that ran `init` asks the user before drawing roles and sending `KICK`. Settling exclusions up front avoids re-running research after the user lifts a constraint.

## 2. Phases

Each phase has a color; the reporter uses it in every status line (section 4).

| Phase | Color | Who | Output (owner) |
|-------|-------|-----|----------------|
| 1 Brief | 🔵 | reporter + user | kickoff, GO |
| 2 Research | 🟡 | both, independently | `research/<id>/<slot>-notes.md` (each slot its own) |
| 3 Cross-check | 🟠 | both | `F`/`CONFIRM`/`DISPUTE`/`GAP` messages |
| 4 Findings | 🟢 | executor merges, reporter shows the user | `research/<id>/FINDINGS.md` (executor) |
| 5 Report | 🟣 | executor writes, peer reviews | `research/<id>/REPORT.md` (executor, snapshot) |
| 6 Approval | ✅ | both | `REPORT_APPROVED` |

**2 Research.** Each agent covers the whole question on its own, without reading the peer's notes. Split the work across your own subagents, for example:
- landscape: what exists (tools, services, libraries, models), with maturity and cost;
- practice: how people use it (docs, examples, issues, discussions), common patterns and pitfalls;
- fit: the user's codebase and constraints, what integrates and what conflicts;
- risks: licensing, security, cost, lock-in, maintenance.

Verify subagent findings at the source before writing them to your notes; a verification subagent may do this, and you own the verdict and spot-check. Mark each `F` line `[v]` (source opened and supports the claim), `[u]` (subagent-cited, not opened) or `[p]` (proposal or inference). The executor merges `[u]` lines only as labelled candidates; a `[u]` finding the peer confirms (`CONFIRM`, source opened) is merged as `[v]`. Write them to your notes file, one `F` line each (section 3), then send `NOTES research/<id>/<slot>-notes.md "<n> findings"` (control `info`).

Give every subagent brief the current decision ledger (section 2a), so subagents do not re-apply a constraint the user has lifted.

**3 Cross-check.** After both `NOTES`, read the peer's notes. Open the sources of findings rated `high`, directly or through a verification subagent, then answer with `CONFIRM`, `DISPUTE`, or `GAP`. Research a `GAP` and add the result to your own notes. The finding's author answers each `DISPUTE` once, with a revised `F` line or `A <id> "keep: <why>"`. Cross-check ends after these answers; anything still disputed or uncovered goes into the findings as an open question.

**4 Findings.** The executor merges confirmed findings into `FINDINGS.md`: what exists, how it is used, what is relevant to the case, open questions, with sources. The peer checks it against both notes files and the decision ledger (missing ids, contradicted revisions, re-applied exclusions, unlabelled `[u]`/inference claims) and sends `ACK <seq>` of the executor's `MERGED` message, or `Q`. The reporter then shows the user the path, the kickoff `Deliverable` (if any), the top findings, the open questions, and the executor's proposed `REPORT.md` text (review §6 proposed changes), and asks: write the report, dig deeper into …, or drop …. The user's answer is relayed as `USR`; "write the report" approves that proposed text, and later `REPORT.md` edits follow review §6 proposed changes. Deeper research returns to phase 2 for the named topic.

**5 Report.** An authoring task (review §6): the executor runs `begin-edit`, writes `REPORT.md`, runs `revise`, and publishes its own review. `REPORT.md` contains:
1. Question and case.
2. What exists.
3. How people use it.
4. What is relevant to this case, and why.
5. Recommendation: the kickoff `Deliverable` first (if any), what to apply here, alternatives with tradeoffs, next steps.
6. Open questions and sources.

**6 Approval.** The peer reviews `REPORT.md` against `FINDINGS.md` and the sources (review §4, §7). A blocker is a wrong or unsourced claim, a finding missing that changes the recommendation, or a recommendation that ignores the case. After the current revision's `REPORT_APPROVED`, the executor copies `REPORT.md` unchanged to the user-authorized "Save copy to" path, if any (if a file already exists there, ask the user before overwriting); links relative to `research/<id>/` are reported to the user, not silently rewritten.

### 2a. Decision ledger

The reporter keeps `research/<id>/DECISIONS.md` (reporter-owned, outside the snapshot): one line per relayed `USR`/`SCOPE` with its sequence, verbatim text and resulting rule, latest first. It records only explicit relayed user decisions. A later decision that widens or lifts a constraint names the decisions it supersedes, and each agent re-checks its own notes for findings framed under the old rule (`F` revisions, not new research, unless the user asks to reopen). Agents and subagents read this file instead of searching messages.

When research reopens after `REPORT_APPROVED` ("dig deeper"), the approved report covers only its prior scope. New notes and proposals stay separate until a new exact-text `USR` and matching approvals of the new revision; old consensus is never presented as approval of the deeper research.

### 2b. Context handoff

When an agent's context runs low, or the user asks for a restart, that agent writes `research/<id>/<slot>-handoff.md`: helper path, run ID, slot and role, last processed peer sequence and own sequence, the decision ledger state, phase and what is done or pending, open `Q`/`DISPUTE` items, owned files, and the restart steps. It then idles without `FINISHED`. The user exits the old CLI, which stops its watcher; the slot is reclaimable once its heartbeat is stale (over 120 s). The new session reads the handoff first, reclaims the slot with `start` (review §10 crash/restart), sends a readiness `Q`, and drains from the recorded cursor. The reporter tells the user the restart command.

## 3. AASM additions

| Op | Carried by | Meaning |
|----|-----------|---------|
| `F <id> [v\|u\|p] "<finding>" src=<url\|file:line> rel=high\|med\|low` | notes file line | sourced finding; `[v\|u\|p]` = verification (phase 2), `rel` = relevance to the case |
| `NOTES <file> "<n> findings"` | `control --kind info` | own research done |
| `CONFIRM <ids>` | `control --kind info` | peer findings verified at the source |
| `DISPUTE <id> "<why not>" src=<url\|file:line>` | `control --kind info` | finding wrong or outdated (in phase 6: `publish`) |
| `GAP "<missing area>"` | `control --kind info` | area neither notes file covers |
| `MERGED <file>` | `control --kind info` | executor merged `FINDINGS.md` |

Finding IDs follow the review ID rule: `1F3`, `2F7`.

## 4. Status reports (reporter → user)

As review §8, with the phase and its color at the start of the first line:

```
🟡 Phase 2 Research · [<run> · round <n>] 🟦 Topic: <what is being researched or discussed>
🟨 Options: <real alternatives with brief tradeoffs, or agreed decision: <decision>>
🟩 Agent 1: <position with brief evidence>
🟪 Agent 2: <position with brief evidence; disagreement: <where> | none>
🟧 Next: <current action or waiting decision>
```

Every status report uses this template, at least once at each phase change. The user may intervene at any time (review §8).

## 5. Stop and logoff

- Results: `REPORT_APPROVED`, `BLOCKED`, `WAITING_FOR_OTHER_AGENT` (review §10).
- Report rounds are limited as in review §10.
- After `REPORT_APPROVED`, the reporter gives the user the report path and a one-line recommendation, then follows the review transcript and logoff (review §9, §10); the transcript covers all phases.
