# Claude implementation handover

> Superseded 2026-11-10 by batch 7 of `docs/app-plan.md`: the terminal UI, the
> tmux pane handling and the `prism --agent` launcher described here are removed;
> the Prism desktop app shows the workspace. Current state: `README.md`,
> `plugins/claude/README.md` and `docs/validation.md`.

> Status 2026-11-09: implemented. See `docs/claude-continuation.md` for the
> implementation record and `docs/validation.md` for the real Claude loop.

Shared Prism and Codex source baseline: **`f476550`**, following `f913411`,
`09e4c4d`, and `d771855`. Read the latest HEAD and repository status before work;
subsequent documentation commits do not change that source baseline.

## Paste into the Claude agent

```text
Implement Claude's native integration for Prism in /workspace/agents_setup.
Read AGENTS.md, features/prism/docs/claude-handover.md,
features/prism/HANDOVER.md, features/prism/IMPLEMENTATION.md, and
features/prismapp_plan.codex.md first; inspect git status and recent commits.

Shared Prism and Codex are implemented and independently reviewed at source
commit f476550. Implement Claude's adapter/plugin, launcher registration and
minimal shared integration glue; reuse the existing store, actions and UI.
Do not rewrite the shared product or change Codex's disabled native-send gate.

Use feature agents with disjoint file ownership and separate read-only reviewers.
Follow AGENTS.md's research, plan/approval, small-batch and regression process.
Coordinate ownership of shared entry points before editing them. Preserve
.reviews/ and unrelated files. Existing local Claude probe/report are preliminary
untracked artifacts: inspect and revalidate them before adopting or committing.

Deliver the real Claude publication -> Prism -> explicit Share -> same-session
retrieval -> visual revision loop, with correct identity, approvals, lifecycle,
privacy and restart behavior. Implement automatic Claude submission only after
its native session-switch, cancellation and request-to-turn correlation gates
pass; otherwise keep those modes disabled and document Share as the fallback.
Never invent receipt semantics, bypass authentication/trust, or guess a pane.

Validate both Claude and all existing shared/Codex behavior. Finish independent
reviews, installation/use/removal instructions, tracking and reviewed commits.
Keep updates concise and end responses with a sunrise emoji.
```

## Existing product to reuse

| Artifact | Contract |
| --- | --- |
| `src/model.rs` | Stable document/element/relationship/group UUIDs; `WorkspaceKey` is canonical project + `AgentKind` + exact native conversation. `AgentKind::Claude` already exists, but is not an implementation. |
| `src/actions.rs`, `src/store.rs` | Versioned SQLite, expected-revision conflicts, idempotent publication replay, acceptance, archive/conflict-aware undo, private views/drafts, frozen question snapshots. |
| `src/ui/` | Shared list/diagram UI, keyboard/mouse actions, private question editor, persistent tabs/splits. Do not fork a Claude UI. |
| `src/adapters/mod.rs` | `AgentAdapter::{capabilities, submit, poll_events, retire}`, `Capabilities`, `NativeEvent`. Keep native operations inside the Claude adapter/mod. |
| `src/session.rs` | Protected runtime/binding files; process birth identity; guarded tmux pane creation/reuse, selection, attachment and cleanup. |
| `src/bridge.rs` | Authenticated Unix HTTP, protocol/generation checks, shared operations, actual health checks, bounded requests/responses and paginated inspection. Native lifecycle routing is currently Codex-specific. |
| `src/mcp.rs` | Codex stdio MCP schemas and strict `_meta.threadId` extraction. Reuse semantic operations/schema definitions as appropriate, not Codex caller proof for Claude. |
| `src/io.rs` | Crate-private absolute-deadline Unix writes; handles partial progress and slow readers. |

The native plugin belongs in Claude; shared rendering, storage and content actions
belong in Prism. No tracker-specific lifecycle or automatic chat extraction is
required. The primary workflow is asking the CLI to open Prism to its right.

## Required integration points: these are not already generic

- `src/main.rs::Agent` accepts only Codex. `launch`, `supervise` and `native_cli`
  create a Codex private app-server/TUI pair. Add a verified Claude launch/explicit
  resume path; do not pretend Claude has Codex's `--remote` app-server protocol.
- `src/main.rs::run_ui` currently sets `WorkspaceKey.agent` to Codex. Use the
  authenticated binding's agent when enabling Claude, including disconnected
  workspace reopening. Test identical native IDs under different agents.
- `src/session.rs::LiveBinding::new` sets `AgentKind::Codex` and records a native
  server PID/endpoint. Define Claude's owned CLI/mod process identity explicitly;
  do not pass arbitrary PIDs just to satisfy existing fields.
- `src/bridge.rs::poll_native` constructs `CodexAdapter`; `run` also terminates
  the binding's owned native server on cleanup. Dispatch lifecycle and cleanup
  by verified agent/process ownership. Never terminate an unrelated or merely
  discovered Claude process.
- `src/bridge.rs::authorize` currently validates canonical UUID `thread_id`.
  Establish the actual Claude identity contract before retaining/generalizing
  this wire field. The caller cannot override agent/project/pane through tool
  arguments. `connection_info` and error guidance currently mention Codex.
- `src/mcp.rs::native_thread` is Codex-specific. The Claude native mod should
  obtain `$.session.id()` itself and use its authenticated launcher channel;
  never synthesize Codex `_meta` from model-supplied arguments.

Prefer the smallest agent dispatch necessary in these shared entry points. Keep
Codex tests passing and its private app-server, approvals and launch behavior
unchanged. Do not introduce parallel copies of core operations.

## Proposed file ownership for the Claude work

Assign actual agents after inspection; do not let two agents edit one file.

