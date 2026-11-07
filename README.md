
to install: clone and symlink\
the symlink name might change based on the coding agent used

```bash
git clone git@github.com:raulc0399/agents_setup.git
cd agents_setup
ln -s "$(pwd)/AGENTS.md" ~/.claude/CLAUDE.md
ln -s "$(pwd)/AGENTS.md" ~/.gemini/GEMINI.md
```

## Collaborative review

`skills/collaborative-review/scripts/review.py` coordinates revision-bound local reviews with SQLite event history, two independent reviewers and a final challenger. Requires Python 3.10+, POSIX and writable reliable local storage supporting SQLite WAL and cross-process locking. No model account or provider adapter is bundled.

Install the skills for your agent (keep the clone available):

```bash
mkdir -p ~/.agents/skills ~/.claude/skills
ln -s "$(pwd)/skills/collaborative-review" ~/.agents/skills/collaborative-review
ln -s "$(pwd)/skills/collaborative-research" ~/.agents/skills/collaborative-research
ln -s "$(pwd)/skills/collaborative-review" ~/.claude/skills/collaborative-review
ln -s "$(pwd)/skills/collaborative-research" ~/.claude/skills/collaborative-research
```

### Run a review

Set `REVIEW` to this clone's absolute script path, `ADAPTER` to a real trusted executable adapter you supply, and select existing project paths. Run these commands from the project directory; `design.md` below is your actual input file.

```bash
REVIEW=/absolute/path/to/agents_setup/skills/collaborative-review/scripts/review.py
ADAPTER=/absolute/path/to/your-approved-worker-adapter
python3 "$REVIEW" doctor --project "$PWD" -- "$ADAPTER"
python3 "$REVIEW" --run design-review create --project "$PWD" --paths design.md --approval-scope technical-plan --budget-seconds 3600 --max-workers 2
python3 "$REVIEW" --run design-review authorize --actions review inspect --paths design.md
python3 "$REVIEW" --run design-review plan
python3 "$REVIEW" --run design-review work --worker reviewer-1 --timeout-seconds 300 -- "$ADAPTER"
python3 "$REVIEW" --run design-review work --worker reviewer-2 --timeout-seconds 300 -- "$ADAPTER"
python3 "$REVIEW" --run design-review challenge
python3 "$REVIEW" --run design-review work --worker challenger --timeout-seconds 300 -- "$ADAPTER"
python3 "$REVIEW" --run design-review check
python3 "$REVIEW" --run design-review export
```

`authorize` displays the exact revision, actions and paths and requires the operator to type `APPROVE` in a terminal; there is no unattended approval flag. Review permission does not authorize implementing recommendations, deployment or destructive actions. Commands enforce scoped authorization within a trusted local process model; this is not an OS sandbox. `doctor` performs local temporary-file and subprocess lock probes without launching your adapter, installing packages or contacting accounts; successful probes cannot certify storage durability.

Both reviewers must supply real evidence and reviewed coverage for every captured file; missing or blocked results keep `check` blocked. `plan --risk NAME --specialist NAME` adds requested specialist work, which needs additional `work` invocations. Workers may run concurrently within `--max-workers`; each `work` invocation runs one task. `check` exits 0 for approved, 1 for blocked, 2 for invalid requests; `finish` marks the run complete only after checks approve.

### Quiet continuous dispatch and discussion

After authorization and `plan`, `dispatch` promptly schedules claimable reviews, visible explicit peer questions and the final challenge within worker capacity and existing budget. It returns concrete blockers when no work can progress; it never retries failures, recovers uncertain effects or authorizes actions. Completion means the queue drained; `check` determines approval.

```bash
python3 "$REVIEW" --run design-review dispatch --timeout-seconds 300 --recipient lead-chat -- "$ADAPTER" > /absolute/path/review-dispatch.log 2>&1 &
python3 "$REVIEW" --run design-review discussion begin --recipient lead-chat
# Discuss one item while workers continue.
python3 "$REVIEW" --run design-review discussion end --recipient lead-chat
python3 "$REVIEW" --run design-review notifications --recipient lead-chat --after 0
```

