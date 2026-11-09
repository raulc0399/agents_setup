# Prism delivery handover

Shared Prism and Codex implementation is complete at source commit **f476550**.
The earlier unfinished checkpoint is **bcdbfae**; its historical handover remains
available with `git show bcdbfae:features/prism/HANDOVER.md`.

## Current scope and usage

- Shared lists, diagrams, actions, questions, persistent tabs/splits and private
  view state are implemented in `features/prism/`.
- The supervised Codex launcher, native plugin, authenticated bridge and MCP are
  implemented. Use `prism --agent codex` in the desired project, then ask Codex
  to publish content in Prism. See [README.md](README.md).
- Claude Code is integrated through `src/adapters/claude.rs`, the native mod
  package `plugins/claude/` and launcher support for `prism --agent claude`.
  The original implementation prompt is in
  [docs/claude-handover.md](docs/claude-handover.md); the implementation
  record, verified CLI facts and session notes are in
  [docs/claude-continuation.md](docs/claude-continuation.md).
- Native Start/Steer/Queue remain disabled for both agents: Codex `/new` focus
  changes are not reliably observable, and Claude's prompt receipt carries no
  request-to-turn correlation. Explicit Share and same-conversation retrieval
  are the supported question workflow. There is no bypass flag.

## Reviewed implementation commits

| Commit | Work |
| --- | --- |
| `f913411` | Interactive diagrams, persistent views/tabs/splits, private UI state and schema 1→2 migration. |
| `09e4c4d` | Launcher, owned-process supervision, duplicate-resume rejection, cancellation and guarded tmux lifecycle. |
| `d771855` | Preserve already-shared questions through disconnect and resume. |
| `f476550` | Bridge/MCP, Codex integration, pagination, native evidence checks and absolute transport deadlines. |
| `cef55d2` | Claude Code launcher, lifecycle bridge glue, adapter, native mod package and tests. |

Feature authors and separate read-only reviewers resolved all reported findings.
Final source integration review approved f476550. Subsequent documentation
commits record installation, validation and the Claude handoff.

## Installation and verification

- Installed binary: `$HOME/.local/bin/prism`, version **0.1.0**.
- Installed/enabled native packages: **prism@prism-local 0.1.0** in Codex (marketplace
  source `features/prism/plugins/codex`) and in Claude Code (marketplace source
  `features/prism/plugins/claude`, which is both plugin and marketplace).
- Verified host: Debian 12 aarch64, tmux 3.3a, Codex CLI/app-server 0.167.0,
  Claude Code 2.1.319, Node 24.22.2, Rust 1.99.1. Linux `/proc` is required; no
  macOS claim.
- With the Claude integration: formatting, Clippy with warnings denied,
  **116 Rust tests**, **16 Python probe regressions**, **9 Codex lifecycle
  tests**, **4 Claude lifecycle tests** (real CLI, print mode, no login) and the
  release build pass. Commands and evidence are in [docs/validation.md](docs/validation.md).
- Actual isolated Claude CLI/model (installed plugin, auto permission mode):
  `/prism` pane → publication → accept/archive/undo → stable-ID revision →
  explicit Share → same-session retrieval/answer → `/clear` retirement with the
  CLI pane kept → exact resume → duplicate rejection → simultaneous sessions
  passed. Prism signalled no Claude process.
- Actual isolated Codex TUI/model: publication → right pane → selection/actions →
  explicit Share → same-thread retrieval/answer → stable-ID revision passed.
  Approvals stayed in Codex; observer connections never answered them.
- Installed binary: exact resume restored revision 7 and private drafts;
  duplicate resume was rejected. A new Share remained unchanged and retrievable
  after actual CLI exit and another exact resume.

## Preserve these boundaries

`.reviews/` remains untracked/excluded. `docs/claude-integration.md` and
`tests/probe_claude.py` were revalidated on Claude Code 2.1.319 and extended with
the interactive and end-to-end evidence; they are committed with the Claude
integration.

Keep project `.prism/state.db` and private drafts on upgrade/uninstall. Schema 2
migrates schema 1 transactionally; unknown schemas are refused. Runtime bindings
are separate from durable project/agent/conversation identity. Never guess an
unregistered CLI pane, reuse a retired generation, expose drafts through tools,
replay an unknown native send automatically, or weaken Codex's delivery gate.

Source changes require the same feature ownership, regression tests, narrow
validation and independent review documented in `AGENTS.md` and
`IMPLEMENTATION.md`. The temporary Python venv and compiler wrappers are host-local;
see `docs/development-setup.md` to recreate them.
