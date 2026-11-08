# Concern teams and automatic communication

Use this workflow for both collaborative skills. The two reviewers are independent team leads, including when they run in separate agent chats.

```text
reviewer-1                              reviewer-2
  |-- concern A subagent                  |-- concern A subagent
  |-- concern B subagent                  |-- concern B subagent
  `-- further relevant concerns          `-- further relevant concerns
  |                                       |
  +-- consolidate --> shared run <-- consolidate --+
                         |
               both blind reviews finish
                         |
              automatic review exchange
                         |
              each lead + its own team
              cross-check peer evidence
                         |
              separate disputed points
                         |
              challenge / research / revise <---+
                         |                      |
              both approve THIS point? -- no ---+
                         |
                all points agreed
                         |
              independent challenger
                         |
                  check and export
```

## Start independent teams

Each lead claims its own whole-scope task and identifies relevant concerns from the user's requirements and captured inputs. Use the concerns in the joining brief and add any others the scope requires. Concerns are concrete review questions, such as retry correctness or source freshness, not an obligatory fixed list.

Spawn a separate subagent for each concern using the host's delegation tools. Give each child the pinned revision, captured input location, requirements, its concern and relevant paths. Children inspect inputs read-only; they never edit project files, claim the other lead's task, or use the other team's results. Both teams may read the same files. Do not share child agents between teams or delegate recursively. Queue children when host capacity is full; `--max-workers` limits coordinator tasks, not host subagents. All child work must fit the parent task's remaining time and the host's limits.

Each child returns its actual identity, inspected paths, observed check results, every actionable finding, and uncertainties. The lead verifies and consolidates these results, merges duplicates, checks interactions between concerns, and covers any remaining scope. Include the concern-to-child mapping and concrete evidence in the submitted `COV.evidence`; only the lead submits the team's single final `VOTE`. Missing concern coverage must remain explicit. If delegation is unavailable or a child fails, report the missing work with `GAP` and a blocked verdict rather than inventing a team or claiming complete coverage.

For research, retain actual authorship of captured sources and claims. Use `record source --data FILE` and `record claim --data FILE` with the real child's `author` and current `revision`; use `record claim-verify --data FILE` with the actual verifier's `actor`, `claim_id` and observed `evidence`. Obtain these records from the child and retain the child-to-concern mapping in `COV.evidence`. Do not relay a child's `SRC`/`F`/`CONFIRM` as the lead: AASM uses the submitting worker's identity. Both teams may reference and verify the common records frozen in the initial `research_context`. For later private team discoveries during blind review, submit child record IDs as textual coverage evidence, not AASM references to another author's private records; use the administrative `record recommendation` route if a lead needs to reference its team's claims. The other team independently verifies that new support after the blind gate opens; neither a new label nor the lead relaying a child's work counts as independent verification.

Material research changes make the initial research verdicts stale without discarding their history. After blind completion, read the current `research_context` from handoff or the follow-up adapter request, reassess it independently with each lead's own team, and renew each lead's verdict through `record review` with its current `research_fingerprint`, completed initial `task_id`, and actual coverage evidence. Ordinary verification attestations do not invalidate the research fingerprint; source/support changes, disputed claims and retirement do. Follow-up `A` replies and point `AGREE` actions alone do not renew a research review. The final challenge waits for both current verdicts.

## Keep both chats processing work

Do not end the turn after submitting the initial review or ask the user to prompt either agent to check messages. Each lead runs this loop automatically using the existing absolute helper/run paths and its own identity:

- Call `poll --worker <own-role> --after <processed-sequence> --wait-seconds 30`; start at sequence `0`. The helper checks for work every 0.1 seconds and returns within 30 seconds. `--timeout-seconds` sets follow-up execution time separately (default 300 seconds, bounded by the plan/run budget). A `waiting` result means call it again, not ask the user to continue.
- Process returned `messages` and retain `next_after` only after processing succeeds. Keep the cursor with the session's handoff context; replay from `0` if uncertain. Reading never consumes messages. Join or check progress with `handoff --worker <own-role>`, which includes your task details and only peer status. Before the blind gate opens, do not use unfiltered `handoff`, `status`, exports, or direct database reads; they expose peer results.
- For a returned own `tasks` entry, use `task claim --data -` with `worker`, its `task_id`, and a suitable `lease_seconds`. The returned lease, not the initial review lease, authorizes that follow-up. Read the claimed brief, route its questions to the relevant concern subagents, and submit the consolidated response with `task submit`. Renew running leases while children work; renewal cannot extend the task budget.
- Answer ordinary question IDs with evidence-backed `A` records. Create a separate `POINT` or `DISPUTE` for every disagreement and follow [the per-point loop](consensus.md): fresh concern teams as needed, proposals and rebuttals until both leads explicitly agree on that point. Task submission routes responses to the peer and coordinator automatically. Send questions arising outside an assigned task through `message send`. A `GAP` is for an actual blocker, not ordinary disagreement.
- When `challenge_tasks` is returned to reviewer-1, start a fresh challenger subagent that belongs to neither concern team. It claims the supplied task as `challenger` and submits its own coverage and verdict. Reviewer-1 keeps polling; reviewer-2 does not claim the challenger task.
- Stop on `complete`, an explicit `blocked` result, a user pause/cancellation, or the agreed run deadline. On `blocked`, surface concrete decisions or missing capability; do not loop retries. Use the run's `budget_seconds` as a wall-clock ceiling from joining as well as respecting coordinator task spending. The lead runs `check`, exports the result, and calls `finish` only after approval. Preserve handoff context if a host limit ends the session earlier.

The first `poll` or dispatcher cycle after every blind task completes publishes each review to the other lead exactly once and creates two cross-check assignments. Each disagreement then progresses through its own automatically scheduled consensus tasks; approval of one point never approves another. The challenger waits for both cross-checks, both approvals on every point, and all required follow-ups. Pending or running review teams keep their findings private, including during concurrent polling.

`poll` does not claim tasks, start models, or wake a closed chat. The active leads execute this loop with host tools. For unattended execution use one `dispatch` controller and a real adapter capable of running each lead's concern subagents; do not let it compete with separate chats for their lead roles. The adapter receives the task on stdin, returns AASM on stdout, and must finish its child work before returning. If neither active host delegation nor such an adapter is available, report that capability blocker.

## Joining prompt

Include the skill's absolute path, run ID, project/run directories, pinned revision, assigned role, requirements and concern brief. Tell the joining agent to use `handoff --worker reviewer-2`, then add:

```text
Claim only reviewer-2 and immediately start your own read-only concern subagents.
Keep your team independent until both blind reviews are submitted.
After submission, automatically poll as reviewer-2, process peer messages, claim
your assigned follow-ups, and route questions to your concern subagents.
Challenge each disputed point separately; start fresh research/review teams when
needed and continue until both leads explicitly approve that point's resolution.
Continue until completion, an explicit blocker, or the run deadline; do not wait
for the user to relay messages. Use the existing run and authorization.
```
