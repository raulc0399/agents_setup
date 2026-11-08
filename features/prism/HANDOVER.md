# Prism implementation handover

Recorded 2026-11-08. Work stopped at the user's request. This is a checkpoint of an unfinished implementation, not a release. The user explicitly requested committing the current work, including unfinished parts, before stopping.

## Scope and authority

- Repository: `/workspace/agents_setup`, branch `main`; application: `features/prism/`.
- Approved design: `features/prismapp_plan.codex.md` (plan commit `83bd14f`).
- User authorized implementation, dependency installation, one implementation agent per feature, independent feature reviews, and commits.
- **Shared Prism and Codex only. Do not implement Claude.** The user assigned Claude implementation to Claude separately. Do not request Claude authentication or modify its configuration.
- The primary workflow is talking to the coding CLI and asking it to open Prism in a right-hand **tmux pane**. `prism --agent codex` is the explicit launcher option. Native plugins remain inside their respective CLI; shared features belong in Prism.
- Follow `AGENTS.md`: small batches, explicit disjoint file ownership for agents, independent reviewers, narrow validation. Existing implementation authorization persists; clarify material scope changes. User-facing responses must be concise and end with a sunrise emoji.

## Current usability

**There is no `src/main.rs`, executable launcher, or complete end-to-end application yet.** Library features and a Codex plugin package exist. Do not tell the user they can install and use a finished Prism binary.

The checkpoint includes unfinished, unreviewed bridge/MCP code. Its compilation is verified, but compilation does not establish runtime correctness.

## Commit and file boundaries

- `59e801d Add Prism development setup checks` contains the independently reviewed development instructions and checker.
- The checkpoint commit containing this document saves shared application code, Codex code/plugin/probes, tests, and implementation tracking. Find its ID with `git log -1 --oneline`; the ID cannot be embedded in its own committed contents.
- `.reviews/` is pre-existing local review-coordinator state and is excluded from commits.
- `docs/claude-integration.md` and `tests/probe_claude.py` were created before the scope correction and remain untracked and excluded. Leave them untouched; no Claude adapter or plugin was implemented.
- Do not delete excluded files, uninstall Claude, or commit unrelated files without instruction.

## Implementation and review status

| Feature and concrete files | State |
| --- | --- |
| `docs/development-setup.md`, `scripts/check-development.sh` | Independently approved and committed in `59e801d`. |
| `src/model.rs`, `src/actions.rs`, `src/store.rs`, `tests/core.rs` | Implemented; latest atomic question/draft change independently re-reviewed and approved; 16 tests. |
| `src/session.rs`, `tests/session.rs` | Implemented; binding and tmux restart races fixed with regression tests; latest review approved; 12 tests. |
| `src/ui/mod.rs`, `src/ui/list.rs`, `tests/ui.rs` | List/question UI implemented; atomic send, long-history navigation, and status layout fixes independently approved; 15 tests. |
| `src/ui/diagram.rs`, `tests/diagram.rs` | Layout/rendering/hit detection implemented; 7 tests; independent review and interactive UI integration pending. |
| `tests/probe_codex.py`, `tests/probe_codex_test.py`, `docs/codex-integration.md` | Earlier corrected probe independently approved; latest command/switch/plugin extensions need regression rerun. |
| `src/adapters/mod.rs`, `src/adapters/codex.rs`, `tests/codex.rs`, `plugins/codex/` | Adapter and native plugin implemented; 8 library tests and 1 package test pass; independent implementation review pending. Probe approval is not adapter approval. |
| `src/bridge.rs`, `src/mcp.rs` | New primary-agent implementation; compiles; dedicated tests and independent review pending. |
| `src/main.rs`, `README.md` | Not created. Launcher, supervision, routing, end-user installation and usage remain. |
| Multiple views/tabs/splits | No manager implementation yet. Existing document cycling does not complete this feature. |

`IMPLEMENTATION.md` records file ownership. Assign a new feature agent for missing features and separate reviewers. All previous agents have finished or stopped; no agent should continue implementing during this handover.

## Validation already completed

The final combined-tree run of `cargo test --locked --all-targets --quiet` exited successfully:

- 8 library tests, 1 package integration test, 16 core tests, 7 diagram tests, 12 session tests, 15 UI tests: **59 Rust tests passed**.
- This aggregate run includes the Codex adapter author's two newest tests; older notes saying those were unrun are superseded.
- 10 Python probe regressions passed before the latest probe extensions; rerun those in the next session.
- Latest combined-tree formatting and Clippy have not been completed. Per-feature earlier checks do not replace final checks.
- No complete Prism end-to-end test, executable installation, or actual Prism PTY exit/signal restoration test has run.
- No further implementation or test run was performed to prepare this handover checkpoint.

## Container development setup

Debian 12, aarch64, user `node` (UID 1000), no sudo/root and no system C toolchain/headers. `$HOME` is `/home/agent`, a symlink to `/home/node`; do not change it.

