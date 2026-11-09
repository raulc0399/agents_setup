# Prism for Claude Code

Install the built `prism` binary on `PATH`, then register this directory using
the installed Claude Code CLI (verified with 2.1.319). The directory is both the
plugin and its local marketplace: `.claude-plugin/marketplace.json` lists the
plugin with source `./`.

```sh
claude plugin validate /absolute/path/to/features/prism/plugins/claude
claude plugin marketplace add /absolute/path/to/features/prism/plugins/claude
claude plugin install prism@prism-local
claude plugin list
```

The installed plugin is read from this directory, so keep it available. It
bundles the native mod `hooks/prism.js` (tool schemas in `hooks/tools.json`) and
the `skills/prism-workspace/SKILL.md` instructions. The skill directory and
frontmatter name are `prism-workspace`: a skill named `prism` would claim
`/prism:prism` and the CLI would then refuse the mod's `/prism` command. Prism
tools refuse calls from subagents; only the main conversation publishes. The mod registers the tools
`mcp__prism__publish`, `mcp__prism__inspect`, `mcp__prism__action` and
`mcp__prism__questions` plus the `/prism` command, which opens or refreshes the
Prism pane without a model turn.

The mod receives `PRISM_BINDING_FILE` only from the trusted launcher
environment; this path identifies a protected registration file, never a
model-provided credential. Caller identity is the current Claude session ID,
read by the mod itself and verified by the authenticated Unix bridge. Running
the plugin in an unregistered conversation does not authorize pane discovery:
its tools and `/prism` return the explicit launch/resume instruction until the
binding is established.

Start or explicitly resume a registered conversation with the launcher:

```sh
cd /path/to/project
prism --agent claude
prism --agent claude --resume SESSION_UUID
```

The launcher owns the exact CLI pane and binding generation and pre-binds the
session ID it passes to `claude`. `/clear`, `/resume`, `/logout` and CLI exit are
reported by the mod's `session.end` hook and retire the binding; relaunch or
resume explicitly afterwards. User approvals remain in Claude.

Native question sending stays disabled because Claude's prompt receipt carries
no request-to-turn correlation. Explicit sharing in Prism and retrieval with
`mcp__prism__questions` from the same conversation remain available.

Remove only the native package with `claude plugin uninstall prism@prism-local`
and `claude plugin marketplace remove prism-local`. Project `.prism/state.db`
content is preserved.
