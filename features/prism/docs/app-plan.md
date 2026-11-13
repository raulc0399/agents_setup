# Prism desktop app: plan

Status: all batches and the follow-ups below are implemented on branch `prism-app` (2026-11-13), not merged into `main`. Open: the manual passes on the Mac, builds on Mac and PC, and the Codex items in `docs/codex-handover.md`.
Diagram: [app-plan.html](app-plan.html), clickable; keep both files in step.

## Start here (handover)

- **Code today**: `features/prism/` holds two Cargo workspaces. The bridge workspace:
  `prism` (`src/bridge.rs` tool operations, `src/applink.rs` app link, `src/store.rs`
  SQLite, `src/actions.rs` publish and shared actions, `src/attachment.rs` attach by process
  ID and start time, `src/session.rs` bindings, `src/mcp.rs` Codex tools) and `prism-core/`
  (model, app protocol, ask text, diagram layout and SVG in `graph.rs`). The app workspace
  in `app/`: `host/` (listener, registry, arrangement; testable on Linux), `src-tauri/`
  (Tauri shell), `ui/` (plain HTML, CSS and JavaScript), `ui-tests/` (headless Chromium
  checks). Plugins: `plugins/claude/` (mod `hooks/prism.js`, `hooks/tools.json`, skill,
  mod tests), `plugins/codex/` and `src/adapters/codex.rs` (Codex agent's area).
- **Build and test the bridge** (from `features/prism`, with `CARGO_BUILD_JOBS=2` in the
  container): `cargo fmt --all --check`,
  `cargo clippy --locked --workspace --all-targets -- -D warnings`,
  `cargo test --locked --workspace --all-targets`, `cargo build --locked --release`; install
  with `install -m 755 target/release/prism ~/.local/bin/prism`. Mod:
  `claude plugin test plugins/claude`, `claude plugin validate plugins/claude`.
- **Build and test the app** (from `features/prism/app`): `cargo fmt --all --check`,
  `cargo clippy --locked -p prism-app-host --all-targets -- -D warnings`,
  `cargo test --locked -p prism-app-host --all-targets`, `bash ui-tests/run.sh` (11 check
  suites and screenshots in `app/target/ui-screenshots/`). Tauri cannot be built in the
  container; on the Mac run `cargo tauri dev` or `cargo tauri build` in `app/src-tauri`
  (see `app/README.md`).
- **Versions that must match**: app protocol 3 (`APP_PROTOCOL` in
  `prism-core/src/protocol.rs`). After a change to the bridge, the mod, `tools.json` or a
  skill: install the binary, then restart Claude (`/exit`, `claude -c`). After a change to
  `prism-core` or the app's Rust code: restart `cargo tauri dev`; changes to `app/ui/` reload
  by themselves.
- **Environment**: each CLI runs in its own throwaway container (`d_claude`, `d_codex`,
  `docker compose run --rm`). Only the project and `~/.claude` / `~/.codex` survive a new
  container; `~/.local/bin/prism` must be reinstalled after one, and
  `git config --global --add safe.directory /workspace/agents_setup` set again. The
  container has no `ssh`, so pushes happen on the host.
- **A session's tab is named after the folder Claude was started in**, and its documents
  live in that folder's `.prism/state.db` (`/workspace/agents_setup` or `features/prism`).
- **Lessons that still hold**: the project mount can report stale file attributes right
  after a rename, so binding reads retry briefly; the mod polls every second through the
  read-only `pending` operation; a bridge that died is restarted by the next attach; WebKit
  does not resolve a custom property set on an SVG group for its children (see
  `app/ui/styles.css`).
- **Working rules**: plan and approval before code (see the repository's `AGENTS.md`). The
  user's flow for this work: one Opus agent develops a piece, a second Opus agent reviews
  it, fixes go back to the first, then commit as `raulc0399 <raci0399@gmail.com>`; the
  separate review may be skipped for small, well-tested pieces. Decisions 1 to 8 below
  bind; Decision 8 (shared state only) decides where new state lives.
- **How Prism is used** (the user's words): Prism is a view on the current chat. The agent
  publishes lists and diagrams, the user asks, explains, drops, accepts and bookmarks from
  the app, decisions are recorded on the items, and at the end the agent saves the plan to
  a file.
- **Next steps**: the manual pass on the Mac (pinch zoom, line widths, arrow tips, split
  view, Explain and bookmarks in a real session), packaging builds per `app/README.md`,
  merge of `prism-app` into `main`, and `docs/codex-handover.md` for the Codex agent.
  "Later" below lists ideas that were postponed.

## Progress

| Batch | State | Note |
| --- | --- | --- |
| 1 Gates | **[done]** 1.1, 1.2, 1.3 (Claude half) passed | the Codex half of 1.3 is for the Codex agent to confirm |
| 2 `prism-core` | **[done]** | protocol 3 (2 before bookmarks, 1 before documents could close): `hello` → `welcome` → `auth` → `snapshot`; see `prism-core/src/protocol.rs` |
| 3 Bridge app link | **[done]** | `src/applink.rs`; commits noticed by polling SQLite `data_version`; legacy bindings report `bound_at_ms = 1` |
| 4 App shell | **[done]**, Mac pass open | one review round; the fix round was committed without a second review pass (usage limit). Mac checklist: see `app/README.md` if present, else the batch 4 report in the session |
| 5 Diagram | **[done]**, Mac pass open (pinch, save dialog, Tab in WKWebView) | layout in the shell via `prism-core`; `LANE_GAP` grew to 48, arrows that skip a box run below the row |
| 6 Ask, answer, drop, undo | **[done]**, Mac pass open (text selection, ⌘-click, prompt round trip) | multi-item ask lists every item; questions ordered by `(selection.revision, request_id)` with UUID v7 ids |
| 7 Remove terminal UI | **[done]**, real loop open | needs: install binary, `/exit`, `claude -c`, app running on the Mac; steps in `docs/validation.md` |
| 8 Packaging | **[done]**, builds open | build on the Mac and the PC per `app/README.md`; install on a clean account |
| 9.2 Codex handover | **[done]** | `docs/codex-handover.md` |

Gate results (2026-11-09, from the `d_claude` container):

- **1.1 passed.** `host.docker.internal` resolves to `192.168.65.254` (IPv4) and to an IPv6
  address that is unreachable. A TCP connect to the Mac's own AirPlay listener
  (ports 5000 and 7000) succeeded and returned its HTTP banner; port 7717 answered
  "connection refused" (host reached, nothing listening). Consequence: the bridge must try
  every resolved address (or IPv4 first). Windows Docker Desktop provides the same name.
  Plain Linux Docker needs `--add-host=host.docker.internal:host-gateway` and an app
  listener that is not bound to loopback only.
- **1.2 passed** (user's Mac, 2026-11-09). The Mac had neither Node nor Rust; after
  installing rustup, `cargo install create-tauri-app tauri-cli --locked`,
  `cargo create-tauri-app prism-probe --template vanilla --manager cargo --yes` and
  `cargo tauri dev` opened the window. Consequence: Decision 7.
- **1.3 Claude half passed.** `~/.claude` is a host mount (virtiofs); a file written to
  `~/.claude/prism/` in the container is visible there. `~/.codex` is not mounted in the
  Claude container (expected); the Codex half is for the Codex agent to confirm.

## Goal

Replace the terminal UI with one desktop app for macOS and Windows. A tab is one
conversation; a window is a group of tabs the user arranges (for example projects 1 and 2 in
window A, projects 3 and 4 in window B). The CLI, its plugin and the bridge keep running in
the Linux container.

## Architecture

| Part | Runs | Role |
| --- | --- | --- |
| Prism app (new, Tauri 2) | host | One process, one listening port. Windows, tabs, cards, diagram, zoom. Keeps only arrangement and view state. |
| `prism bridge` (stays) | container, one per conversation | Owns content, questions, answers in `.prism/state.db`. New: dials out to the app over WebSocket. |
| Plugins (stay) | container | Tools `publish`, `inspect`, `action`, `questions`, `answer`; the Claude mod fills the prompt box. |
| `prism-core` (new crate) | both | Portable model, app protocol messages, diagram layout and SVG (from `src/ui/graph.rs`). |

- **Connection**: the bridge opens a WebSocket to `host.docker.internal:<port>`; both sides
  send. If the app is not running the bridge retries quietly and the CLI is unaffected.
- **Pairing**: the app writes `{port, token}` (mode 0600) to `~/.claude/prism/app.json` and
  `~/.codex/prism/app.json`. The Claude containers mount the first, the Codex containers the
  second. `PRISM_APP_FILE` overrides the path for any other setup.
- **Identity**: each bridge announces container host name, project, agent, conversation and
  CLI process. The app keys tabs by conversation and remembers project → window.
- **View state** (hidden items, filter, zoom, pan, selection) moves into the app's own data
  directory. The store keeps content, questions, answers and undo records.

## Flows

1. **Publish**: agent tool → bridge → store → `update` to the app → tab redraws.
2. **Ask**: select text or an item in the app → `ask` to the bridge → shared question →
   mod poll → the reference is placed in the prompt box → user types the question and
   sends → the agent answers in the chat. Answers are not recorded in Prism for now; the
   app changes only when the agent publishes the revised item.
3. **Drop / undo**: app → bridge → store → `update` to every connected app view; a dropped
   item stays visible in its place, greyed, with its number.
4. **Session end**: bridge sends `closed`; the tab stays, marked ended, until the user closes it.

## Batches

1. **Gates (no product code)**
   1.1 Probe: a container reaches a host listener through `host.docker.internal` on Docker
       Desktop for Mac; note what Windows and plain Linux Docker need.
   1.2 Probe: a minimal Tauri 2 app builds and opens on the user's Mac.
   1.3 Confirm the pairing file is visible in a Claude container and in a Codex container.

2. **`prism-core` crate** **[done]**
   2.1 Cargo workspace: `prism-core` (portable), `prism` (bridge, Linux). The app is its own
       workspace in `app/` (own `Cargo.lock`), because Tauri cannot build in the container
       and the bridge's `cargo test --workspace` must keep working there.
   2.2 Move `model.rs` types the app needs and the layout/SVG half of `src/ui/graph.rs`;
       drop the rasterizer and the terminal image code.
   2.3 Define app protocol messages: `hello`, `welcome`, `auth`, `snapshot`, `update`,
       `closed`, `reply`, `refused`, and the commands `ask`, `action`, `undo`, `fetch`.
       Versioned, with a protocol number. The pairing token never crosses; both sides
       prove it with an HMAC over the other side's nonce.
   2.4 SVG carries `data-id` and `data-kind` on boxes, arrows, pills and gates for clicks.

3. **Bridge app link** **[done]**
   3.1 WebSocket client with reconnect and backoff, authenticated by the pairing token.
   3.2 Send `hello` and `snapshot` on connect, `update` after every committed change.
   3.3 Apply commands from the app through the existing shared actions (same checks as tools).
   3.4 Tests against a fake app listener: reconnect, replay after reconnect, refused token,
       stale revision, app absent.

4. **App shell** **[done]** (manual Mac pass open)
   4.1 Listener, pairing file, session registry.
   4.2 Windows and tabs: tab per conversation, drag between windows, project → window
       memory, list of live and ended sessions to reopen a closed tab.
   4.3 List view: cards with kind color, expanded focused card, `ask`, `hide`, `drop`.

5. **Diagram in the app** **[done]** (follows the "Diagrams (HTML)" rules in CLAUDE.md)
   5.1 Lanes = groups, boxes = bold name + subtitle ≤ 5 words, numbered pills on arrows,
       dashed = `manual`, amber gate badges.
   5.2 Pan and zoom with wheel, pinch, drag and keys; fit-all at start.
   5.3 Every box, arrow, pill and badge is clickable and keyboard-focusable; a side panel
       shows tag, title, text, source and the questions with answers.
   5.4 Light and dark theme through CSS tokens; SVG export.

6. **Ask, answer, drop, undo in the app** **[done]**
   6.1 Native text selection inside cards and the side panel; `Ask` sends item + quote.
   6.2 Question state on the item: shared, in prompt box, answered, with the answer text.
       (2026-11-10: answers are not recorded for now, so "answered" shows only for older
       questions; Ask places the reference in the prompt box without an editor.)
   6.3 Drop and undo; filter and hide stay local to the tab.
   6.4 Accept and multi-selection (Space), as the terminal list view has them today; needed
       before batch 7 deletes it (found in the batch 4 review).

7. **Remove the terminal UI, the tmux requirement and the old launcher** **[done]** (Decisions 1 and 2)
   7.1 Delete `src/ui/`, `tests/ui.rs`, `tests/diagram.rs`, `tests/graph.rs` (moved to core),
       `examples/`, and the dependencies `ratatui`, `crossterm`, `resvg`, `usvg`,
       `tiny-skia`, `fontdb`.
   7.2 Remove the Prism pane handling: `ensure_ui`, pane tags, `allow-passthrough`, the
       `pane` field in tool results; `/prism` reports the app connection instead.
   7.3 Attach without tmux: identify the CLI by process ID and start time; delete the
       `prism --agent` launcher, its supervisor and their tests.
   7.4 Update plugin texts, `SKILL.md`, README and `docs/validation.md`.

8. **Packaging** **[done]** (builds on the user's machines open)
   8.1 macOS `.app` / `.dmg`, built on the user's Mac, unsigned.
   8.2 Windows installer, built on the user's Windows PC, unsigned.
   8.3 Version handshake so an old bridge and a new app refuse each other with a clear message.

9. **Codex**
   9.1 The MCP path uses the same bridge and app link; nothing app-specific in the adapter.
   9.2 Codex-only changes are listed in `docs/codex-handover.md` for the Codex agent. **[done]**

## Follow-ups after the batches (2026-11-10)

- **[done]** Default view by content: Diagram when the document has arrows or groups, else
  List; toggle only when there is something to draw; mode saved per document.
- **[done]** Toolbar keeps only the document strip; view toggle, filter, item count and
  revision, hidden/selected counts and SVG export sit in the right pane in both views, with
  the focused item's details below.
- **[done]** Arrows inside one lane or between the same two lanes get distinct colors
  (`prism-arrow-c0..5`, tokens `--arrow-0..5`); focus and selection marked by outline and
  halo, not hue.

- **[done]** (2026-11-11) Close and reopen documents as shared state; numbers never change;
  dropped items stay in place; ask goes straight to the CLI prompt box; answers stay in the
  chat.
- **[done]** (2026-11-11) Diagram: card-style boxes, smooth zoom with an explicit detail
  level, step numbers on arrows, pills clear of each other, rounded arrows, lanes as rows
  or columns (stored at the first publish), `manual: true` for any dashed arrow.
- **[done]** (2026-11-11) App: resizable side pane, two documents side by side, restored
  tabs show whether their conversation ended.
- **[done]** (2026-11-12) Explain: `E` or the Explain button sends an explain request for
  the focused item, the selection or the quoted text; the Claude mod submits it by itself;
  the agent records the explanation with the answer tool and the side pane shows it under
  "Explanation". Ordinary Ask answers stay in the chat.
- **[done]** (2026-11-12) Bookmarks: `B` marks the focused item, the selected items or
  the selected text; `Shift+B` lists the conversation's bookmarks and jumps to them. They
  are shared state (actions `bookmark`/`unbookmark`, app protocol 3), so the agent reads
  them with inspect.
- **[done]** (2026-11-12) Working rule in both skills: a decision or change agreed in the
  chat is recorded on the list item as `Decided (date): … Why: ….` and republished.
- **[done]** (2026-11-13) When an explanation the user asked for arrives, the app focuses
  the item and scrolls the side pane to it.

## Later (not planned yet)

- Free-form documents (2026-11-11): the agent publishes its own SVG or HTML, shown as it
  is without scripts; boxes and arrows carry item IDs so the side pane, ask and drop still
  work. For polished diagrams; no automatic layout, numbering or detail levels.
- A chat thread per item in Prism (2026-11-12): type under the item, the question runs
  through the CLI conversation, the answer shows under the item. Postponed.
- Notes beside diagram boxes (2026-11-11): left out.

## Validation per batch

- Batches 2, 3: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` for the touched crates.
- Batches 4–6: protocol tests in Rust; UI checked through headless screenshots at desktop
  width plus a manual pass by the user on the Mac.
- Batch 7: full suite; a real Claude loop: publish, ask, answer, drop, undo, `/clear`, resume.
- Batch 8: install from the built artifact on a clean user account.

## File ownership (if batches run in parallel)

| Owner | Paths |
| --- | --- |
| Core | `prism-core/` |
| Bridge link | `src/bridge.rs`, `src/applink.rs` (new), `tests/bridge.rs`, `tests/applink.rs` (new) |
| App | `app/` |
| Integrator | `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/main.rs`, README, docs |
| App | `app/Cargo.toml`, `app/Cargo.lock` (the app's own workspace) |
| Codex agent | `src/adapters/codex.rs`, `plugins/codex/`, Codex probes and docs |

Batch 2 must land before 3, 4 and 5 start; 3 and 4 can then run in parallel.

## Decisions (settled 2026-11-09)

1. **Terminal UI**: deleted once the app covers list, diagram and ask (batch 7).
2. **tmux**: no longer required. The CLI is identified by process ID and start time, which
   the binding already records. The older `prism --agent` launcher goes with it
   (`src/main.rs` supervisor, most of `src/session.rs`, `tests/launcher.rs`,
   `tests/launcher_lifecycle.py`).
3. **Builds**: on the user's own machines, one build command on the Mac and one on the
   Windows PC. No CI.
4. **Signing**: none; unsigned builds for the user's own machines.
5. **Pairing file**: written to both `~/.claude/prism/app.json` and
   `~/.codex/prism/app.json`; `d_claude` mounts the first, `d_codex` the second.
6. **Port**: 7717.
7. **No Node** (2026-11-09): the frontend is plain HTML, CSS and JavaScript without a
   bundler; the app is built with `cargo tauri build` only.
8. **Shared state only** (2026-11-10): all content and document state (publish, close,
   drop, accept, questions, answers) goes through the bridge into `.prism/state.db`, so the
   agent and every app view see the same thing; no app-local document state. Prism shows the
   conversation: we discuss what the agent publishes, and another agent can execute it.
   Only presentation (view mode, zoom, pan, filter) stays in the app.

## Risks

- `host.docker.internal` behaves differently outside Docker Desktop; gate 1.1 settles it.
- The app shows live content from several containers; the pairing token is the only thing
  that stops another local process from posing as a bridge. Token file mode and location matter.
- Two bridges for the same conversation (resume in a second container) must not fight over
  one tab; the app keeps the newest generation and marks the older one ended.
