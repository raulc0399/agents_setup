# Iterative agreement on individual points

Read this after the initial independent team reviews, together with [the team loop](teams.md). Each disagreement is a separate point; there is no batch approval of all disputed conclusions.

## Identify and challenge

During the automatic peer cross-check, examine the other team's evidence and explain every disagreement. Use `DISPUTE` for an existing finding; it also creates `point-<dispute-id>`. Use `POINT` for a separate research, design or review disagreement:

```text
POINT id=retry-policy text="Whether retries preserve the input" paths=["src/worker.py"] evidence=["Observed retry path at src/worker.py:42"] refs=["retry-finding"]
```

Use actual IDs, captured paths and observed evidence. Include all affected paths. `refs` links the point to supporting findings, sources, claims or other typed evidence; their current content and transitive dependencies contribute to its evidence digest. Do not combine unrelated concerns into one point, dismiss a disagreement in an `A` reply, or interpret silence as approval. Ordinary factual questions can still use `Q`/`A`; disagreements must use `POINT` or `DISPUTE` so their two approvals are enforced.

## Work one point per assignment

`poll` and `dispatch` automatically schedule `consensus` tasks. Each task contains exactly one `point_id`, its current `point_digest`, the proposed resolution, latest counterargument, supporting records and approvals. One lead works on a given point at a time; different points may progress independently within capacity. The other lead receives the result automatically.

- Investigate the assigned point and challenge unsupported assumptions. Either lead may start a fresh subagent team for additional research or review whenever the evidence is insufficient; give it the specific disagreement, current proposal, counterargument and pinned inputs. Preserve actual subagent identities and observations in the response evidence.
- Use `PROPOSE id=<point-id> digest=<assigned-digest> text="<specific resolution>" evidence=["<observed support>"] refs=["<evidence-id>"]` to propose or revise the resolution. Proposing never approves it; the helper creates a new content version and both leads must examine it.
- Use `CONTEST id=<point-id> digest=<assigned-digest> text="<specific counterargument>" evidence=["<observed contradiction>"] refs=["<evidence-id>"]` when it is still wrong or incomplete. This is normal iteration: the helper sends the point back to the other lead for more investigation and a revised proposal, without asking the user to relay messages.
- Use `AGREE id=<point-id> digest=<assigned-digest> evidence=["<your independent checks>"]` only when you accept that exact point's resolution and evidence. The proposer must also explicitly agree. A completed response, an `A`, or a whole-review `VOTE` cannot substitute for either lead's point approval.

Return exactly one of these actions for the assigned point, with supporting source/claim/coverage records as appropriate. Do not approve other points in that response. Keep proposing, contesting and independently checking until both leads agree on the same digest. A blocking `GAP` is for evidence that cannot be obtained within authorized work, not ordinary disagreement or work another subagent can perform.

The helper compares the assigned digest before accepting a response. If another session changes the point or its evidence while a task runs, the obsolete response is retained as superseded without applying its effects, and current work is scheduled. Capture updates to existing referenced evidence before claiming the next point task; changing that evidence and approving an old digest in one response is rejected. Newly captured supporting records may accompany a proposal referencing them.

## Preserve settled points and finish

Approvals apply only to one point's exact resolution, counterargument and linked evidence. Editing the proposal or its linked evidence invalidates that point's old approvals; unrelated points remain settled. A new `CONTEST` can reopen an agreed point. Histories and prior approvals remain in the stored events, but only approvals matching the current digest count.

Agreement on a finding dispute also answers its linked dispute question. It does not apply a code fix, dismiss the original finding, or override a blocked initial review. Record justified finding resolutions and independently reconsider your own initial verdict with concrete coverage evidence through the existing record APIs; do not change the peer's verdict. If the captured input itself needs edits, apply only changes already authorized by the user and use `revise`; the initial go covers reviewing the new revision within scope, while fresh review verdicts and a new challenge remain required.

The common result consists of the individually agreed resolutions plus the uncontested, independently reviewed conclusions. `SUMMARY.md` lists every point, its resolution/version, current approvals and evidence digest; `TRANSCRIPT.html` retains the discussions and prior versions. Only after all points have both approvals and required follow-ups finish does the independent challenger run. A new point raised by the challenger returns to the same lead-to-lead loop and requires a fresh final challenge afterward.

Existing coverage, source verification and approval gates still apply. Stop with explicit unresolved point IDs on exhausted budget, unavailable required evidence, execution failure, user pause or cancellation. Never force agreement, fabricate verification, silently skip a point, or restart failed work automatically. Both active leads continue polling without user prompts while useful authorized work remains.
