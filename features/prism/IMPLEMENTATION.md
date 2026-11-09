# Prism implementation

The user authorized implementation, dependency installation, feature agents, independent feature reviews, and commits. The user subsequently excluded Claude implementation: another Claude agent will implement that adapter/plugin. This work covers shared Prism features and Codex only. Agent-triggered pane opening is the primary workflow; Codex currently requires a launcher-established binding. The reviewed design is `../prismapp_plan.codex.md`.

## Batches and validation

- [x] Batch 1: verify installed Codex native contracts (Claude excluded; unavailable sends remain gated), caller identity, transport, lifecycle and approval ownership; record unsupported and untested modes explicitly.
- [x] Batch 2: common model, versioned store, actions, delivery records and bridge/adapter contracts; validate conflicts, replay, archive/undo and private state.
- [x] Batch 3: session/pane lifecycle, shared list/question UI, then separate native integrations; test each feature and obtain independent review.
- [x] Batch 4: diagrams and selection; test layout, hit detection, resizing and shared actions; independent review.
- [x] Batch 5: multiple views and persistence; test shared updates and independent view state; independent review.
- [x] Batch 6: release checks, installation instructions, real supported CLI loops and final independent review; commit only reviewed files.

## File ownership

Assignments are disjoint within each active batch. Reviewers are read-only; the owning implementation agent fixes findings. Shared entry points and dependency changes belong to the integrator.

| Assignment | Owned paths under this directory |
| --- | --- |
| Bridge/MCP integration (primary agent) | `src/bridge.rs`, `src/mcp.rs`, `tests/bridge.rs`, `tests/mcp.rs` |
| Integration | `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.gitignore`, `src/main.rs`, `src/lib.rs`, `README.md`, `IMPLEMENTATION.md` |
| Codex contract probe | `tests/probe_codex.py`, `docs/codex-integration.md` |
| Claude probe (completed before scope correction; no further edits) | `tests/probe_claude.py`, `docs/claude-integration.md` |
| Codex adapter/plugin feature | `src/adapters/mod.rs`, `src/adapters/codex.rs`, `plugins/codex/`, `tests/codex.rs`; existing Codex probes remain with this agent |
| Diagram feature | `src/ui/diagram.rs`, `tests/diagram.rs` |
| List/question UI feature | `src/ui/mod.rs`, `src/ui/list.rs`, `tests/ui.rs` |
| Session/pane feature | `src/session.rs`, `tests/session.rs` |
| Shared core feature | `src/model.rs`, `src/store.rs`, `src/actions.rs`, `tests/core.rs` |
| Development setup | `scripts/check-development.sh`, `docs/development-setup.md` |

Further feature ownership is recorded before those agents start. Native integration probes gate dependent native sends; they do not prevent implementing and testing the agent-neutral model and storage.

## Evidence

No application feature or live delivery is claimed complete until its checks pass. Credentials and authentication remain with each coding CLI. No secrets are committed. Local review coordinator artifacts under the repository's `.reviews/` are outside implementation commits.

## Checkpoint evidence (bcdbfae; superseded by resumed work)

- At checkpoint bcdbfae, work stopped at the user's request; the committed `HANDOVER.md` recorded the resume guide. The user explicitly requested committing unfinished work for handover; this checkpoint is not a release or final review approval.
- Development setup passed independent review and was committed as `59e801d`. Rust 1.99.1, rustfmt/clippy, Zig 0.17.1 and cargo-zigbuild 0.24.1 are installed user-locally.
- Shared core: 16 tests passed; latest atomic question/draft change independently re-reviewed and approved.
- Session feature: 12 tests passed; binding/pane race fixes independently re-reviewed and approved.
- List/question UI: 15 tests passed; latest fixes independently re-reviewed and approved.
- Diagrams: 7 tests passed; independent review and interactive integration pending.
- Codex adapter/plugin: 8 library tests and 1 package test passed; independent implementation review pending. Earlier corrected native probe approved; latest probe extensions need regression rerun. Immediate conversation-switch detection remains unverified, so automatic Start/Steer/Queue remain disabled.
- Final combined Rust test run passed all 59 tests. Latest global formatting/Clippy, bridge/MCP dedicated tests/review, launcher, multiple views, end-to-end installation and final integration review remain pending.
- Claude probe artifacts were created before the user excluded Claude implementation; no Claude adapter or plugin was implemented. Those probe artifacts are excluded from implementation commits.

