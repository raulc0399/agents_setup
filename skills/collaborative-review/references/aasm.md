# AASM communication

`review_core/aasm.py` defines `aasm-1`: a deterministic body language shared by workers and the coordinator. `messages.py` generates JSON envelopes with identities, revision, lease, sequence and frozen dialect metadata; `Store` persists them and typed effects in SQLite `run.db`. Bodies, envelopes and persistence are separate layers. Never hand-write envelopes or edit the database.

## Grammar and validation

One command per nonempty line: `UPPERCASE_OPCODE key=value key=value`. Keys are lowercase snake_case. Values use JSON strings, arrays, objects, numbers or booleans, or bare canonical tokens without whitespace, quotes, brackets, equals or backslashes. Quote text with spaces. Types still constrain each value: objects and numbers are not accepted where text is required. No implicit file context, indentation blocks, prose, or comments; use `NOTE text="..."` for information.

Unknown operations/fields, missing/duplicate operands, duplicate JSON keys, malformed/trailing tokens, invalid UTF-8, nonfinite numbers, booleans as integers and invalid IDs/paths/locations fail with a source line. Relative paths must be canonical, without traversal, empty components, backslashes or drive prefixes. Locations are `path:positive_line` or `path:start-end`, with end at least start; the bridge checks actual immutable snapshot line bounds. IDs start with an ASCII letter/digit and otherwise contain letters/digits or `_.:-`.

Limits apply to raw and canonical documents: 1 MiB total, 65,536 bytes per line, 4,096 commands. Defaults become explicit in normalized AST records: `{"op":"NOTE","args":{"text":"Observation","refs":[]}}`. Canonical formatting validates the roundtrip. No command executes shell code or grants authorization.

## Builtin operations

Required operands appear without brackets; brackets mean optional. `strings` is an array of nonempty text; `ids` and `paths` are typed arrays. A `+` suffix requires a nonempty array. `text` is nonempty text, `integer` means a positive integer, and `boolean` means JSON `true`/`false`.

| Operation | Operands | Effect |
| --- | --- | --- |
| `B` | `id:id`, `at:location`, `claim:text`, `fix:text`, `evidence:strings+`, `[severity:critical/high/medium/low="high"]` | Verified blocking finding. |
| `D` | `id:id`, `at:location`, `claim:text`, `fix:text`, `evidence:strings+`, `[severity:medium/low="medium"]` | Nonblocking finding detail. |
| `O` | `id:id`, `at:location`, `claim:text`, `fix:text`, `evidence:strings+`, `[severity:medium/low="low"]` | Optional nonblocking finding. |
| `FIX` | `id:id`, `evidence:strings+` | Resolve finding with evidence. |
| `REOPEN` | `id:id`, `reason:text`, `evidence:strings+` | Reopen finding. |
| `DISPUTE` | `id:id`, `finding:id`, `reason:text`, `evidence:strings+` | Create blocking question linked to finding. |
| `POINT` | `id:id`, `text:text`, `paths:paths+`, `evidence:strings+`, `[refs:ids=[]]` | Open one disputed point for both leads. |
| `PROPOSE` | `id:id`, `digest:text`, `text:text`, `evidence:strings+`, `[refs:ids=[]]` | Propose a point resolution; does not approve it. |
| `AGREE` | `id:id`, `digest:text`, `evidence:strings+` | Explicitly approve one exact point resolution and evidence bundle. |
| `CONTEST` | `id:id`, `digest:text`, `text:text`, `evidence:strings+`, `[refs:ids=[]]` | Challenge one point and require a revised resolution. |
| `Q` | `id:id`, `text:text`, `[findings:ids=[]]`, `[blocking:boolean=true]` | Create question. |
| `A` | `id:id`, `question:id`, `text:text`, `evidence:strings+`, `[option:id]` | Answer question. |
| `OPT` | `id:id`, `title:text`, `description:text`, `[findings:ids=[]]` | Create option. |
| `COV` | `id:id`, `paths:paths+`, `status:reviewed/partial/unreviewed`, `evidence:strings+`, `[findings:ids=[]]` | Record coverage. |
| `VOTE` | `verdict:APPROVED/BLOCKED`, `evidence:ids+` | Record task verdict with coverage references. |
| `NOTE` | `text:text`, `[refs:ids=[]]` | Information without approval effect. |
| `SRC` | `id:id`, `locator:text`, `date:text`, `content:text`, `[excerpt:text]` | Capture source. |
| `F` | `id:id`, `text:text`, `sources:ids`, `[impact:decision-changing/not-decision-changing="decision-changing"]`, `[inference:boolean=false]`, `[contradictions:ids=[]]` | Unverified research claim. |
| `CONFIRM` | `id:id`, `evidence:strings+`, `[sources:ids]`, `[contradictions:ids]` | Independently verify claim. |
| `GAP` | `id:id`, `text:text` | Blocking question about missing evidence. |
| `REC` | `id:id`, `text:text`, `claims:ids+`, `case:text` | Recommendation. |
| `RETIRE` | `kind:claim/recommendation`, `id:id`, `evidence:strings+`, `[replacement:id]` | Audited retirement; optional active replacement of the same kind. |
| `ASSUME` | `id:id`, `text:text`, `paths:paths+`, `[blocking:boolean=true]` | Record assumption. |
| `VERIFY` | `id:id`, `evidence:strings+` | Verify assumption. |
| `IMP` | `proposal:id`, `text:text` | Propose future improvement without authorization. |