Available: tmux 3.3a, Codex CLI 0.167.0, Node/npm/curl. Installed user-locally: Rust 1.99.1 with rustfmt/Clippy, Zig 0.17.1, cargo-zigbuild 0.24.1, and linker/archive wrappers. Rust is pinned by `rust-toolchain.toml`.

Working test environment:

```sh
cd /workspace/agents_setup/features/prism
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CC="$HOME/.local/bin/prism-zig-cc"
export AR="$HOME/.local/bin/prism-zig-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$HOME/.local/bin/prism-zig-cc"
cargo test --locked --all-targets
```

Python probes require `websockets==15.0.1` in a virtual environment; inspect their help/test entry points before running. Real-model probes can consume model usage and must use isolated conversations/configuration.

For a normal root-enabled Debian container, `apt-get update && apt-get install -y build-essential pkg-config` supplies a conventional compiler. SQLite is bundled. See `docs/development-setup.md` and run `scripts/check-development.sh` for the recorded setup details. The current process-birth verification uses Linux `/proc`; do not claim macOS support.

Claude CLI 2.1.319 was installed at `~/.local/bin/claude` before the scope correction. It is unauthenticated and not required for this scope.

## Shared core contracts

- Stable UUID document/element/relationship/group identities; workspace identity is canonical project + agent + exact native conversation, separate from live runtime identity.
- `Store` uses versioned JSON snapshots in SQLite, short immediate transactions, expected-revision conflicts, bounded busy handling, atomic publication replay records, and changed-payload rejection.
- Acceptance is tied to semantic revisions. Archive and conflict-aware undo preserve unrelated changes. Private view drafts, selections, and layout are not exposed to agent inspection.
- Frozen questions have explicit delivery mode/outcome, compare-and-swap transitions, and generation invalidation; no automatic resend.
- `Store::prepare_question_and_clear_draft` atomically records the question and clears only the matching view draft. A replay must not erase a newer draft.
- Canonical project path validation is the caller's responsibility. The shared `AgentKind::Claude` enum is not a Claude implementation.

## Session and pane contracts

- Call `RuntimeDirectory::create(project)` **before** allowing `Store` to create `.prism`; runtime directory ownership and mode 0700 are enforced. Binding files are private 0600.
- `LiveBinding` records project, exact conversation when known, server endpoint/PID/birth time, tmux socket/PID/birth time, CLI pane identity, optional Prism pane, secret, generation, and retirement.
- `LiveBinding::new` creates a provisional Codex binding. `bind_conversation` accepts the first/same exact ID; mismatch retires it. `workspace_key` rejects provisional/retired bindings.
- Runtime-directory inode locking serializes binding and pane changes. Reload authoritative state before writes; stale clones cannot erase a newer conversation or duplicate panes.
- `TmuxClient` uses same-connection `if-shell -F` guards for pane mutation/cleanup to protect against a replaced tmux server and reused pane IDs.
- `launch_cli` inside tmux creates a detached new window. The eventual launcher must select it rather than nesting `attach`.
- The expected UI command is `prism ui --binding <protected-path>`; it does not exist until `main.rs` is implemented. Never put the binding secret in argv/logs.

## UI and diagram integration

`ui::run(Store, Workspace, connection_callback)` owns terminal restoration and renders the list/question interface. The callback returns generation, connection status, offered modes, and a message.

Existing controls: arrows/jk focus; Space/click selection; A select visible; r clear; a accept; x archive; u undo; h/H hide/show; / filter; Tab/Shift-Tab cycle documents; e edit question; Enter newline; Ctrl-S/Ctrl-Enter send; Ctrl-N mode; [/ ] history pages; PgUp/PgDn scroll; ? help; q/Ctrl-C exit.

The diagram module is exported but not wired into this event/render loop. `DiagramLayout::new`, `visible_ids`, `position`, `focus_navigation`, and `DiagramState::{pan,reveal,hit_test,render}` are the integration points. Integrate the planned view-mode switch, navigation, panning, mouse targeting, visible selection, and shared actions/details. Layout has bounded three-lane nodes, selectable relationship cards, groups, private position overrides, Unicode handling, and large cyclic-graph tests.

## Codex native evidence and delivery gate

Verified in isolated real CLI/model probes:

- Private `codex app-server --listen unix://PATH`, WebSocket JSON-RPC, private socket, actual server version and process identity.
- Native MCP calls carry `_meta.threadId` and correlated native turn metadata. Neither MCP process environment nor `windowId` proves the caller's tmux pane; `windowId` is a thread-based identifier.
- The shipped plugin forwards `PRISM_BINDING_FILE` through its MCP configuration. Plugin installation was verified in a disposable Codex home, not installed as a finished tool in the user's environment.
- Real TUI MCP approval and command-execution approval remained in the CLI. Observer clients never answered them. Busy-turn steering was also observed.
- A fresh empty thread cannot reliably be resumed by a second client: observed `no rollout found`. Let a normal TUI create its thread, then bind from authenticated native MCP metadata. Explicit resume must match the supplied persisted conversation ID.

