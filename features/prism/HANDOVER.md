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
- **Claude is not implemented here.** Its dedicated implementation prompt,
  shared integration points and validation contract are in
  [docs/claude-handover.md](docs/claude-handover.md).
- Native Codex Start/Steer/Queue remain disabled: immediate `/new` focus changes
  are not reliably observable. Explicit Share and same-conversation retrieval
  are the supported question workflow. There is no bypass flag.

## Reviewed implementation commits

| Commit | Work |
| --- | --- |
| `f913411` | Interactive diagrams, persistent views/tabs/splits, private UI state and schema 1→2 migration. |
| `09e4c4d` | Launcher, owned-process supervision, duplicate-resume rejection, cancellation and guarded tmux lifecycle. |
| `d771855` | Preserve already-shared questions through disconnect and resume. |
| `f476550` | Bridge/MCP, Codex integration, pagination, native evidence checks and absolute transport deadlines. |

Feature authors and separate read-only reviewers resolved all reported findings.
Final source integration review approved f476550. Subsequent documentation
commits record installation, validation and the Claude handoff.

## Installation and verification

- Installed binary: `$HOME/.local/bin/prism`, version **0.1.0**.
- Installed/enabled native package: **prism@prism-local 0.1.0**; marketplace source
  is this repository's `features/prism/plugins/codex` directory.
- Verified host: Debian 12 aarch64, tmux 3.3a, Codex CLI/app-server 0.167.0,
  Node 24.22.2, Rust 1.99.1. Linux `/proc` is required; no macOS claim.
- Formatting, Clippy with warnings denied, **100 Rust tests**, **16 Python
  probe regressions**, **9 release-binary lifecycle tests**, release build and
  installation pass. Commands and evidence are in [docs/validation.md](docs/validation.md).
- Actual isolated Codex TUI/model: publication → right pane → selection/actions →
  explicit Share → same-thread retrieval/answer → stable-ID revision passed.
  Approvals stayed in Codex; observer connections never answered them.
- Installed binary: exact resume restored revision 7 and private drafts;
  duplicate resume was rejected. A new Share remained unchanged and retrievable
  after actual CLI exit and another exact resume.

## Preserve these boundaries

`.reviews/`, `docs/claude-integration.md` and `tests/probe_claude.py` were present
before this continuation and remain untracked/excluded. They were not changed,
deleted or committed. The Claude report/probe are preliminary evidence, not an
implemented or authenticated interactive Claude integration.

Keep project `.prism/state.db` and private drafts on upgrade/uninstall. Schema 2
migrates schema 1 transactionally; unknown schemas are refused. Runtime bindings
are separate from durable project/agent/conversation identity. Never guess an
unregistered CLI pane, reuse a retired generation, expose drafts through tools,
replay an unknown native send automatically, or weaken Codex's delivery gate.

Source changes require the same feature ownership, regression tests, narrow
validation and independent review documented in `AGENTS.md` and
`IMPLEMENTATION.md`. The temporary Python venv and compiler wrappers are host-local;
see `docs/development-setup.md` to recreate them.
