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
opens or refreshes the Prism pane without a model turn. Once a conversation is
attached, the mod polls shared questions every second and appends each new one
to the prompt box as `Item N: <title>`, quoted lines and a `[prism:<request id>]`
marker; the user sends it. The skill tells Claude to answer in the conversation
and record the reply with `mcp__prism__answer`.

Start `claude` normally inside tmux and discuss your task. When needed, use
`/prism` or ask Claude to show the plan in Prism. The mod reads the native session
ID and attaches through `prism attach`; process ancestry verifies the exact tmux
pane. Prism opens to the right and later calls reuse that pane, without restarting
Claude. Outside tmux, the mod asks you to start the CLI inside tmux.

The protected binding authenticates subsequent calls. `/clear`, `/resume`,
`/logout` and exit retire it through `session.end`; a later session attaches on
demand. Prism never terminates the attached CLI. Existing launcher bindings
remain compatible through `PRISM_BINDING_FILE`. User approvals remain in Claude.

Native question sending stays disabled because Claude's prompt receipt carries
no request-to-turn correlation. Explicit sharing in Prism and retrieval with
`mcp__prism__questions` from the same conversation remain available.

Remove only the native package with `claude plugin uninstall prism@prism-local`
and `claude plugin marketplace remove prism-local`. Project `.prism/state.db`
content is preserved.
