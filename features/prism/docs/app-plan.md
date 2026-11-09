# Prism desktop app: plan

Status: plan only. Nothing here is implemented; saving this file authorizes no code change.

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
- **Pairing**: the app writes `{port, token}` to `~/.claude/prism/app.json` (mode 0600).
  `~/.claude` is mounted into the Claude containers. `PRISM_APP_FILE` overrides the path for
  containers that mount something else (Codex).
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
   1.3 Confirm the pairing file is visible in the Claude container and decide the Codex path.

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

7. **Remove the terminal UI** (needs explicit approval, see Decisions 1 and 2)
   7.1 Delete `src/ui/`, `tests/ui.rs`, `tests/diagram.rs`, `tests/graph.rs` (moved to core),
       `examples/`, and the dependencies `ratatui`, `crossterm`, `resvg`, `usvg`,
       `tiny-skia`, `fontdb`.
   7.2 Remove the Prism pane handling: `ensure_ui`, pane tags, `allow-passthrough`, the
       `pane` field in tool results; `/prism` reports the app connection instead.
   7.3 Update plugin texts, `SKILL.md`, README and `docs/validation.md`.

8. **Packaging**
   8.1 macOS `.app` / `.dmg`, built on the user's Mac; unsigned first.
   8.2 Windows installer, built in CI or on a Windows machine.
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

## Decisions needed

1. **Delete the terminal UI** (about 4,400 lines in `src/ui/` plus its tests) once the app
   covers list, diagram and ask. Confirm.
2. **tmux**: with no UI pane, Prism needs tmux only to identify the CLI pane. Proposal: drop
   the requirement and identify the CLI by process ID and start time, which the binding
   already records. This also retires the older `prism --agent` launcher (`src/main.rs`
   supervisor, most of `src/session.rs`, `tests/launcher.rs`, `tests/launcher_lifecycle.py`).
   Confirm, or keep tmux and the launcher.
3. **Windows build**: CI (GitHub Actions) or a Windows machine you provide.
4. **Signing**: unsigned builds for your own machines, or Apple Developer ID and a Windows
   certificate for distribution.
5. **Codex containers**: where the pairing file is visible there.
6. **Default port**: 7717 unless it collides with something you run.

## Risks

- `host.docker.internal` behaves differently outside Docker Desktop; gate 1.1 settles it.
- The app shows live content from several containers; the pairing token is the only thing
  that stops another local process from posing as a bridge. Token file mode and location matter.
- Two bridges for the same conversation (resume in a second container) must not fight over
  one tab; the app keeps the newest generation and marks the older one ended.
