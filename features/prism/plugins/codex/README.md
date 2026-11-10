# Prism for Codex

Install the built `prism` binary on `PATH`, then register this directory using
the installed Codex CLI:

```sh
codex plugin marketplace add /absolute/path/to/features/prism/plugins/codex
codex plugin add prism@prism-local
```

Start `codex` normally and discuss your task. When needed, ask it to show the
plan in Prism. The first tool call attaches that same conversation, and the Prism
desktop app shows it in a tab; later calls reuse the attachment. No CLI restart
is needed.

The native compatibility manifest loads `skills/prism/SKILL.md` and `.mcp.json`.
The binding records the MCP process by its process ID and start time. Native
`_meta.threadId` identifies the conversation. The MCP process owns the connection
lifetime, not the user's CLI. No hooks are required.

Native question sending stays disabled until current-conversation lifecycle and
all required approval ownership gates pass. Explicit sharing and retrieval from
the same conversation remain available through the shared application.

Remove only the native package with `codex plugin remove prism@prism-local`.
Project `.prism/state.db` content is preserved.