`FIX` resolves a finding; `REOPEN` reopens it. `DISPUTE` creates a blocking linked question and does not dismiss its finding. `F` creates an unverified claim; `CONFIRM` is independent verification, not automatic source validation. `RETIRE` preserves authorship and history, requires resolution evidence, and cannot revive a record; active recommendations using retired claims still block readiness. `ASSUME` defaults to blocking; `VERIFY` records evidence of verification. `IMP` only proposes future improvements. `NOTE` has no approval effect.

Current plans also create `point-<dispute-id>` for each `DISPUTE`. The two leads iterate through automatically scheduled `consensus` tasks, one point per task, and both must submit `AGREE` for the same current digest. Changed resolutions or linked evidence invalidate only dependent point approvals. Consensus responses use exactly one `PROPOSE`, `CONTEST` or `AGREE` for the assigned point, or a blocking `GAP` when work needs external input; they do not use `VOTE`. Results automatically reach the other lead. See [iterative point agreement](consensus.md) for research teams, evidence linkage, and stopping conditions.

After dialect expansion, every builtin except `NOTE` and `IMP` requires a current `review` grant covering the task scope, or all run paths for taskless calls; unmapped domain extensions remain informational.

A review/challenge task result requires exactly one final `VOTE`; approval evidence references real reviewed coverage spanning the task scope. A peer message cannot approve a task. References must exist in the current revision or be created in the same atomic body; `NOTE refs` rejects ambiguous record types. Invalid commands, references, scope or locations roll back the whole application. Use factual evidence, not placeholder approvals.

## Sending and reading

Adapters receive task/snapshot/language JSON on stdin and return only AASM on stdout. For interactive work, claim using the administrative `task claim --data FILE`, then submit:

```bash
python3 "$REVIEW" --run review-001 task submit --worker reviewer-1 --task-id TASK --lease-id LEASE --body /absolute/path/to/result.aasm
python3 "$REVIEW" --run review-001 message send --worker reviewer-1 --to reviewer-2 --task-id TASK --lease-id LEASE --body /absolute/path/to/discussion.aasm
python3 "$REVIEW" --run review-001 message inbox --worker reviewer-2 --after 0
```

Replace `TASK`/`LEASE` with the returned `id`/`data.lease_id`. `--body -` reads stdin. `--reply-to ID` links an accessible existing conversation; `--id ID` supplies a stable retry ID. Identical retries do not reapply operations; changed bodies under the same ID fail. Inbox `after` is a durable event sequence and does not update read markers.

Workers bind current tasks to their asserted identity and matching lease; running leases cannot be expired. Completed tasks permit follow-up discussion. Peer inboxes remain closed while any current initial blind review is incomplete. For task-independent messaging, declare actual identities with `create --participants NAME NAME ...`; both participants and current review authorization are required. `--worker` is caller-supplied identity in a trusted local process model, not authentication. Do not impersonate peers. These commands do not authorize implementation.

For current plans, `poll --worker NAME --after SEQ --wait-seconds 30` and `dispatch` automatically exchange each team's submitted review after all blind tasks complete. The generated message references the review task in an informational `NOTE` and includes one blocking cross-check `Q`, bound to the author's completed task and lease. The assigned discussion brief includes the original review result, its AASM and dialect metadata; this does not reapply findings or grant a peer vote. Both concern teams must complete their cross-check assignments before the challenge; later unanswered peer questions use the normal follow-up rules. See [the team workflow](teams.md) for the active-chat loop and host delegation requirements.

## Peer followups and human presentation

Research tasks also receive `task.approval_scope=report-ready` and `task.research_context` (revision, captured records, material fingerprint). Initial tasks share the context frozen by `plan`; post-blind follow-ups receive current research. Blind references to common frozen research IDs are allowed, while later records owned by the other team remain private. A changed material fingerprint requires independent reassessment by both leads using administrative `record review` with the current `research_fingerprint`; a normal follow-up response cannot renew either verdict.

