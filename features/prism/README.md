# Prism

Prism 0.1.0 is a project-local visual workspace beside a launcher-bound Codex
conversation in tmux. It provides lists, diagrams, acceptance, archive/undo,
private questions, and multiple views of shared content. Claude integration is
owned separately and is not included in this implementation; its implementation
prompt is in [Claude handover](docs/claude-handover.md).

## Requirements

- Linux with `/proc`, tmux, and a terminal supporting UTF-8 and alternate screens.
- Authenticated Codex CLI with native plugins and Unix app-server connections.
  The tested native contract is **Codex CLI/app-server 0.167.0**, **tmux 3.3a**,
  on Debian 12 aarch64 with Node 24.22.2. Other versions/platforms are not certified.
- To build: Rust **1.99.1**, a C compiler/linker and libc headers. SQLite is
  bundled. See [development setup](docs/development-setup.md) for this
  container's user-local Zig compiler setup.

Codex labels the app-server interface experimental. Prism keeps authentication,
models, approvals and permission policy in Codex; it never answers native
approval requests. Native **Start, Steer and Queue are disabled** because
immediate conversation-switch detection remains unverified. The supported
question workflow is explicit **Share**, followed by retrieval in Codex.

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

## Open a conversation

```sh
cd /path/to/project
prism --agent codex
```

Outside tmux, Prism creates and attaches a new tmux session. Inside tmux, it
creates and selects a new window in the current session. It launches the normal
Codex TUI against its own private app-server. Ask Codex to “show this plan in
Prism”; the native publication tool opens the pane to the right. Later tool
calls reuse that pane. No UI pane opens until a Prism tool is called.

Resume only an explicitly identified persisted Codex thread:

```sh
prism --agent codex --project /path/to/project --resume THREAD_UUID
```

Installing the plugin into an already-running unregistered CLI does not prove
which tmux pane owns it. Its tools return a launcher/resume instruction. Exit
that CLI before explicitly resuming it through Prism; Prism never chooses a
recent thread or adopts a pane by project path alone. A second launch of the
same live project/thread is rejected with the existing binding and pane.

`prism --agent codex --detach` starts without attaching, for managed terminal
sessions and integration checks. `prism ui --binding /protected/binding.json`
reopens the persisted workspace for that binding; use the launcher's actual
binding path. `prism bridge --binding PATH` and `prism mcp` are service entry
points normally started by the launcher and native plugin.

## Work with content

Ask Codex to publish a list or diagram. It supplies stable content IDs; revisions
reuse those IDs. Accepting content records feedback on that content revision,
not permission for implementation. Semantic edits make earlier acceptance stale.
Archive affects every view; hide affects the current view. Undo rejects edits
that conflict with later changes instead of overwriting them.

Select content, press `e`, write a question, then press `Ctrl-S` to **Share**.
Return to Codex and ask it to retrieve your shared Prism question. Codex can
read the frozen selection and question, answer in that conversation, and revise
the visual content. Share does not start a model turn or prove an answer began.
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
| `Ctrl-T` / `Ctrl-W` / `Ctrl-R` | Duplicate view / close tab / reopen closed tab |
| `Ctrl-\` / `F6` | Toggle split / focus other split |
| `Shift` + arrows / `Alt` + arrows | Pan diagram / move focused diagram card |
| `e` / `Esc` | Edit question / leave editor |
| `Enter` | Newline in question |
| `Ctrl-S` / `Ctrl-Enter` | Explicitly share question |
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
`.prism/run/` contain bindings, sockets and logs; directory mode is 0700 and
binding mode is 0600. Tokens never belong in command arguments or published
content. Do not share binding files or runtime logs as public diagnostics.

Schema 2 adds private UI arrangement with an in-place transactional migration
from schema 1; content, questions and views are preserved. Unknown newer schemas
are refused. Stop live Prism processes before copying `.prism/` for a backup;
include SQLite's WAL/SHM files if copying a live database. Keep `.prism/` out of
version control.

CLI exit, server loss or an observed conversation switch retires the binding.
Persisted content remains available. `/new` may not be observed until another
native event or tool call; this is why native automatic sends remain disabled.
Use a fresh explicit launcher/resume binding after retirement. Questions with
unknown outcomes are never resent automatically.

If pane creation fails after publication, the tool returns the committed revision
and pane error separately. Retry the identical publication with the same request
ID; it replays the result without duplicating the revision. Changed payloads need
a fresh request ID. A too-long canonical project path is rejected because Unix
sockets have a platform path-length limit; use a shorter actual project path.

Diagnostics: `prism --help`, `codex --version`, `tmux -V`,
`codex plugin list --marketplace prism-local --json`, and the specific binding's `supervisor.log` and
`codex.log`. An unavailable bridge disables Share even if a stale socket remains.
Do not delete the project database to repair a runtime connection.

## Remove

Exit the launcher-owned Codex session and Prism UI, then run:

```sh
codex plugin remove prism@prism-local
codex plugin marketplace remove prism-local
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
```

Completed checks and the real Codex publication/Share/revision loop are recorded
in [validation evidence](docs/validation.md).

Python native probes additionally require `websockets==15.0.1` in a virtual
environment. Real-model probes use isolated Codex homes and can consume model
usage; their options are documented in [Codex integration evidence](docs/codex-integration.md).

Dependency versions are pinned in `Cargo.lock`. Ratatui, Crossterm, rusqlite and
libsqlite3-sys declare MIT; the other direct Rust dependencies declare MIT and/or
Apache-2.0. Bundled SQLite is public domain. For all resolved dependencies,
including target-specific transitive crates, inspect their `license`,
`license_file`, and `manifest_path` with `cargo metadata --locked --format-version 1`;
the source packages contain the applicable license texts and notices. Preserve
those notices when redistributing a binary. No repository-wide application
license is inferred from dependency licenses.
