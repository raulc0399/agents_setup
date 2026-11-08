# Prism implementation

The user authorized implementation, dependency installation, feature agents, independent feature reviews, and commits. The user subsequently excluded Claude implementation: another Claude agent will implement that adapter/plugin. This work covers shared Prism features and Codex only. Agent-triggered opening is the primary workflow; the launcher is optional. The reviewed design is `../prismapp_plan.codex.md`.

## Batches and validation

- [ ] Batch 1: verify installed native contracts, caller identity, transport, lifecycle and approval ownership; record unsupported and untested modes explicitly.
- [ ] Batch 2: common model, versioned store, actions, delivery records and bridge/adapter contracts; validate conflicts, replay, archive/undo and private state.
- [ ] Batch 3: session/pane lifecycle, shared list/question UI, then separate native integrations; test each feature and obtain independent review.
- [ ] Batch 4: diagrams and selection; test layout, hit detection, resizing and shared actions; independent review.
- [ ] Batch 5: multiple views and persistence; test shared updates and independent view state; independent review.
- [ ] Batch 6: release checks, installation instructions, real supported CLI loops and final independent review; commit only reviewed files.

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

## Current implementation evidence

- Work stopped at the user's request; `HANDOVER.md` is the authoritative checkpoint and resume guide. The user explicitly requested committing unfinished work for handover; this checkpoint is not a release or final review approval.
- Development setup passed independent review and was committed as `59e801d`. Rust 1.99.1, rustfmt/clippy, Zig 0.17.1 and cargo-zigbuild 0.24.1 are installed user-locally.
- Shared core: 16 tests passed; latest atomic question/draft change independently re-reviewed and approved.
- Session feature: 12 tests passed; binding/pane race fixes independently re-reviewed and approved.
- List/question UI: 15 tests passed; latest fixes independently re-reviewed and approved.
- Diagrams: 7 tests passed; independent review and interactive integration pending.
- Codex adapter/plugin: 8 library tests and 1 package test passed; independent implementation review pending. Earlier corrected native probe approved; latest probe extensions need regression rerun. Immediate conversation-switch detection remains unverified, so automatic Start/Steer/Queue remain disabled.
- Final combined Rust test run passed all 59 tests. Latest global formatting/Clippy, bridge/MCP dedicated tests/review, launcher, multiple views, end-to-end installation and final integration review remain pending.
- Claude probe artifacts were created before the user excluded Claude implementation; no Claude adapter or plugin was implemented. Those probe artifacts are excluded from implementation commits.
