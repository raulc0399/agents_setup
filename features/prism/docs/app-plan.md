# Prism desktop app: plan

Status: plan only. Nothing here is implemented; saving this file authorizes no code change.
Diagram: [app-plan.html](app-plan.html), clickable; keep both files in step.

## Start here (handover)

- **Code today**: `features/prism/` is one Rust crate. `src/bridge.rs` (bridge and tool
  operations), `src/store.rs` (SQLite), `src/attachment.rs` (attach a running CLI),
  `src/session.rs` (bindings, tmux), `src/ui/` (terminal UI, to be removed),
  `plugins/claude/` (mod, tools, skill), `plugins/codex/` and `src/adapters/codex.rs`
  (Codex agent's area).
- **Build and test**: `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets`, `cargo build --locked --release`; install with
  `install -m 755 target/release/prism ~/.local/bin/prism`.
- **Environment**: each CLI runs in its own throwaway container (`d_claude`, `d_codex`,
  `docker compose run --rm`). Only the project and `~/.claude` / `~/.codex` survive a new
  container; `~/.local/bin/prism` must be reinstalled after one.
- **The Claude mod loads at session start.** A change to `plugins/claude/hooks/prism.js`
  needs a Claude restart (`/exit`, `claude -c`).
- **Lessons from the terminal test (2026-11-09)**, all relevant to the bridge that stays:
  the project mount can report stale file attributes right after a rename, so binding
  reads retry briefly; the mod polls every second through the read-only `pending`
  operation; a bridge that died is restarted by the next attach.
- **Working rules**: plan and approval before code, one batch at a time, validation after
  each batch (see the repository's `AGENTS.md`).
- **First step**: batch 1, the three gates. No product code before they pass.

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
   mod poll → prompt box → user sends → `answer` tool → bridge → `update` → shown on the item.
3. **Drop / undo**: app → bridge → store → `update` to every connected app view.
4. **Session end**: bridge sends `closed`; the tab stays, marked ended, until the user closes it.

## Batches

1. **Gates (no product code)**
   1.1 Probe: a container reaches a host listener through `host.docker.internal` on Docker
       Desktop for Mac; note what Windows and plain Linux Docker need.
   1.2 Probe: a minimal Tauri 2 app builds and opens on the user's Mac.
   1.3 Confirm the pairing file is visible in a Claude container and in a Codex container.

2. **`prism-core` crate**
   2.1 Cargo workspace: `prism-core` (portable), `prism` (bridge, Linux), `app/src-tauri`.
   2.2 Move `model.rs` types the app needs and the layout/SVG half of `src/ui/graph.rs`;
       drop the rasterizer and the terminal image code.
   2.3 Define app protocol messages: `hello`, `snapshot`, `update`, `closed`, and the
       commands `ask`, `action`, `undo`, `fetch`. Versioned, with a protocol number.
   2.4 SVG carries `data-id` and `data-kind` on boxes, arrows, pills and gates for clicks.

3. **Bridge app link**
   3.1 WebSocket client with reconnect and backoff, authenticated by the pairing token.
   3.2 Send `hello` and `snapshot` on connect, `update` after every committed change.
   3.3 Apply commands from the app through the existing shared actions (same checks as tools).
   3.4 Tests against a fake app listener: reconnect, replay after reconnect, refused token,
       stale revision, app absent.

4. **App shell**
   4.1 Listener, pairing file, session registry.
   4.2 Windows and tabs: tab per conversation, drag between windows, project → window
       memory, list of live and ended sessions to reopen a closed tab.
   4.3 List view: cards with kind color, expanded focused card, `ask`, `hide`, `drop`.

5. **Diagram in the app** (follows the "Diagrams (HTML)" rules in CLAUDE.md)
   5.1 Lanes = groups, boxes = bold name + subtitle ≤ 5 words, numbered pills on arrows,
       dashed = `manual`, amber gate badges.
   5.2 Pan and zoom with wheel, pinch, drag and keys; fit-all at start.
   5.3 Every box, arrow, pill and badge is clickable and keyboard-focusable; a side panel
       shows tag, title, text, source and the questions with answers.
   5.4 Light and dark theme through CSS tokens; SVG export.

6. **Ask, answer, drop, undo in the app**
   6.1 Native text selection inside cards and the side panel; `Ask` sends item + quote.
   6.2 Question state on the item: shared, in prompt box, answered, with the answer text.
   6.3 Drop and undo; filter and hide stay local to the tab.

7. **Remove the terminal UI, the tmux requirement and the old launcher** (Decisions 1 and 2)
   7.1 Delete `src/ui/`, `tests/ui.rs`, `tests/diagram.rs`, `tests/graph.rs` (moved to core),
       `examples/`, and the dependencies `ratatui`, `crossterm`, `resvg`, `usvg`,
       `tiny-skia`, `fontdb`.
   7.2 Remove the Prism pane handling: `ensure_ui`, pane tags, `allow-passthrough`, the
       `pane` field in tool results; `/prism` reports the app connection instead.
   7.3 Attach without tmux: identify the CLI by process ID and start time; delete the
       `prism --agent` launcher, its supervisor and their tests.
   7.4 Update plugin texts, `SKILL.md`, README and `docs/validation.md`.

8. **Packaging**
   8.1 macOS `.app` / `.dmg`, built on the user's Mac, unsigned.
   8.2 Windows installer, built on the user's Windows PC, unsigned.
   8.3 Version handshake so an old bridge and a new app refuse each other with a clear message.

9. **Codex**
   9.1 The MCP path uses the same bridge and app link; nothing app-specific in the adapter.
   9.2 Codex-only changes are listed in `docs/codex-handover.md` for the Codex agent.

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

## Risks

- `host.docker.internal` behaves differently outside Docker Desktop; gate 1.1 settles it.
- The app shows live content from several containers; the pairing token is the only thing
  that stops another local process from posing as a bridge. Token file mode and location matter.
- Two bridges for the same conversation (resume in a second container) must not fight over
  one tab; the app keeps the newest generation and marks the older one ended.