Use the host's supported background facility or the shell launch above. The controller queues one concise completion/blocker summary without foreground wake prompts or routine peer output. A lead/host adapter must explicitly call discussion begin/end and deliver notifications; none is bundled. Holds affect only that recipient's presentation queue, never workers or explicit `status` requests. Save the last delivered sequence after delivery succeeds; held messages remain durable and replayable. Updates and holds belong to the current revision. Helpers cannot detect typing, hide host tool traces or automatically mute chat UI. Both one lead with subagents and separate chats use explicit recipient identities.

Only applied unanswered `Q`, `GAP` and `DISPUTE` peer messages schedule one current discussion assignment per source message after blind reviews finish. `NOTE` prose and `A` replies do not wake workers. Discussion results answer assigned IDs using `A` or report an explicit blocking `GAP`, without `VOTE`; failed tasks require explicit recovery. Assignments track question content versions: answered or edited questions supersede pending work and invalidate stale responses; changed unanswered assignments run after prior running work finishes. A blocking `GAP` reply from a discussion worker does not schedule another automatic discussion; an explicit followup `Q` can. Unapplied questions return an explicit blocker. Peer communication remains internal during human discussion.

Present a stable finding/question ID, short subject, concrete consequence and decision question; give short done/doing/blocked/next updates with actual evidence and location. Reviewers understand the user's case, trace callers/data flow, examine failure paths, conflicting assumptions and blast radius, and run relevant checks when feasible. `COV.evidence` names exact observed checks/results, partial/unreviewed coverage and untested limitations. Finding claims explain condition, behavior and impact with an exact target, actual evidence and proposed fix; no fixed finding cap applies.

### AASM worker and message contract

Worker communication uses the strict [AASM language](skills/collaborative-review/references/aasm.md). The adapter receives helper-generated JSON on stdin: `task`, `snapshot`, and frozen `language` metadata (`id`, `digest`, `definitions`, `builtin_schemas`, `instructions`). `snapshot.root` is an isolated writable copy of pinned inputs. Environment supplies `REVIEW_RUN_DIR`, `TASK_ID`, and `LEASE_ID`. Emit UTF-8 AASM commands on stdout and logs on stderr; do not construct result JSON envelopes.

For example, after actually reviewing `design.md` and observing the stated evidence:

```text
COV id=coverage-reviewer-1 paths=["design.md"] status=reviewed evidence=["Checked every requirement against the design"]
VOTE verdict=APPROVED evidence=["coverage-reviewer-1"]
```

Use factual observations, unique stable IDs, and full task coverage; a canned verdict is invalid. Review/challenge results require exactly one final `VOTE` referencing coverage IDs. `B` records verified blockers; `D`/`O` record nonblocking findings; `Q`, `A`, and `OPT` make disagreements explicit. The helper validates scope, references, real snapshot locations, identity and lease, then atomically stores typed records, normalized operations, and its JSON envelope in SQLite `run.db`. AASM is the body language; JSON carries metadata; SQLite is durable state. None grants authorization.

Interactive workers can claim and submit without an adapter:

```bash
printf '%s\n' '{"worker":"reviewer-1"}' | python3 "$REVIEW" --run design-review task claim --data -
python3 "$REVIEW" --run design-review inspect --paths design.md
python3 "$REVIEW" --run design-review task submit --worker reviewer-1 --task-id <claimed-id> --lease-id <claimed-lease> --body /absolute/path/to/review.aasm
```

The claim returns `id` and `data.lease_id`. Review pinned inputs, renew long work using `task renew --data FILE` (`task_id`, `worker`, `lease_id`, `lease_seconds`), and repeat for reviewer-2 and challenger. Task `fail`, `cancel`, and `retry` retain their administrative JSON inputs and attempt history. Retries share the original allocation; exhaustion requires a new task or run.

For task-bound discussion, use `message send --worker reviewer-1 --to reviewer-2 --task-id <id> --lease-id <lease> --body /absolute/path/to/message.aasm`; read with `message inbox --worker reviewer-2 --after 0`. `--body -` reads stdin; `--reply-to <message-id>` links a reply. Messages bind to the current task/revision/lease. Blind reviewers cannot read peer messages until every initial blind review completes. For task-independent conversations, declare identities at `create --participants NAME NAME ...`; scoped review authorization is still required. Metadata and IDs are generated by the helper.

