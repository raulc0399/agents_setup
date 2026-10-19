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
- Files in `research/<run-id>/` are owned as listed in section 2; never write a file you do not own.
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
Slots: agent-1=<CLI>, agent-2=<CLI> · Roles: executor=<slot>, reporter=<slot> (random)
Reply "go" to start, or tell me what to change.
```

If the question, case, or sources are unclear, the slot that ran `init` asks the user before drawing roles and sending `KICK`.

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

Verify subagent findings yourself. Write them to your notes file, one `F` line each (section 3), then send `NOTES research/<id>/<slot>-notes.md "<n> findings"` (control `info`).

**3 Cross-check.** After both `NOTES`, read the peer's notes. Open the sources of findings rated `high`, then answer with `CONFIRM`, `DISPUTE`, or `GAP`. Research a `GAP` and add the result to your own notes. The finding's author answers each `DISPUTE` once, with a revised `F` line or `A <id> "keep: <why>"`. Cross-check ends after these answers; anything still disputed or uncovered goes into the findings as an open question.

**4 Findings.** The executor merges confirmed findings into `FINDINGS.md`: what exists, how it is used, what is relevant to the case, open questions, with sources. The peer checks it against both notes files and sends `ACK <seq>` of the executor's `MERGED` message, or `Q`. The reporter then shows the user the path, the top findings, the open questions, and the executor's proposed `REPORT.md` text (review §6 proposed changes), and asks: write the report, dig deeper into …, or drop …. The user's answer is relayed as `USR`; "write the report" approves that proposed text, and later `REPORT.md` edits follow review §6 proposed changes. Deeper research returns to phase 2 for the named topic.

**5 Report.** An authoring task (review §6): the executor runs `begin-edit`, writes `REPORT.md`, runs `revise`, and publishes its own review. `REPORT.md` contains:
1. Question and case.
2. What exists.
3. How people use it.
4. What is relevant to this case, and why.
5. Recommendation: what to apply here, alternatives with tradeoffs, next steps.
6. Open questions and sources.

**6 Approval.** The peer reviews `REPORT.md` against `FINDINGS.md` and the sources (review §4, §7). A blocker is a wrong or unsourced claim, a finding missing that changes the recommendation, or a recommendation that ignores the case.

## 3. AASM additions

| Op | Carried by | Meaning |
|----|-----------|---------|
| `F <id> "<finding>" src=<url\|file:line> rel=high\|med\|low` | notes file line | sourced finding; `rel` = relevance to the case |
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
