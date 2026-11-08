# Prism validation

Validation uses Linux `/proc`, Debian 12 aarch64, tmux 3.3a, Codex CLI/app-server
0.167.0, Node 24.22.2, and Rust 1.99.1. Native automatic Start/Steer/Queue remain
disabled; none of these checks clears the immediate-conversation-switch gate.

Final combined result: **100 Rust tests, 16 Python probe regressions, 9
release-binary lifecycle tests**, formatting, Clippy and release build passed.
`cargo install --locked --path . --root "$HOME/.local"` installed Prism 0.1.0;
`prism@prism-local` 0.1.0 is installed and enabled in the user's Codex home.

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
```

`launcher_lifecycle.py` uses a temporary project, a fake native WebSocket server,
and its own tmux server. It tests real Prism processes and terminal behavior,
without model usage. Native protocol evidence comes separately from
`probe_codex.py` and the real-model loop below. The default native probe creates
its own Codex home/server and does not make model requests.

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
