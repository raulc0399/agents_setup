# Prism for Codex

Install the built `prism` binary on `PATH`, then register this directory using
the installed Codex CLI:

```sh
codex plugin marketplace add /absolute/path/to/features/prism/plugins/codex
codex plugin add prism@prism-local
```

The native compatibility manifest loads `skills/prism/SKILL.md` and `.mcp.json`.
The MCP process receives `PRISM_BINDING_FILE` only from the trusted launcher
environment; this path identifies a protected registration file, never a
model-provided credential. Installing the plugin in an existing unregistered
conversation does not authorize pane discovery: its tools return the explicit
launch/resume instruction until that binding is established.

The launcher owns the private app-server, exact CLI pane and binding generation.
The first server-injected MCP `_meta.threadId` completes a provisional binding;
an explicitly resumed thread ID must match. No hooks are required or trusted by
this package. User approvals remain in Codex.

Native question sending stays disabled until current-conversation lifecycle and
all required approval ownership gates pass. Explicit sharing and retrieval from
the same conversation remain available through the shared application.

Remove only the native package with `codex plugin remove prism@prism-local`.
Project `.prism/state.db` content is preserved.
