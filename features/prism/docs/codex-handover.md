# Codex handover (desktop app, batches 3-7)

Written 2026-11-10 for the Codex agent. The Claude side is done; these points are
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
6. The real Codex loop against the app: publish, ask from the app, answer, drop, undo.
