# Prism for Codex

Install the built `prism` binary on `PATH`, then register this directory using
the installed Codex CLI:

```sh
codex plugin marketplace add /absolute/path/to/features/prism/plugins/codex
codex plugin add prism@prism-local
```

Start `codex` normally inside tmux and discuss your task. When needed, ask it to
show the plan in Prism. The first tool call opens Prism to the right of that same
conversation; later calls reuse the pane. No CLI restart or Prism launcher is needed.

The native compatibility manifest loads `skills/prism/SKILL.md` and `.mcp.json`.
The MCP process inherits `TMUX` and `TMUX_PANE`; Prism verifies process ancestry
against the exact pane before attaching. Native `_meta.threadId` identifies the
conversation. The MCP process owns the connection lifetime, not the user's CLI.
Outside tmux, tools ask you to start the CLI inside tmux. No hooks are required.
Existing launcher bindings remain compatible through `PRISM_BINDING_FILE`.

Native question sending stays disabled until current-conversation lifecycle and
all required approval ownership gates pass. Explicit sharing and retrieval from
the same conversation remain available through the shared application.

Remove only the native package with `codex plugin remove prism@prism-local`.
Project `.prism/state.db` content is preserved.