**Unresolved safety/correctness gate:** `/new` produces no immediate observer event before the next user input. Replacement `thread/started` can retire a binding later, but immediate visible-conversation switch detection remains unverified. Queue/cancellation/reconnect races are also not fully verified.

Therefore native automatic **Start/Steer/Queue stay disabled**. Do not add a bypass flag or claim supported exact-session delivery without evidence or an explicitly approved design change. The explicit Share mode records a question for retrieval by the CLI through Prism's tool; it is the current intended fallback, still needing complete-app validation.

`CodexAdapter::connect(socket, exact_thread, generation, timeout)` provides bounded RPC, native event filtering and retirement. It never answers approvals. `AgentAdapter` exposes capabilities, submit, poll_events, and retire. Independent adapter/plugin review is still required.

## Bridge/MCP unfinished work

`bridge.rs` currently provides a private Unix HTTP endpoint, a lease, protocol-version/generation/thread checks, token authentication, first-call conversation binding, Store operations, publication persistence independent of pane errors, and retirement/cleanup handling. `mcp.rs` provides bounded stdio JSON-RPC and `prism_publish`, `prism_inspect`, `prism_action`, `prism_questions`.

- **Neither has dedicated tests or independent review.** `tests/bridge.rs` and `tests/mcp.rs` do not exist.
- `connection_info` currently checks binding/tmux and socket existence; a stale socket after bridge death may falsely report connected. Validate actual service liveness.
- Review same-generation races, cleanup on every error path, request body deadlines, response bounds, token handling, and process ownership before relying on this service.
- MCP thread identity comes from native `_meta.threadId`, not caller tool arguments; nested native metadata must be consistent. Private drafts must remain inaccessible.
- Missing `PRISM_BINDING_FILE` produces an explicit launcher/resume instruction; never guess an existing unregistered CLI's pane.
- MCP protocol-version compatibility needs integration validation. No native adapter delivery worker is wired; native automatic sends are intentionally gated.

## Launcher implementation direction

Create the protected runtime directory, private app-server socket and binding-file path; forward `PRISM_BINDING_FILE` to native MCP. Launch the ordinary Codex TUI against that private server with `--remote unix://PATH`, optionally resuming only an explicitly identified persisted thread. Obtain and tag the verified tmux CLI pane, record the server birth identity and binding, then let the first authenticated MCP call bind the native conversation. The bridge currently has a bounded wait for the binding file.

Open Prism only when requested by a tool call. Clean up only owned processes/panes. Handle tmux selection/attachment appropriately. Avoid duplicate MCP registrations from both plugin and app-server overrides. An already-running unregistered CLI lacks a verified pane mapping; require explicit registration/resume rather than promising arbitrary zero-setup attachment.

## Remaining work packages

- Independently review Codex adapter/plugin and rerun regression coverage for the latest native probe extensions.
- Complete bridge/MCP tests, liveness and lifecycle fixes, and independent review.
- Implement `src/main.rs`: launcher, CLI routing, UI, MCP, bridge and owned-process supervision.
- Independently review diagrams and integrate them into the interactive UI.
- Implement multiple views/tabs/splits with shared content and separate private state; use a feature agent and independent reviewer.
- Run isolated real Codex publication-to-pane, actions, Share retrieval, restart/crash/privacy and PTY cleanup tests. Keep unsupported native sends disabled.
- Build/install the actual executable and Codex plugin; write end-user setup/use/removal and compatibility/storage instructions in `README.md`.
- Complete formatting, Clippy, full relevant tests, final independent integration review, update tracking, and commit subsequent reviewed work.

End users will need tmux, a supported authenticated Codex CLI, the built Prism executable on PATH and the Codex plugin installed. Exact supported versions and install commands must be confirmed against the completed executable. The package's documented plugin commands are `codex plugin marketplace add /absolute/path/to/features/prism/plugins/codex` and `codex plugin add prism@prism-local`; successful isolated package installation alone does not make the unfinished tool usable.

## New-session prompt

```text
Continue Prism in /workspace/agents_setup. Read AGENTS.md, features/prism/HANDOVER.md,
features/prism/IMPLEMENTATION.md, and features/prismapp_plan.codex.md first; inspect git status.
Implement shared Prism and Codex only. Do not implement Claude or modify Claude configuration.
The prior checkpoint is unfinished: no main.rs/launcher; bridge/MCP unreviewed; diagram UI
integration and multiple views missing. Use a feature agent per feature and a separate reviewer,
with explicit disjoint file ownership. Implementation, dependency installation, and commits
were already authorized. Keep Start/Steer/Queue disabled until the native conversation-switch
gate is resolved by evidence or an explicitly approved design change. Preserve .reviews/ and
the excluded Claude probe/report. Follow the handover's remaining work and build environment.
Finish installation and end-user documentation, validate, review, and commit completed work.
Keep updates short and end user-facing messages with 🌅.
```
