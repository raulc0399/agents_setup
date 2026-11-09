# Prism validation

Validation uses Linux `/proc`, Debian 12 aarch64, tmux 3.3a, Codex CLI/app-server
0.167.0, Claude Code 2.1.319, Node 24.22.2, and Rust 1.99.1. Native automatic
Start/Steer/Queue remain disabled for both agents; none of these checks clears
the Codex conversation-switch gate or supplies Claude request-to-turn correlation.

Codex release result (f476550): **100 Rust tests, 16 Python probe regressions,
9 release-binary lifecycle tests**, formatting, Clippy and release build passed.
`cargo install --locked --path . --root "$HOME/.local"` installed Prism 0.1.0;
`prism@prism-local` 0.1.0 is installed and enabled in the user's Codex home.

Claude integration result (2026-11-09): **116 Rust tests** (7 Claude adapter
contract tests, 4 Claude package tests, lifecycle and agent-separation tests),
**16 Codex probe regressions, 9 Codex lifecycle tests, 4 Claude lifecycle
tests**, formatting, Clippy with warnings denied and the release build passed
after the independent review fixes. The host's development toolchain is wiped on
container restart; reinstall it per `docs/development-setup.md` first.

## Automated checks

From `features/prism/`, with the compiler environment in
`docs/development-setup.md`:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked
python3 -m venv /tmp/prism-validation-venv
/tmp/prism-validation-venv/bin/pip install websockets==15.0.1
/tmp/prism-validation-venv/bin/python -m unittest discover -s tests -p probe_codex_test.py
PRISM_BINARY=target/debug/prism /tmp/prism-validation-venv/bin/python tests/launcher_lifecycle.py
/tmp/prism-validation-venv/bin/python tests/probe_codex.py
PRISM_BINARY=target/debug/prism python3 tests/claude_lifecycle.py
python3 tests/probe_claude.py
```

`launcher_lifecycle.py` uses a temporary project, a fake native WebSocket server,
and its own tmux server. It tests real Prism processes and terminal behavior,
without model usage. Native protocol evidence comes separately from
`probe_codex.py` and the real-model loop below. The default native probe creates
its own Codex home/server and does not make model requests.

`claude_lifecycle.py` uses the real launcher, the real installed `claude` CLI in
print mode (`--plugin-dir plugins/claude`, isolated `CLAUDE_CONFIG_DIR`, no login,
no model request) and its own tmux server. It proves the pre-bound
`--session-id`, the CLI pane as the recorded native process, the mod's `/prism`
reaching the bridge and opening the UI pane to the right, foreign session
rejection, lifecycle retirement with the CLI pane kept alive, and explicit
`--resume` forwarding. `probe_claude.py` records the native mod runtime facts in
`docs/claude-integration.md`.

## Real Codex loop

The completed-app check used a temporary project and temporary `CODEX_HOME`,
copied authentication with mode 0600, and installed the shipped native package
there. It used the actual `prism --agent codex --project PATH --detach` launcher,
ordinary Codex TUI, private app-server, bundled MCP process, bridge and real UI.
The model was `gpt-6.1-sol`; this check consumes model usage.

| Check | Observed result |
| --- | --- |
| Publication and caller binding | A real model called `prism_publish` with two nodes and a directed relationship; native metadata bound the exact TUI thread and opened the right-hand pane. |
| Approval ownership | Each publication/retrieval received one-time Allow in the Codex TUI. Observer RPC clients never answered approvals; no persistent allow rule was created. |
| Shared actions | UI acceptance, archive and undo advanced the same document through revisions 4, 5 and 6; the archived node and relationship were restored. |
| Share retrieval | UI explicit Share persisted one frozen question; the same Codex conversation listed and fetched it through `prism_questions`, then answered with the marker `PRISM_ANSWERED`. |
| Private draft | `UNSENT_PRIVATE_RESTART` persisted intact without creating another question; native document inspection did not return it. |
| UI restart | Closing only the UI and calling native inspection reopened a new owned pane; the same database, tabs and private draft remained. |
| CLI exit | Exiting the actual Codex TUI retired its binding; the existing Prism pane displayed disconnected state and retained revision 7. |
| Installed release resume | Exact resume restored revision 7/private drafts; a second live exact resume was rejected. A fresh fully typed Share survived CLI exit and was retrieved unchanged through native MCP after another exact resume. |
| Stable revision | The same model conversation called `prism_publish` with the existing document/node IDs and expected revision 6; revision 7 retained the omitted source and relationship. |

To reproduce, publish a small list/diagram from the launcher-bound Codex TUI;
select content, accept/archive/undo, then explicitly Share a question and ask
Codex to retrieve it. Reselect after shared mutations: actions clear the previous
revision's selection. Enter the question editor before typing; an unselected
view correctly refuses `e`. Use fresh request UUIDs for changed publications.

The native probe also confirms installed-plugin MCP discovery and metadata,
private socket mode, actual server identity, and expected rejection of a fresh
unpersisted thread resume. See `docs/codex-integration.md` for the unresolved
native delivery gates. A passing fake-server lifecycle test does not certify
all Codex versions or bypass these gates.

## Real Claude loop

The check on 2026-11-09 used a temporary project (`/tmp/pl/project`, a short path
because of the Unix socket limit), a temporary isolated `CLAUDE_CONFIG_DIR`
holding a 0600 copy of the user's claude.ai login plus onboarding state, the
shipped `plugins/claude` package installed there through
`claude plugin marketplace add` and `claude plugin install prism@prism-local`
(not `--plugin-dir`), the release `prism --agent claude --project PATH --detach`
launcher inside a private tmux server, the ordinary interactive `claude` CLI
(2.1.319, Opus 5.5, auto permission mode by default in that fresh config), the
bridge and the real UI. The copied credentials were deleted afterwards. This
check consumes Claude usage.

| Check | Observed result |
| --- | --- |
| Launch and identity | `binding.json` recorded `agent: claude`, the pre-bound session UUID, `server_endpoint: claude-cli://%1` and the CLI pane PID; the pane ran `claude --session-id` with that UUID after the folder-trust prompt. |
| `/prism` from the installed mod | Opened Prism pane `%2` to the right (0 documents) without a model turn. |
| Publication and caller binding | The real model called `mcp__prism__publish` with three steps, one finding and three relationships; revision 1 appeared in the pane. No permission prompt was shown under auto mode; Prism has no approval channel and answered none. |
| Shared actions | UI acceptance, archive (cascading the finding's relationship) and undo advanced revisions 2, 3 and 4; the archived content was restored. |
| Stable revision | The model inspected and republished at expected revision 4 → revision 5, adding "Enforce size limit" and re-pointing an existing relationship while keeping every existing ID. |
| Stale selection | Sharing a draft selected at revision 4 after revision 5 was refused as STALE with the draft kept; reselect and `Ctrl-S` shared it. A retrieval requested before the Share correctly reported no shared questions. |
| Share retrieval | The same Claude session retrieved the frozen question through `mcp__prism__questions` and answered with the marker `PRISM_ANSWERED`. |
| `/clear` | The mod's `session.end` report retired the binding (generation 2) within seconds; the CLI pane stayed alive; the UI showed `Disconnected · Binding retired; explicitly resume` with revision 5 retained; `/prism` in the new session returned `prism --agent claude --resume <new session UUID>`. |
| Exit and exact resume | After `/exit`, `--resume <original UUID>` restored revision 5 and the shared question in a new binding and reopened a pane on `/prism`; a second live exact resume was rejected naming the existing pane. |
| Simultaneous sessions | A second `prism --agent claude` in the same project created a separate window, binding and empty workspace while the first stayed connected. |
| CLI exit | `/exit` in each pane retired its binding; Prism sent no signal to any Claude process. |

Interactive approval ownership in default (non-auto) permission mode and
busy-queue cancellation were not exercised; they gate only the disabled native
send modes, not Share.

## On-demand tmux attachment (2026-11-09)

The native plugins now attach to an already-running CLI inside tmux. The first
Prism call verifies inherited tmux identity against Linux process ancestry,
creates the bridge, and opens the UI to the right without launching another CLI.
Attached bindings never authorize termination of the user's CLI or its pane.

`tests/attachment.py` verifies lazy attachment, publication, pane reuse/reopening,
refusal of unrelated processes and conflicting conversations, duplicate bridge
ownership, outside-tmux instructions, and real Claude `/prism` through its native
process API. Its four tests passed without model requests.

A separate isolated real Codex 0.167.0 TUI, started normally with the native Prism
plugin, called `prism_inspect` after its one-time tool approval. The call returned
`pane.id = "%1"`, the original pane remained `%0`, and the assistant confirmed
completion. No `prism --agent`, `--remote`, or replacement conversation was used.
The isolated tmux server and temporary Codex home were removed afterwards.

The attachment tracks Codex's MCP process lifetime, not app-server thread events;
it rejects a different live thread claiming the same pane. Claude session-end
hooks retire their attachment. Native Start/Steer/Queue remain disabled.