After the blind gate opens, `dispatch` schedules one revision-bound current discussion assignment per peer message containing applied, unanswered `Q`, `GAP` or `DISPUTE` records. The worker receives the original body, dialect identity and current question records. `NOTE` prose, answer-only replies, self messages and coordinator messages never schedule work; failures require explicit recovery. Assignments track question content versions: answered or edited questions supersede pending work and invalidate stale responses; changed unanswered assignments run after prior running work finishes. Blocking `GAP` replies from discussion workers never create automatic followup loops; explicit `Q` followups can. Unapplied questions raise an explicit scheduling blocker. Discussion output answers assigned IDs using `A` with evidence or reports an explicit blocking `GAP`, without `VOTE`; the helper atomically publishes the validated response to the original sender with `reply_to` and to the coordinator.

In human discussion, restate the stable item ID, short subject, concrete consequence and decision question. Findings explain their triggering condition, observed behavior and impact with exact targets and actual evidence. `COV.evidence` names actual check results, partial/unreviewed coverage and untested limitations; no fixed finding cap applies. Keep raw AASM and routine peer chatter internal.

`discussion begin/end --recipient NAME` holds/releases only that recipient's app-consumable notification queue. `notifications --recipient NAME --after SEQ` reads without consuming a cursor; held updates remain replayable. The lead/host adapter explicitly manages begin/end, delivery and the cursor after successful delivery. Updates and holds belong to the current revision. No host integration or typing detection is bundled; helpers cannot hide host tool traces or mute chat UI. Workers continue during discussion; explicit user status requests use `status`, while routine background events do not resume foreground conversation.

## Declarative dialects

`aasm-1` needs no negotiation. Extensions preserve grammar and add typed names with explicit meaning. Fields support `text`, `id`, `path`, `location`, `strings`, `ids`, `paths`, `boolean`, `integer`, and `enum`; each declares `required` as a boolean, and enum fields supply unique string choices. No builtin redefinition, recursive expansion, executable plugin, policy edit or shell evaluation is permitted.

Example manifest JSON (an informational alias, not an approval operation):

```json
{
  "name": "observations",
  "version": 1,
  "participants": ["reviewer-1", "reviewer-2"],
  "definitions": [{
    "op": "OBSERVE",
    "meaning": "Record an informational observation.",
    "fields": {"message": {"type": "text", "required": true}},
    "expands_to": {"op": "NOTE", "args": {"text": {"field": "message"}}}
  }],
  "examples": [{
    "body": "OBSERVE message=\"A fact\"",
    "expected": [{"op": "NOTE", "args": {"text": "A fact", "refs": []}}]
  }]
}
```

`expands_to` may map fields through `{"field":"name"}` or literals through `{"value":...}` to a builtin. Static mapping shape/types and runtime values must both validate. An extension without a mapping is stored as a typed domain message with its frozen meaning/schema and never silently becomes control or approval. The raw body, expanded AST and frozen dialect are retained; unmapped domain operations retain their original AST. Example expected AST uses expanded builtins and explicit defaults.

```bash
python3 "$REVIEW" --run review-001 dialect propose --worker reviewer-1 --manifest /absolute/path/to/manifest.json
python3 "$REVIEW" --run review-001 dialect test --worker reviewer-1 --id observations:1 --readback /absolute/path/to/readback-1.json
python3 "$REVIEW" --run review-001 dialect ack --worker reviewer-1 --id observations:1
python3 "$REVIEW" --run review-001 dialect test --worker reviewer-2 --id observations:1 --readback /absolute/path/to/readback-2.json
python3 "$REVIEW" --run review-001 dialect ack --worker reviewer-2 --id observations:1
python3 "$REVIEW" --run review-001 dialect show --id observations:1
python3 "$REVIEW" --run review-001 dialect export --id observations:1
```

Each participant supplies their own readback JSON array of `{"body":...,"expected":[...]}` covering every manifest example. The helper parses examples and compares supplied expanded meanings; every new operation must appear in examples. A failed test cannot be ACKed. Only all fixed participants' passing tests and ACKs make that immutable version usable; ACKs bind to the caller-asserted participant, without authentication; do not ACK as a peer. Select it with `--dialect observations:1` on `work`, `task submit`, or `message send`.

The stored manifest includes identity, base, author and digest. Changed definitions need a new name/version; accepted definitions cannot mutate. Export preserves reusable definition/example data, but another run needs its own proposal, readbacks and ACKs. Executable code/policy remain pinned for the run. Conformance tests show deterministic interpretation; they do not demonstrate model understanding or improved review quality. Quality comparisons require actual measured runs through `evaluate`; do not invent benchmarks or claim AASM is superior.
