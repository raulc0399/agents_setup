
to install: clone and symlink\
the symlink name might change based on the coding agent used

```bash
git clone git@github.com:raulc0399/agents_setup.git
cd agents_setup
ln -s "$(pwd)/AGENTS.md" ~/.claude/CLAUDE.md
ln -s "$(pwd)/AGENTS.md" ~/.gemini/GEMINI.md
```

## Collaborative review

`skills/collaborative-review/scripts/review.py` coordinates revision-bound local reviews with SQLite event history, two independent concern teams, automatic peer exchange and polling, iterative agreement on each disputed point, and a final challenger. Each lead spawns its own subagent per relevant concern and consolidates a whole-scope review. Requires Python 3.10+, POSIX and writable reliable local storage supporting SQLite WAL and cross-process locking. No model account or provider adapter is bundled; active agent chats use their host's delegation tools.

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
python3 "$REVIEW" --run design-review authorize --chat-approval /absolute/path/to/chat-approval.json
python3 "$REVIEW" --run design-review plan
python3 "$REVIEW" --run design-review dispatch --timeout-seconds 300 -- "$ADAPTER"
python3 "$REVIEW" --run design-review check
python3 "$REVIEW" --run design-review export
```

The lead presents the captured scope in chat, accepts the user's explicit approval, and records it with `authorize --chat-approval FILE` (`-` reads stdin). The JSON contains exactly these fields, filled from the real run and approving exchange:

```json
{
  "run_id": "design-review",
  "revision": "<exact revision from create or handoff>",
  "approval_scope": "technical-plan",
  "actions": ["review", "inspect"],
  "paths": ["design.md"],
  "message": "<actual user approval message>",
  "reference": "<conversation/message reference or accurate exchange description>"
}
```

The helper checks run identity, current revision, approval scope and captured paths, then stores the approval message/reference with the grant. It trusts the calling agent to relay real user consent; it cannot authenticate chat history or interpret approval language. Do not combine `--chat-approval` with terminal authorization options. Calling `authorize` without that option retains the interactive terminal flow. Review permission does not authorize implementing recommendations, deployment or destructive actions. Commands enforce scoped authorization within a trusted local process model; this is not an OS sandbox. `doctor` performs local temporary-file and subprocess lock probes without launching your adapter, installing packages or contacting accounts; successful probes cannot certify storage durability.

After approval, run `plan` and `handoff --worker reviewer-1`, then give the user the review ID and a joining prompt such as:

```text
Use $collaborative-review. Join review <review-id> as reviewer-2.
Project: <absolute-project>; shared run directory: <absolute-run-directory>.
Revision: <captured-revision>. Task: <user's requirements and feature brief>.
Read handoff --worker reviewer-2 for that run, verify its scope/revision, claim only reviewer-2,
and start your own subagent per relevant concern against the pinned inputs.
Keep your team independent; do not edit source files or create another run.
After submitting your team's review, automatically keep polling as reviewer-2,
claim your assigned follow-ups, and route them to your concern subagents.
Challenge each disputed point separately, start fresh research/review teams when
needed, and require explicit agreement from both leads on each point's resolution.
Continue until completion, an explicit blocker, or the run deadline without user prompts.
```

Agent 2 uses the run directory's parent as `--root` and its name as `--run`. Both sessions need access to the same reliable local storage; the ID alone does not connect remote machines. Joining does not require new authorization for the unchanged scope. If reviewer-2 is already claimed or completed, report that state. Initial findings stay independent until both reviews finish. Existing runs keep their frozen helper version; start a fresh run to use a new authorization workflow rather than editing a frozen bundle.

Both leads must supply real evidence, concern/subagent provenance and reviewed coverage for every captured file; missing or blocked results keep `check` blocked. `plan --risk NAME --specialist NAME` adds coordinator specialist work without replacing either lead's concern team. Workers may run concurrently within `--max-workers`; host subagent capacity is separate and children must fit their parent's deadline. Each `work` invocation runs one task. `check` exits 0 for approved, 1 for blocked, 2 for invalid requests; `finish` marks the run complete only after checks approve.

For separate active chats, follow [the concern-team loop](skills/collaborative-review/references/teams.md). Each lead calls this automatically after submitting its initial review, processes returned messages, claims its own ready tasks and polls again:

```bash
python3 "$REVIEW" --run design-review poll --worker reviewer-1 --after 0 --wait-seconds 30
```

Use `reviewer-2` in the other chat. `poll` returns `state` (`ready`, `waiting`, `blocked`, or `complete`), `messages`, own ready `tasks`, `next_after`, `challenge_tasks` for reviewer-1, and any blockers/terminal approval. Save `next_after` only after processing; replay is safe. Polling checks every 0.1 seconds and waits at most 30 seconds per call; `--timeout-seconds` separately sets the follow-up execution limit (default 300, bounded by plan/run budgets). It automatically publishes both reviews after all blind tasks finish and schedules cross-checks/follow-ups; it never claims tasks or starts models. Both active leads keep the loop running without user prompts, within the run deadline. A closed chat requires a host integration to resume it. For unattended work, supply a real team-capable adapter to `dispatch` instead of competing with the chats for their roles.

### Quiet continuous dispatch and discussion

After authorization and `plan`, `dispatch` schedules the two team reviews, automatically exchanges them after the blind gate opens, runs both peer cross-checks and iterative point deliberations, and finally schedules the independent challenge. The plan budgets the initial tasks, two cross-checks and the challenge, leaving two additional time shares for point deliberation within the same run budget. `dispatch` returns concrete blockers when no work can progress; it never retries failures, recovers uncertain effects or authorizes actions. Completion means the queue drained; `check` determines approval.

Each disagreement becomes a separate `POINT` (or `point-<id>` for a finding `DISPUTE`). The helper assigns one point per `consensus` task: the leads challenge, research, `PROPOSE` and `CONTEST` until both explicitly `AGREE` to that point's exact resolution/evidence digest. They may start fresh concern teams for additional work. Proposing is not approval, answering a question is not agreement, and a whole-review `VOTE` cannot approve all points. Changed resolutions or linked evidence reopen only dependent points. `SUMMARY.md` lists their individual states and approvals; [the point workflow](skills/collaborative-review/references/consensus.md) defines the commands and stopping conditions.

```bash
python3 "$REVIEW" --run design-review dispatch --timeout-seconds 300 --recipient lead-chat -- "$ADAPTER" > /absolute/path/review-dispatch.log 2>&1 &
python3 "$REVIEW" --run design-review discussion begin --recipient lead-chat
# Discuss one item while workers continue.
python3 "$REVIEW" --run design-review discussion end --recipient lead-chat
python3 "$REVIEW" --run design-review notifications --recipient lead-chat --after 0
```

Use the host's supported background facility or the shell launch above. The controller queues one concise completion/blocker summary without foreground wake prompts or routine peer output. A lead/host adapter must explicitly call discussion begin/end and deliver notifications; none is bundled. Holds affect only that recipient's presentation queue, never workers or explicit `status` requests. Save the last delivered sequence after delivery succeeds; held messages remain durable and replayable. Updates and holds belong to the current revision. Helpers cannot detect typing, hide host tool traces or automatically mute chat UI. Both one lead with subagents and separate chats use explicit recipient identities.

Applied unanswered ordinary `Q` and `GAP` peer messages schedule one current discussion assignment per source message after blind reviews finish; current-plan `DISPUTE` messages instead enter the per-point consensus loop. `NOTE` prose and `A` replies do not wake workers. Discussion results answer assigned IDs using `A` or report an explicit blocking `GAP`, without `VOTE`; failed tasks require explicit recovery. Assignments track question content versions: answered or edited questions supersede pending work and invalidate stale responses; changed unanswered assignments run after prior running work finishes. A blocking `GAP` reply from a discussion worker does not schedule another automatic discussion; an explicit followup `Q` can. Unapplied questions return an explicit blocker. Peer communication remains internal during human discussion.

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

`status --json` shows work, budgets, decision requests and uncovered paths; `handoff` emits a replacement worker checkpoint. During blind review use `handoff --worker <own-role>`; it exposes only your task details and peer status, while unfiltered `status` and `handoff` can expose peer evidence. `export` regenerates `.reviews/design-review/exports/OPTIONS.md`, `COVERAGE.md`, `DECISIONS.md`, `SUMMARY.md`, and `TRANSCRIPT.html` from stored records/events, with current approval checks. Do not hand-edit generated reports.

`pause --reason TEXT` stops new claims; `resume` reconstructs expired work without automatically repeating uncertain writes. `recover --data FILE` requires terminal `APPROVE`; JSON contains `task_id`, `outcome` (`completed`, `failed`, `cancelled`, `retry`) and factual `evidence`. When process termination cannot be observed, establish that the worker stopped: either pass `process_termination_evidence` with the recovery decision, or first use `recover --confirm-termination --data FILE` with only `task_id` and factual `evidence`. Then resolve or retry the task explicitly. Never retry uncertain execution solely because its lease expired.

### Research and evaluation

Agents submit research using AASM `SRC`, `F`, `CONFIRM`, `GAP`, and `REC`. The separate administrative `record KIND --data FILE` interface (or `--data -` for stdin) validates a JSON object against the corresponding public function in `review_core/findings.py`, `assumptions.py`, or `research.py`; manual callers supply author/revision where required. Sources retain supplied content, locator, access date and hash; no automatic fetch occurs. Claims use `source_ids`; `record claim-verify` accepts `claim_id`, `actor`, `evidence`, optional `verification`, `contradictions`, `source_ids`. Record recommendations against supported claims, and verify blocking assumptions before approval. `report-ready` additionally checks research support.

```bash
python3 "$REVIEW" evaluate --cases /absolute/path/to/cases.json --results /absolute/path/to/measured-results.json
```

Both files contain JSON arrays. Cases require nonempty `id`, `trigger`, `expected_finding`, `source_commit`, `test`; the repository's grounded cases are in `skills/collaborative-review/scripts/fixtures/review-evaluation.json`. Each measured run requires unique `run_id`, `strategy`, `detected_ids`, nonnegative finite `elapsed_seconds` and `cost`, and a shared `currency`. Unknown detections count as false positives, missed case IDs as misses; output compares supplied strategies without model calls or invented measurements. Parser/dialect conformance tests establish deterministic interpretation, not better review quality; no language superiority is claimed without measured runs.

The retired file-based helper is no longer shipped. Existing user run data is left untouched; use a new run ID for this coordinator.
