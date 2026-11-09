# Prism

Prism 0.1.0 is a project-local visual workspace beside an existing Codex or
Claude Code conversation in tmux. It provides lists, diagrams, acceptance,
archive/undo, private questions, and multiple views of shared content. Both CLIs
share one application; each has its own native package under `plugins/`.

## Requirements

- Linux with `/proc`, tmux, and a terminal supporting UTF-8 and alternate screens.
- For Codex: authenticated Codex CLI with native plugins and Unix app-server
  connections. The tested native contract is **Codex CLI/app-server 0.167.0**.
- For Claude: authenticated Claude Code CLI with native mods (hooks modules).
  The tested native contract is **Claude Code 2.1.319**; its mod API is labelled
  early access by Anthropic.
- Tested on **tmux 3.3a**, Debian 12 aarch64, Node 24.22.2. Other
  versions/platforms are not certified.
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
`mcp__prism__questions` and the `/prism` command) and the `prism-workspace`
skill. Installed mods activate in new sessions without `--plugin-dir`. See
[plugins/claude/README.md](plugins/claude/README.md) and
[Claude integration evidence](docs/claude-integration.md).

## Open Prism from a conversation

Start a tmux session, then run your normal CLI from the project:

```sh
cd /path/to/project
tmux
codex
# Or: claude
```

Discuss your task normally. When needed, ask “show this plan in Prism”. The
native plugin attaches to the current conversation and opens Prism in a pane to
the right. Later calls reuse it; closing the Prism pane lets the next call reopen
it. Claude also provides `/prism` to open the pane without a model turn.
The original CLI stays running in its original pane throughout.

Outside tmux, Prism asks you to start the CLI inside tmux; it cannot split an
ordinary terminal. Install the plugin before starting the CLI so its native
tools are loaded. No `prism --agent ...` launcher or conversation restart is
required to open Prism from an already-running CLI with the plugin loaded.

Pane routing uses inherited tmux identity plus verified process ancestry, never
a model-provided pane or a recently active terminal. Codex supplies the native
thread ID through MCP metadata; Claude reads its own session ID in the mod.
The application keeps sockets in private `/tmp` directories and binds all content
to that exact project, agent and conversation.

The older explicit launcher remains available for existing integrations. Its
bindings are still accepted, but the native plugins no longer require it.

## Work with content

Ask the CLI to publish a list or diagram. It supplies stable content IDs; revisions
reuse those IDs. Accepting content records feedback on that content revision,
not permission for implementation. Semantic edits make earlier acceptance stale.
Archive affects every view; hide affects the current view. Undo rejects edits
that conflict with later changes instead of overwriting them. Elements appear
in publication order; reordering them in a later publication is not a semantic
edit.

Select content, press `e`, write a question, then press `Ctrl-S` to **Share**.
Return to the CLI and ask it to retrieve your shared Prism question. It can
read the frozen selection and question, answer in that conversation, and revise
the visual content. If the content changed since you selected it, the editor
reports a stale selection and keeps your draft; reselect and share again. Share does not start a model turn or prove an answer began.
Unsent selections and drafts are excluded from the Prism tools. This is tool
exposure control; another process running as your user can read local files.

## Controls

