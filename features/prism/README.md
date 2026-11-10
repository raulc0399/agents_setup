# Prism

Prism 0.1.0 is a project-local visual workspace beside an existing Codex or
Claude Code conversation. It provides lists, diagrams, acceptance, archive/undo,
private questions, and multiple views of shared content. Both CLIs share one
bridge application; each has its own native package under `plugins/`. The Prism
desktop app (`app/`, see [the app plan](docs/app-plan.md); build and install: [app/README.md](app/README.md)) runs on the host and
shows each conversation in a tab.

## Requirements

- Linux with `/proc` for the CLI, its plugin and the bridge; the desktop app on
  macOS or Windows reachable as `host.docker.internal` (see the app plan).
- For Codex: authenticated Codex CLI with native plugins and Unix app-server
  connections. The tested native contract is **Codex CLI/app-server 0.167.0**.
- For Claude: authenticated Claude Code CLI with native mods (hooks modules).
  The tested native contract is **Claude Code 2.1.319**; its mod API is labelled
  early access by Anthropic.
- Tested on Debian 12 aarch64, Node 24.22.2. Other versions/platforms are not
  certified.
- To build: Rust **1.99.1**, a C compiler/linker and libc headers. SQLite is
  bundled. See [development setup](docs/development-setup.md) for this
  container's user-local Zig compiler setup.

Codex labels the app-server interface experimental. Prism keeps authentication,
models, approvals and permission policy in each CLI; it never answers native
approval requests. Native **Start, Steer and Queue are disabled** for both
agents: Codex because immediate conversation-switch detection remains
unverified, Claude because its prompt receipt carries no request-to-turn
correlation. The supported question workflow is explicit **Share**, followed by
retrieval in the same CLI conversation.

## Build and install

From `features/prism/`, with the development prerequisites on `PATH`:

```sh
cargo test --locked --all-targets
cargo build --locked --release
cargo install --locked --path . --root "$HOME/.local"
export PATH="$HOME/.local/bin:$PATH"
codex plugin marketplace add "$PWD/plugins/codex"
codex plugin add prism@prism-local
prism --version
codex plugin list --marketplace prism-local --json
```