| Owner | Exclusive paths under `features/prism/` |
| --- | --- |
| Claude native feature | New `src/adapters/claude.rs`, `plugins/claude/`, `tests/claude.rs`, Claude-specific test files; existing `tests/probe_claude.py` and `docs/claude-integration.md` only after reviewing their provenance. |
| Shared integration owner | `src/main.rs`, `src/session.rs`, `src/bridge.rs`, `src/adapters/mod.rs`, `src/lib.rs`, corresponding shared tests and any necessary Cargo changes. |
| Documentation/integration owner | `README.md`, `IMPLEMENTATION.md`, `HANDOVER.md`, `docs/validation.md`; coordinate any reassignment before edits. |
| Independent reviewers | Read-only; feature authors fix findings in their owned files. |

The store/UI are already implemented. If a Claude feature needs changes there,
report the need and assign ownership before editing. Do not delete existing
files or remove comments without the permission required by `AGENTS.md`.

## Preliminary Claude evidence: revalidate before implementation

`docs/claude-integration.md` and `tests/probe_claude.py` remain **untracked** and
were excluded from Codex commits at the user's request. They may exist only in
this working directory. Do not overwrite, delete or blindly stage them.

The report records Claude CLI/mod 2.1.319 on Linux arm64, with isolated,
unauthenticated print-mode checks for native registration, exact explicit session
identity, user-only Unix HTTP and a narrow exit event. It does **not** prove an
interactive authenticated loop, approval ownership, submission or switch races.
The CLI was installed before scope was separated; verify current version/auth
state without exposing credentials or altering existing conversations.

Recorded package shape: `.claude-plugin/plugin.json`, `hooks/hooks.json` with a
`modules` array, and an ES module exporting `register(on, options)`. Recorded APIs
include `$.tool.register`, `$.command.register`, `$.session.id`, `$.env.get`, and
`$.http.fetch(..., {socketPath, ...})`. Its response `text` was a string, not a
method. Read the installed native declarations and validate these observations;
the API was marked EARLY ACCESS. A cached `/tmp` declaration path is not a shipped
dependency or a stable interface guarantee.

The recorded `$.prompt.submit({text})` receipt lacked an authoritative request ID,
queued/started discriminator and turn ID; resolution could occur after queueing.
Do not mark Started from that receipt or match the next turn by timing alone.
Clear/resume/branch/logout, mod reload and queue cancellation need runtime checks.

## Non-negotiable delivery and privacy contracts

- Create `RuntimeDirectory` before opening the project store. Keep directories
  0700, binding files/sockets 0600, and secrets out of argv, logs and tool payloads.
- Exact native session, agent, project, owned process, tmux server birth, pane
  identity and generation must agree. A matching project or inherited tmux pane
  variable alone is insufficient. Unregistered sessions get an explicit
  launch/resume instruction; never guess a pane or silently fork a conversation.
- Keep native execution approvals and trust decisions in Claude. Prism acceptance
  is content feedback, never permission to run tools or implement changes.
- Share is durable exposure of a frozen question/selection, not an automatic
  model turn. `Store::invalidate_generation` deliberately preserves Share
  records; retirement must not relabel a successful Share as failed.
- Persist native sends before transport; recheck exact identity before dispatch.
  Distinguish Pending, Submitted/Queued, Started, Failed/Refused and Unknown.
  Never resend Unknown automatically or redirect old-generation work after a switch.
- Private views, filters, selections, positions and drafts stay out of agent
  inspection. Only explicit Share/send exposes its frozen snapshot. This is tool
  exposure control, not isolation from another same-user filesystem process.
- Publication commits before optional pane opening. A pane failure returns the
  committed result plus error; replay must not create another revision.
- Summary inspection uses UUID cursor pagination and `next_cursor`; large question
  text is a marked preview, fetched fully by request ID. Preserve bounds and
  absolute deadlines, including connection establishment and partial writes.
- Codex Start/Steer/Queue remain disabled. Passing Claude gates enables only the
  verified Claude modes; it does not clear Codex's separate `/new` visibility gate.

## Validation and delivery

Current baseline: **100 Rust tests, 16 Codex Python regressions, 9 release-binary
lifecycle tests**, formatting, Clippy and release build pass. A real Codex model
loop and installed-binary explicit resume/Share preservation also pass. See
`docs/validation.md`; keep this baseline green while adding Claude checks.

```sh
cd /workspace/agents_setup/features/prism
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CC="$HOME/.local/bin/prism-zig-cc"
export AR="$HOME/.local/bin/prism-zig-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$HOME/.local/bin/prism-zig-cc"
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release
/tmp/prism-resume-venv/bin/python -m unittest discover -s tests -p probe_codex_test.py
PRISM_BINARY=target/release/prism /tmp/prism-resume-venv/bin/python tests/launcher_lifecycle.py
```

The temporary Python environment is host-local; recreate it with Python venv and
`websockets==15.0.1` if missing. A conventional Linux C toolchain can replace the
Zig environment; see `docs/development-setup.md`. Do not claim macOS support:
process birth checks use Linux `/proc`.

Add isolated Claude tests for native publication, actions, Share retrieval,
same-session revision, exact resume, separate simultaneous sessions, private
state, CLI/mod crash/reload, switches, cancellation, pane reuse, tmux restart,
and terminal restoration. Use Claude's own supported package validation and
installation commands after inspecting the installed CLI. Real-model tests must
use isolated conversations/configuration; request authentication only when
required and never bypass it.

Update installation/use/removal and compatibility instructions, obtain independent
feature and final integration approvals, and commit only reviewed in-scope files.
Preserve `.reviews/`, persisted user content, unrelated changes and Codex behavior.