## Resumed implementation (from bcdbfae)

Active ownership is disjoint: UI feature agent owns `src/ui/`, `tests/ui.rs`,
`tests/diagram.rs`; launcher feature agent owns `src/main.rs`, `src/session.rs`,
`tests/launcher.rs`, `tests/session.rs`, `tests/launcher_lifecycle.py`; bridge/Codex
feature agent owns `src/bridge.rs`, `src/mcp.rs`, `src/adapters/`, their tests,
Codex probes/docs and `plugins/codex/`. The primary integrator owns `src/store.rs`,
`tests/core.rs`, Cargo manifests, README and tracking. Reviewers are read-only.

- UI/diagrams/multiple views and Store migration independently approved after
  fixing UTF-8 cursor refresh and the diagram position limit; 23 UI, 7 diagram,
  and 19 core tests pass. Schema 1→2 preserves content and adds private UI state.
- Launcher/session independently approved after duplicate-resume, cancellation,
  socket-bound and guarded-attachment fixes; 9 lifecycle, 3 routing and 14 session
  tests pass, including actual tmux PTY terminal restoration.
- Bridge/MCP/Codex and shared transport helper independently approved after
  pagination, attribution and absolute-deadline fixes; 33 scoped Rust tests and
  16 Python regressions pass. Fresh isolated native probe passes.
- Final combined check: 100 Rust tests, formatting, Clippy and release build pass.
  Installed executable and native plugin 0.1.0; all 9 release lifecycle tests pass.
- Final source integration review approved f476550. Actual installed-binary
  exact resume, duplicate rejection, and Share preservation/retrieval after
  exit/resume passed. Native Start/Steer/Queue remain disabled.
- End-user instructions are in README.md; evidence is in docs/validation.md;
  the Claude implementation prompt is in docs/claude-handover.md.

## Claude integration (2026-11-08/09)

The user approved the plan in `docs/claude-continuation.md` ("go all"): Claude
launch with a pre-bound session ID, the CLI pane process as the owned native
process, bridge lifecycle reports from the native mod, Start/Steer/Queue
disabled for Claude, and an authenticated isolated real-model loop.

Ownership was disjoint: the shared integration agent owned `src/main.rs`,
`src/session.rs`, `src/bridge.rs`, `src/model.rs`, `src/adapters/mod.rs`,
`tests/bridge.rs`, `tests/core.rs`, `tests/launcher.rs`, `tests/session.rs`; the
Claude feature agent owned `src/adapters/claude.rs`, `plugins/claude/`,
`tests/claude.rs`, `tests/claude_lifecycle.py`, `tests/probe_claude.py`,
`docs/claude-integration.md`; the integrator owned the toolchain, README,
IMPLEMENTATION, HANDOVER, validation docs and the real loop. Two read-only
reviewers reviewed each feature; the integrator applied the shared-file fixes
after both feature agents were lost to container restarts.

- [x] Batch 1: toolchain reinstall, baseline green, probe rerun on 2.1.319,
  interactive contracts (`--session-id`, exact `--resume`, `/clear` lifecycle,
  installed-mod activation) verified in an isolated `CLAUDE_CONFIG_DIR`.
- [x] Batch 2: adapter, launcher/bridge glue, package and tests; 116 Rust,
  16 + 9 + 4 Python tests; independent reviews and fixes (lifecycle refused for
  Codex bindings before any persistence, retirement reason kept when the pane
  dies first, flat `Operation::Lifecycle(LifecycleEvent)` wire shape, reason
  restricted to lowercase identifiers, `--codex-home` requires `--codex`, pane
  survival asserted; mod reports `e.sessionId` at `session.end`, refuses
  subagent tool calls, decodes its module URL, shares the delivery fingerprint
  helper with Codex, skill directory `prism-workspace`, lifecycle test stops its
  supervisor before cleanup). Declined: removing `ClaudeAdapter::session_id`
  (mirrors `CodexAdapter::thread_id`); tmux stderr already masked in `session.rs`.
- [x] Batch 3: real authenticated Claude loop, documentation, final review,
  reviewed commits. Details in `docs/validation.md`.

Codex behavior is unchanged except that a native retirement observed through
the Codex adapter now records the adapter's reason instead of the generic
"bridge disconnected" when invalidating pending deliveries; Codex has no
enabled native deliveries, so no stored outcome changes in practice.