Keep the local marketplace source directory available. The native package uses
Codex's compatibility manifest and bundles the `prism` skill and MCP tools.
It requires no hooks, hook trust, or manual duplicate MCP registration. Check
that `prism@prism-local` is installed and enabled. Plugin configuration follows
[official Codex packaging guidance](https://developers.openai.com/plugins/build/plugins);
commands above are verified against the installed 0.167.0 CLI.

For Claude Code, the `plugins/claude` directory is both the plugin and its local
marketplace:

```sh
claude plugin validate "$PWD/plugins/claude"
claude plugin marketplace add "$PWD/plugins/claude"
claude plugin install prism@prism-local
claude plugin list
```

The package bundles the native mod `hooks/prism.js` (tools
`mcp__prism__publish`, `mcp__prism__inspect`, `mcp__prism__action`,
`mcp__prism__questions`, `mcp__prism__answer` and the `/prism` command) and the
`prism-workspace` skill. The mod polls shared questions every second and places
new ones in the prompt box. Installed mods activate in new sessions without `--plugin-dir`. See
[plugins/claude/README.md](plugins/claude/README.md) and
[Claude integration evidence](docs/claude-integration.md).

## Open Prism from a conversation

Start the Prism desktop app on the host, then run your normal CLI from the
project:

```sh
cd /path/to/project
codex
# Or: claude
```

Discuss your task normally. When needed, ask “show this plan in Prism”. The
native plugin attaches to the current conversation and starts its bridge, which
connects to the app; the app shows the conversation in a tab. Later calls reuse
the attachment. Claude also provides `/prism`, which reports the published
documents and the app connection (connecting, connected, retrying since when
and whether the pairing file was found, or superseded by a later session) without a model
turn. The original CLI keeps running untouched throughout.

Install the plugin before starting the CLI so its native tools are loaded. No
conversation restart is required to open Prism from an already-running CLI
with the plugin loaded. Without the app the tools keep working; the bridge
retries quietly until the app runs. After updating Prism, restart running Claude
sessions (`/exit`, then `claude -c`): the mod loads only at session start, and an
older mod does not match a newer `prism` binary.

The binding identifies the CLI by its process ID and start time, never by a
model-provided value. Codex supplies the native thread ID through MCP metadata;
Claude reads its own session ID in the mod. The application keeps sockets in
private `/tmp` directories and binds all content to that exact project, agent
and conversation. The app finds the bridge through the pairing file it writes to
`~/.claude/prism/app.json` and `~/.codex/prism/app.json`; `PRISM_APP_FILE`
overrides the path.

## Work with content

Ask the CLI to publish a list or diagram. It supplies stable content IDs; revisions
reuse those IDs. Accepting content records feedback on that content revision,
not permission for implementation. Semantic edits make earlier acceptance stale.
Archive affects every view; hide affects the current view. Undo rejects edits
that conflict with later changes instead of overwriting them. Elements appear
in publication order; reordering them in a later publication is not a semantic
edit.

In the app, the list shows one card per item; the focused card expands with its
text, its questions and the actions ask, hide and drop. **Ask** shares the focused
item, or the text selected inside it, as a quoted reference. With Claude Code,
the native mod places it in the CLI prompt box within a second as
`Item 2: <title>` plus `> quoted lines` and a `[prism:<request id>]` marker; you
type your question after it and press Enter there. Claude records its reply
with `mcp__prism__answer`, and the card shows it. Codex has no prompt box
access: ask it to retrieve the shared question with `prism_questions`. Share does
not start a model turn or prove an answer began. **Drop** is the shared archive;
**hide** and the filter only affect that tab. The diagram draws groups as lanes,
elements as boxes, relationships as arrows with numbered pills, `manual: true`
relationships dashed and gate findings as amber badges; it pans, zooms and
exports SVG. The app's status bar lists the keys of the current view. Hidden
items, filters, zoom and selection live in the app's own data directory and are
excluded from the Prism tools.

## Storage, upgrades and recovery

Each canonical project stores content in `.prism/state.db` (SQLite WAL).
Workspace identity includes the project, agent and exact conversation UUID.
New conversations receive separate content. Private runtime directories under
`.prism/run/` contain bindings and logs. Sockets live in `/tmp/prism-<runtime-id>/`
and are removed when the bridge ends. Directory mode is 0700 and binding mode is 0600. Tokens never belong in command arguments or published
content. Do not share binding files or runtime logs as public diagnostics.

Schema 2 adds private UI arrangement with an in-place transactional migration
from schema 1; content, questions and views are preserved. The terminal UI's
view tables stay in schema 2 unused, so older `prism` binaries still open the
store. Unknown newer schemas are refused. Stop live Prism processes before copying `.prism/` for a backup;
include SQLite's WAL/SHM files if copying a live database. Prism writes
`.prism/.gitignore` containing `*`, so the directory stays out of version
control in every project; an existing file there is never overwritten.

Closing the native connection retires the binding and the app marks the tab
ended; the attached CLI is never terminated. Codex attachment follows its MCP
process lifetime; it does not observe app-server thread events. Claude reports
session changes through `session.end`. A different live Codex thread cannot
reuse the same MCP process's binding. Persisted content remains available, and
unknown question outcomes are never resent automatically.

If a tool response is lost after publication, retry the identical publication
with the same request ID; it replays the result without duplicating the
revision. Changed payloads need a fresh request ID. Socket paths under `/tmp` do
not grow with the project path.

Diagnostics: `prism --help`, `codex --version`, `claude --version`,
`codex plugin list --marketplace prism-local --json`, `claude plugin list`, `/prism`
in Claude, and the specific binding's `bridge.log`. An unavailable bridge
disables Share even if a stale socket remains. Do not delete the project
database to repair a runtime connection.

## Remove

Exit the CLI session, then run the commands for the packages you installed:

```sh
codex plugin remove prism@prism-local
codex plugin marketplace remove prism-local
claude plugin uninstall prism@prism-local
claude plugin marketplace remove prism-local
cargo uninstall --root "$HOME/.local" prism
```

This preserves `.prism/state.db` and saved content. Removing persisted content
is a separate explicit action; Prism does not remove it during uninstall.

## Development and dependency licenses

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
python3 -m unittest discover -s tests -p probe_codex_test.py
PRISM_BINARY=target/debug/prism python3 tests/attachment.py
```

Completed checks and the real Codex and Claude publication/Share/revision loops
are recorded in [validation evidence](docs/validation.md).

Python native probes additionally require `websockets==15.0.1` in a virtual
environment. Real-model probes use isolated Codex homes and can consume model
usage; their options are documented in [Codex integration evidence](docs/codex-integration.md).
`tests/attachment.py` and `tests/probe_claude.py` use the installed
`claude` CLI in print mode with an isolated `CLAUDE_CONFIG_DIR`, need no login
and make no model request.

Dependency versions are pinned in `Cargo.lock`. rusqlite and libsqlite3-sys
declare MIT; the other direct Rust dependencies declare MIT and/or
Apache-2.0. Bundled SQLite is public domain. For all resolved dependencies,
including target-specific transitive crates, inspect their `license`,
`license_file`, and `manifest_path` with `cargo metadata --locked --format-version 1`;
the source packages contain the applicable license texts and notices. Preserve
those notices when redistributing a binary. No repository-wide application
license is inferred from dependency licenses.
