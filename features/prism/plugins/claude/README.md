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
`mcp__prism__publish`, `mcp__prism__inspect`, `mcp__prism__action`,
`mcp__prism__questions` and `mcp__prism__answer` plus the `/prism` command, which
reports the published documents and the Prism app connection without a model
turn. Once a conversation is
attached, the mod polls shared questions every second and appends each new one
to the prompt box as `Item N: <title>`, quoted lines and a `[prism:<request id>]`
marker; the user types the question after it and sends it. The skill tells
Claude to keep answers in the conversation and save them to the md files when
relevant; for now it does not call `mcp__prism__answer`, and the app changes only
when Claude publishes the revised item.

Start `claude` normally and discuss your task. When needed, use `/prism` or ask
Claude to show the plan in Prism. The mod reads the native session ID and
attaches through `prism attach`, which identifies the Claude process by its
process ID and start time. The bridge connects to the Prism desktop app, which
shows the conversation in a tab; later calls reuse the attachment, without
restarting Claude. `/prism` reports whether the link to the app is connecting,
connected, retrying (since when, and whether the pairing file was found) or
superseded by a later session; a superseded session shows in the app again after
`/exit` and `claude -c`. After updating Prism, restart running Claude sessions
the same way, because the mod loads only at session start.

The protected binding authenticates subsequent calls. `/clear`, `/resume`,
`/logout` and exit retire it through `session.end`; a later session attaches on
demand. Prism never terminates the attached CLI. User approvals remain in Claude.

Native question sending stays disabled because Claude's prompt receipt carries
no request-to-turn correlation. Explicit sharing in Prism and retrieval with
`mcp__prism__questions` from the same conversation remain available.

Remove only the native package with `claude plugin uninstall prism@prism-local`
and `claude plugin marketplace remove prism-local`. Project `.prism/state.db`
content is preserved.
