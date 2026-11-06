
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

`authorize` displays the exact revision, actions and paths and requires the operator to type `APPROVE` in a terminal; there is no unattended approval flag. Review permission does not authorize implementing recommendations, deployment or destructive actions. Commands enforce scoped authorization within a trusted local process model; this is not an OS sandbox. `doctor` performs local temporary-file and subprocess lock probes without launching your adapter, installing packages or contacting accounts; successful probes cannot certify storage durability. Use `--legacy-tmux` only to check the optional legacy dependency.

Both reviewers must supply real evidence and reviewed coverage for every captured file; missing or blocked results keep `check` blocked. `plan --risk NAME --specialist NAME` adds requested specialist work, which needs additional `work` invocations. Workers may run concurrently within `--max-workers`; each `work` invocation runs one task. `check` exits 0 for approved, 1 for blocked, 2 for invalid requests; `finish` marks the run complete only after checks approve.

### Adapter contract

The trusted executable receives one JSON object on stdin and emits one JSON result on stdout; send logs to stderr. No provider-specific command flags are assumed. Input contains `task` (`id`, `kind`, `revision`, `brief`, `scope_paths`, `worker`, `attempt`) and `snapshot` (`root`, `revision`, `files`). `snapshot.root` is an isolated writable copy of the pinned inputs; use it as the review workspace. Environment supplies `REVIEW_RUN_DIR`, `TASK_ID`, `LEASE_ID`. Adapters must preserve independent reviewer identities and do actual review before returning a verdict.

Output requires `evidence_ids` (nonempty strings) and `coverage` objects with `paths`, `status` (`reviewed`, `partial`, `unreviewed`), `evidence` and `finding_ids`; review/challenge tasks also require `verdict` (`APPROVED` or `BLOCKED`). Evidence must describe actual observations. Optional `findings`, `questions`, `options` use the validated record schemas in `review_core/findings.py`; findings include `claim`, `proposed_fix`, `paths`, `evidence`, `severity`, `verified`, `blocking`. The coordinator binds adapter records to the leased author and revision and validates submissions atomically. Never return a canned approval.

For a challenge, give each coverage object an explicit `coverage_id` and include that same ID in `evidence_ids`; its reviewed paths must cover the full task scope. A file reference alone cannot substitute for the challenge's stored coverage record.

Existing interactive agents can use the manual task interface without an adapter:

```bash
printf '%s\n' '{"worker":"reviewer-1"}' | python3 "$REVIEW" --run design-review task claim --data -
python3 "$REVIEW" --run design-review inspect --paths design.md
python3 "$REVIEW" --run design-review task submit --data /absolute/path/to/actual-task-submission.json
```

The claim returns the task envelope with `id` and `data.lease_id`; submission contains `task_id`, `worker`, `lease_id`, and `result` with the same validated result contract above. Review only the pinned inputs, renew a long-running lease with `task renew --data FILE` (`task_id`, `worker`, `lease_id`, `lease_seconds`), and repeat for reviewer-2 and challenger. `task fail`, `cancel`, and explicit `retry` accept the corresponding `Coordinator` method arguments in `review_core/coordinator.py`; retries retain attempt history.

Retries share the task's original time allocation. An exhausted allocation requires a new task or run; retrying does not increase the budget.

### Revisions, reports and recovery

`technical-plan`, `implementation-ready`, and `report-ready` describe the approval target; approval always belongs to the captured revision. Source project files remain untouched. Apply authorized changes outside the snapshot, then run `revise` with no running tasks, authorize the new revision and plan again. Old approvals cannot approve changed inputs; affected assumptions are invalidated. Active runs execute their pinned protocol bundle; `protocol-proposal --text TEXT --evidence TEXT` records future changes, which require a new run to take effect.

`status --json` shows work, budgets, decision requests and uncovered paths; `handoff` emits a replacement worker checkpoint. `export` regenerates `.reviews/design-review/exports/OPTIONS.md`, `COVERAGE.md`, `DECISIONS.md`, `SUMMARY.md`, and `TRANSCRIPT.html` from stored records/events, with current approval checks. Do not hand-edit generated reports.

`pause --reason TEXT` stops new claims; `resume` reconstructs expired work without automatically repeating uncertain writes. `recover --data FILE` requires terminal `APPROVE`; JSON contains `task_id`, `outcome` (`completed`, `failed`, `cancelled`, `retry`) and factual `evidence`. When process termination cannot be observed, establish that the worker stopped: either pass `process_termination_evidence` with the recovery decision, or first use `recover --confirm-termination --data FILE` with only `task_id` and factual `evidence`. Then resolve or retry the task explicitly. Never retry uncertain execution solely because its lease expired.

### Research and evaluation

`record KIND --data FILE` (or `--data -` for stdin) validates a JSON object against the corresponding public function in `review_core/findings.py`, `assumptions.py`, or `research.py`; manual callers supply author/revision where required. Sources retain supplied content, locator, access date and hash; no automatic fetch occurs. Claims use `source_ids`; `record claim-verify` accepts `claim_id`, `actor`, `evidence`, optional `verification`, `contradictions`, `source_ids`. Record recommendations against supported claims, and verify blocking assumptions before approval. `report-ready` additionally checks research support.

```bash
python3 "$REVIEW" evaluate --cases /absolute/path/to/cases.json --results /absolute/path/to/measured-results.json
```

Both files contain JSON arrays. Cases require nonempty `id`, `trigger`, `expected_finding`, `source_commit`, `test`; the repository's grounded cases are in `skills/collaborative-review/scripts/fixtures/review-evaluation.json`. Each measured run requires unique `run_id`, `strategy`, `detected_ids`, nonnegative finite `elapsed_seconds` and `cost`, and a shared `currency`. Unknown detections count as false positives, missed case IDs as misses; output compares supplied strategies without model calls or invented measurements.

Existing `review-sync.py` runs retain their legacy helper and state format; the new coordinator does not convert them. Read the retained [legacy review protocol](skills/collaborative-review/references/legacy-protocol.md) or [legacy research protocol](skills/collaborative-research/references/legacy-protocol.md) for those runs. Use a new run ID for `review.py`.
