# Codex handover (desktop app)

Written 2026-11-10, updated 2026-11-11, for the Codex agent. The Claude side is done; these points are
Codex-only or need the Codex container.

## Changed for Codex in batches 3-7

- `src/mcp.rs`: the binding-path parameter is gone; every call attaches the MCP process
  (`ensure_attached`, keyed by process ID and start time). Another thread through the same
  MCP process is refused. Tool descriptions of `prism_publish`, `prism_inspect`,
  `prism_questions` updated.
- `src/main.rs`: `prism mcp` no longer reads `PRISM_BINDING_FILE`; the `prism --agent`
  launcher and its supervisor are deleted.
- `src/bridge.rs`: the observer of the launcher's private app-server and its tests are
  deleted; the health check no longer reports `native_connected`.
- `src/adapters/codex.rs`: `thread_closed()` and `retirement_reason()` removed (only the
  observer used them).
- `plugins/codex/.mcp.json`: `env_vars` is `["PRISM_APP_FILE", "PRISM_APP_HOST"]`
  (`tests/codex.rs` asserts it). Codex README, `SKILL.md` and `docs/codex-integration.md`
  carry dated sections.
- Retired runtime directories under `.prism/run` are pruned on attach.
- `tests/attachment.rs` covers the Codex MCP path with the real binary.

## Changed after batch 7 (2026-11-10 and 2026-11-11)

All of this is in the shared bridge and `prism-core`, so Codex gets it with the new
`prism` binary; the Codex-facing parts are the tool descriptions in `src/mcp.rs` and
`plugins/codex/skills/prism/SKILL.md`, both already updated.

- **Shared state only** (Decision 8 in `docs/app-plan.md`): every change to documents and
  content goes through the bridge store; the app keeps only presentation.
- **Close and reopen documents**: `prism_action` takes `{"action":"close"}` and
  `{"action":"reopen"}` without `ids` (unknown fields are refused). `prism_inspect` marks
  each document with `closed`. Publishing to a closed document reopens it. Store column
  `documents.closed`, schema version unchanged; app protocol is 2.
- **Numbers never change**: every element, relationship and group keeps the position of
  its first publish (`order` on all three); a publish never reorders, new records go to the
  end. Diagram arrows are numbered (steps) in that order, and Prism always numbers them
  itself, also when a label starts with a digit.
- **Dropped items stay**: an archived item stays in place, greyed with a "dropped" badge;
  ask, accept and drop refuse it; undo restores it.
- **Ask goes straight to the conversation**: the app sends the item reference and the
  quoted text without a typed question (`prism-core/src/ask.rs`); the user types the
  question in the CLI.
- **Answers are not recorded**: answers to ordinary asks stay in the conversation and go to
  md files when relevant; Prism changes only when the agent publishes the revised item.
  Only explain requests (`intent: "explain"`, text ending with the line `Explain`) are
  recorded with `prism_answer`, which the bridge refuses for any other question.
- **Diagram rules for agents** (skill and `prism_publish` description): at most 7 boxes,
  9 arrows and 3 lanes per document; relationships listed in data-flow order; labels of at
  most 3 words without numbers; kind `text` for actors and stores (their box shows only
  number and title); a changed flow is published as a new document.
- **App**: diagram zoom is continuous with an explicit detail level, boxes look like the
  list cards, the app remembers ended and last-seen sessions across its restarts. Nothing
  Codex-specific.

## Open for the Codex agent

1. `CodexAdapter` (`src/adapters/codex.rs`, about 1,000 lines, 13 tests) is unused by the
   bridge; only `connect_unix` is still called (`src/bridge.rs`). Delete it (move
   `connect_unix` to `src/io.rs`) or reconnect it to Codex's app-server.
2. `AgentKind::label()` in `prism-core/src/model.rs` is unused.
3. No Codex tool exposes the bridge's `status` operation (`{"method":"status"}`; see
   `src/applink.rs`), so Codex has no equivalent of Claude's `/prism`.
4. Confirm that Codex passes `PRISM_APP_FILE` and `PRISM_APP_HOST` through `env_vars` into
   the MCP process.
5. Gate 1.3, Codex half: in the `d_codex` container, `cat ~/.codex/prism/app.json` must
   show the file the app wrote on the host (mode 0600).
6. The real Codex loop against the app: publish, ask from the app, drop, undo, close and
   reopen a document.
7. Ask without a typed question: for Claude the mod puts the reference into the prompt box
   and the user types there. Codex gets shared questions only through `prism_questions`
   (`src/adapters/codex.rs` never fills a prompt). Decide how a reference-only ask reaches
   the Codex user: for example the skill tells Codex to call `prism_questions` when the
   user says they asked from Prism, or a Codex-side way to place the reference in the
   prompt.
8. Restart Codex after installing the new binary so it loads the new tool schema
   (`prism_action` close/reopen) and skill text.
9. Explain (`intent: "explain"` on a shared question; the app shows the explanation the
   agent records with `prism_answer`) needs the Codex-side delivery of shared questions
   from point 7: until then the Codex user has to ask Codex to retrieve it.