### Negotiated dialects

`aasm-1` is always available. Grammar stays fixed; new named operations are immutable declarative schemas with explicit meanings, optional builtin expansions, and executable conformance examples. They cannot run code or change authorization. Unmapped extensions remain typed domain records. Code and policy stay pinned for the run.

Use `dialect propose --worker <author> --manifest FILE`, then each named participant supplies all independent example readbacks with `dialect test --worker <name> --id <dialect-id> --readback FILE` and runs `dialect ack --worker <name> --id <dialect-id>`. Only passing participants can ACK; every required ACK is needed before use. `dialect show --id <id>` reports agreement; `dialect export --id <id>` emits reusable immutable definition/example data. Import by proposing, testing, and ACKing again; changed definitions require a new name/version. Select an accepted version with `work --dialect`, `task submit --dialect`, or `message send --dialect`. See the AASM reference for manifest/readback shapes.

### Revisions, reports and recovery

`technical-plan`, `implementation-ready`, and `report-ready` describe the approval target; approval always belongs to the captured revision. Source project files remain untouched. Apply authorized changes outside the snapshot, then run `revise` with no running tasks, authorize the new revision and plan again. Old approvals cannot approve changed inputs; affected assumptions are invalidated. Active runs execute their pinned protocol bundle; `protocol-proposal --text TEXT --evidence TEXT` records future changes, which require a new run to take effect.

`status --json` shows work, budgets, decision requests and uncovered paths; `handoff` emits a replacement worker checkpoint. `export` regenerates `.reviews/design-review/exports/OPTIONS.md`, `COVERAGE.md`, `DECISIONS.md`, `SUMMARY.md`, and `TRANSCRIPT.html` from stored records/events, with current approval checks. Do not hand-edit generated reports.

`pause --reason TEXT` stops new claims; `resume` reconstructs expired work without automatically repeating uncertain writes. `recover --data FILE` requires terminal `APPROVE`; JSON contains `task_id`, `outcome` (`completed`, `failed`, `cancelled`, `retry`) and factual `evidence`. When process termination cannot be observed, establish that the worker stopped: either pass `process_termination_evidence` with the recovery decision, or first use `recover --confirm-termination --data FILE` with only `task_id` and factual `evidence`. Then resolve or retry the task explicitly. Never retry uncertain execution solely because its lease expired.

### Research and evaluation

Agents submit research using AASM `SRC`, `F`, `CONFIRM`, `GAP`, and `REC`. The separate administrative `record KIND --data FILE` interface (or `--data -` for stdin) validates a JSON object against the corresponding public function in `review_core/findings.py`, `assumptions.py`, or `research.py`; manual callers supply author/revision where required. Sources retain supplied content, locator, access date and hash; no automatic fetch occurs. Claims use `source_ids`; `record claim-verify` accepts `claim_id`, `actor`, `evidence`, optional `verification`, `contradictions`, `source_ids`. Record recommendations against supported claims, and verify blocking assumptions before approval. `report-ready` additionally checks research support.

```bash
python3 "$REVIEW" evaluate --cases /absolute/path/to/cases.json --results /absolute/path/to/measured-results.json
```

Both files contain JSON arrays. Cases require nonempty `id`, `trigger`, `expected_finding`, `source_commit`, `test`; the repository's grounded cases are in `skills/collaborative-review/scripts/fixtures/review-evaluation.json`. Each measured run requires unique `run_id`, `strategy`, `detected_ids`, nonnegative finite `elapsed_seconds` and `cost`, and a shared `currency`. Unknown detections count as false positives, missed case IDs as misses; output compares supplied strategies without model calls or invented measurements. Parser/dialect conformance tests establish deterministic interpretation, not better review quality; no language superiority is claimed without measured runs.

The retired file-based helper is no longer shipped. Existing user run data is left untouched; use a new run ID for this coordinator.