| Key | Action |
| --- | --- |
| Arrows / `j`, `k` | Focus content |
| Space / click | Toggle selection |
| `A` / `r` | Select visible content / clear selection |
| `a` / `x` / `u` | Accept / archive / undo |
| `h` / `H` | Hide selected content / show hidden content |
| `/` | Edit current view's filter |
| `v` | Switch list/diagram |
| `Tab` / `Shift-Tab` | Change view tab |
| `Ctrl-T` / `Ctrl-W` / `Ctrl-R` | Duplicate view / close tab / reopen closed tab (at the view limit it swaps with the active tab) |
| `Ctrl-\` (sent as `Ctrl-4` by most terminals) / `F6` | Toggle split / focus other split |
| `Shift` + arrows / `Alt` + arrows | Pan diagram / move focused diagram card |
| `e` / `Esc` | Edit question / leave editor |
| `Enter` | Newline in question |
| `Ctrl-S` | Explicitly share question |
| `Ctrl-N` | Cycle offered delivery modes (currently Share only) |
| `[` / `]`, `PgUp` / `PgDn` | History pages / detail scrolling |
| `?` / `q` / `Ctrl-C` | Help / exit UI |

At most 32 tabs can be open in one UI. Views keep independent selections, drafts, filters and layout while sharing
content and acceptance. Closing a tab never archives content. Opening the same
conversation restores its saved views. The `?` screen lists current controls.

## Storage, upgrades and recovery

Each canonical project stores content in `.prism/state.db` (SQLite WAL).
Workspace identity includes the project, agent and exact conversation UUID.
New conversations receive separate content. Private runtime directories under
`.prism/run/` contain bindings and logs. Sockets live in `/tmp/prism-<launch-id>/`
and are removed on normal supervisor exit; abandoned socket directories are pruned
on later launches for that project. Directory mode is 0700 and binding mode is 0600. Tokens never belong in command arguments or published
content. Do not share binding files or runtime logs as public diagnostics.

Schema 2 adds private UI arrangement with an in-place transactional migration
from schema 1; content, questions and views are preserved. Unknown newer schemas
are refused. Stop live Prism processes before copying `.prism/` for a backup;
include SQLite's WAL/SHM files if copying a live database. Keep `.prism/` out of
version control.

Closing the native connection retires the binding and releases the Prism pane;
the attached CLI is never terminated. Codex attachment follows its MCP process
lifetime; it does not observe app-server thread events. Claude reports session
changes through `session.end`. A different live Codex thread cannot reuse the
same pane's binding. Persisted content remains available, and unknown question
outcomes are never resent automatically.

If pane creation fails after publication, the tool returns the committed revision
and pane error separately. Retry the identical publication with the same request
ID; it replays the result without duplicating the revision. Changed payloads need
a fresh request ID. Socket paths under `/tmp` do not grow with the project path.

Diagnostics: `prism --help`, `codex --version`, `claude --version`, `tmux -V`,
`codex plugin list --marketplace prism-local --json`, `claude plugin list`, and
the specific binding's `bridge.log` (or `supervisor.log` and `codex.log` for the older launcher). An unavailable bridge
disables Share even if a stale socket remains. Do not delete the project
database to repair a runtime connection.

## Remove

Exit the CLI session and Prism UI, then run the commands for the
packages you installed:

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
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python3 -m unittest discover -s tests -p probe_codex_test.py
PRISM_BINARY=target/debug/prism python3 tests/claude_lifecycle.py
PRISM_BINARY=target/debug/prism python3 tests/attachment.py
```

Completed checks and the real Codex and Claude publication/Share/revision loops
are recorded in [validation evidence](docs/validation.md).

Python native probes additionally require `websockets==15.0.1` in a virtual
environment. Real-model probes use isolated Codex homes and can consume model
usage; their options are documented in [Codex integration evidence](docs/codex-integration.md).
`tests/claude_lifecycle.py` and `tests/probe_claude.py` use the installed
`claude` CLI in print mode with an isolated `CLAUDE_CONFIG_DIR`, need no login
and make no model request.

Dependency versions are pinned in `Cargo.lock`. Ratatui, Crossterm, rusqlite and
libsqlite3-sys declare MIT; the other direct Rust dependencies declare MIT and/or
Apache-2.0. Bundled SQLite is public domain. For all resolved dependencies,
including target-specific transitive crates, inspect their `license`,
`license_file`, and `manifest_path` with `cargo metadata --locked --format-version 1`;
the source packages contain the applicable license texts and notices. Preserve
those notices when redistributing a binary. No repository-wide application
license is inferred from dependency licenses.
