# Codex native integration evidence

Verified 2026-11-08 on Linux 6.12.76, aarch64, Debian 12, Python 3.11,
Codex CLI/app-server 0.167.0, tmux 3.3a. `tests/probe_codex.py` creates
disposable private Codex homes, app-servers and conversations. It never connects
to the user's existing app-server or changes the user's configuration.

## Reproduce

Install `websockets==15.0.1` in a Python virtual environment, then run:

```sh
python3 tests/probe_codex.py
python3 tests/probe_codex.py --model-turns --model gpt-6-astra --auth-file /absolute/path/to/auth.json
python3 tests/probe_codex.py --tui-approval --model gpt-6-astra --auth-file /absolute/path/to/auth.json
python3 tests/probe_codex.py --tui-command --tui-switch --model gpt-6-astra --auth-file /absolute/path/to/auth.json
```

The default run makes no model requests. The model run makes one new bounded
turn and steers that same turn. Its simulated CLI accepts only that turn's
`prism_probe.inspect_caller` approval once; the simulated Prism connection never
answers requests. The TUI run instead prints its isolated tmux socket name;
inspect that TUI and choose the one-time **Allow** for `inspect_caller` there.
Choose **Continue without trusting** at the probe plugin's hook trust screen.
Neither RPC client answers approvals in TUI mode. The 120-second deadline
interrupts the probe's own turn. Servers, copied credentials and temporary state
are removed afterward. Exit 1 indicates at least one failed contract check;
`untestable` does not grant a capability. The expected empty-thread resume
rejection passes its negative check while recording `supported: false`; other
resume errors fail the check. Baseline and actual-TUI runs both exited 0 after
the review corrections below.

Run the probe regressions from `features/prism/`:

```sh
python3 -m unittest discover -s tests -p probe_codex_test.py
```

Ten tests cover unrelated thread/turn/server/tool results, mismatched native turn
metadata, missing approval/request resolution/assistant evidence, prompt echoes,
expected versus unexpected resume failures, and cleanup after tmux timeout or
client-close failure. Nine assertions failed before the fixes; all ten tests now
pass. Cleanup attempts every client and always terminates/reaps the app-server,
even when an earlier cleanup operation fails.

## Observed contracts

| Check | Result | Evidence and implication |
| --- | --- | --- |
| Native transport | PASS | `codex app-server --listen unix://PATH`, WebSocket-framed JSON messages; socket mode 0600 inside a 0700 temporary directory. The CLI labels app-server experimental. |
| Actual server version | PASS | `initialize.result.userAgent` included `prism_probe_cli/0.167.0`; no dedicated version field exists. `thread.cliVersion` is creator provenance, not current server identity. |
| Server process | PASS | `server/diagnostics.result.process.id` matched the parent PID of the native MCP children. Record OS process start time separately for production restart protection. |
| Exact model caller | PASS | A real model-called `tools/call` supplied native `_meta.threadId`, `_meta.sessionId`, `callId`, `itemId`, `windowId`, `progressToken`, and `x-codex-turn-metadata`. The latter object included `thread_id`, `session_id`, `turn_id`. Exact thread and acknowledged turn matched. |
| Explicit client tool call | PASS | `mcpServer/tool/call` with `threadId`, `server`, `tool`, `arguments`, and **no** client `_meta` injected `threadId` and `progressToken`. This alone is not model-call evidence; the separate real-model check above supplies that evidence. |
| MCP process ownership | PASS | Separate tested threads had distinct MCP child PIDs with the same app-server parent. `CODEX_THREAD_ID`, `CODEX_SESSION_ID`, `TMUX`, and `TMUX_PANE` were absent when removed from the app-server's inherited environment. |
| Bootstrap environment | PASS | `mcp_servers.prism_probe.env_vars = ["PRISM_PROBE_BOOTSTRAP"]` forwarded a fresh server environment credential into the MCP child; the report records only presence, never its value. |
| Pane identity from metadata | UNTESTABLE | Observed `windowId` was `<thread UUID>:0`, not a tmux pane. No verified native metadata-to-TUI PID/pane mapping exists. Use a trusted launcher binding. |
| Fresh empty thread resume | UNSUPPORTED; negative check PASS | A second client `thread/resume` immediately after `thread/start` failed with the expected `-32600: no rollout found for thread id ...`. Do not build a launcher that assumes an empty thread is resumable; unexpected errors still fail validation. |
| Resume persisted exact thread | PASS | `thread/resume` succeeded after `turn/start` persisted the thread. Both clients then received correlated item/completion events. A client attaching after `turn/started` must use resume state; it did not receive a replayed start event. |
| Idle steering precondition | PASS | `turn/steer` on the idle thread failed with `-32600: no active turn to steer`. |
| Busy steering | PASS | `turn/steer` with the active `expectedTurnId` acknowledged the same `turnId`; the original model turn completed. |
| Turn acknowledgement/events | PASS | `turn/start.result.turn.id` matched `turn/started.params.turn.id` and `turn/completed.params.turn.id`, with matching `params.threadId`. |
| MCP approval in actual CLI | PASS after corrected rerun | An isolated `codex --remote unix://PATH resume THREAD_ID` TUI displayed the exact `inspect_caller` approval while both RPC clients never replied. The probe matched its request/resolution, successful tool result with exact thread/turn/server/tool and native turn metadata, completed turn, native assistant message and standalone rendered `PROBE_DONE` line. Selecting **Allow** once completed that correlated sequence. |
| Command approval in actual CLI | PASS | A real TUI displayed the exact `printf prism-probe > PROBE_OWNED_FILE` command. One-time approval produced correlated request/resolution, command exit 0, expected file contents, native assistant `COMMAND_DONE`, and a standalone rendered reply. Both RPC clients never replied. |
| Other approval classes | UNTESTABLE | File/permission approvals, denial, reconnect replay and competing replies have not been exercised. Prism must never reply to any approval request. |
| Fork identity | PASS | The tested ordinary fork had a different `thread.id` and a different `sessionId`. The schema describes session-tree sharing, so neither equality nor inequality of session IDs is a routing guarantee. |
| Native package installation | PASS | A temporary `.codex-plugin/plugin.json` with bundled `hooks/hooks.json` installed through `codex plugin marketplace add PATH --json` then `codex plugin add prism-probe@prism-probe-local --json`. |
| Hook trust/activation | UNTESTABLE | The installed hook did not execute without trust. The actual TUI displayed **Hooks need review**; continuing without trust left it disabled. No hook-trust bypass was used. |
| Switch to a new thread after input | PASS | After `/new` and a new prompt, both observer clients received `thread/started` for the replacement thread. The adapter retires on this event. |
| Immediate TUI focus after `/new` | UNTESTABLE | `/new` alone produced no observer lifecycle event before the next input. This does not prove immediate invalidation when the TUI changes focus; native sends remain disabled. |
| Queue/other lifecycle races | UNTESTABLE | Queue cancellation, resume/clear/reload invalidation and reconnect outcome reconciliation require further native checks. Queue mode remains disabled. |

The initial 40-second model runs stopped at an unanswered MCP approval; they
were not network/authentication/model failures. Answering only the harmless probe
approval allowed successful completion. Do not classify an unanswered approval
as missing native caller metadata.

## Supported integration boundary

Use a private per-binding app-server with a launcher-owned Unix socket and
trusted registration of the exact CLI pane. The launcher records tmux server
socket/PID/start time, pane identity/generation and app-server PID/start time.
Launch the actual TUI normally on that private server for a new conversation;
the first native MCP call supplies `_meta.threadId` for provisional binding.
For resume, require an explicit native thread ID and reject any other caller.
The bridge's token authenticates the MCP process; the native metadata identifies
its calling thread. Model tool arguments must never override either.

An isolated app-server can forward a launcher secret without placing it in
model arguments or the process command line:

```toml
[mcp_servers.prism]
command = "/absolute/path/to/prism"
args = ["mcp"]
env_vars = ["PRISM_BRIDGE_SOCKET", "PRISM_BINDING_TOKEN"]
```

The exact `env_vars` mechanism was tested with `PRISM_PROBE_BOOTSTRAP`; the
production variable names above define the integration boundary, not existing
implemented adapter fields. Put values in the owned app-server environment.
Restrict registration to this server's authenticated MCP process and the recorded
pane; filesystem/ancestry checks alone cannot bind an arbitrary existing CLI.
An existing unregistered CLI must receive an explicit launcher/resume instruction.
`windowId`, inherited tmux variables, workspace directory and model-provided IDs
cannot establish its pane.

`TurnStartParams` and `TurnSteerParams` support `clientUserMessageId`; the latter
requires `expectedTurnId`. `turn/start` may steer an active turn according to the
installed schema; that particular race is not yet runtime-verified. Preserve
unknown outcomes and never automatically resend on timeout. Observing an
acknowledged turn ID or an active thread does not prove all queued input started.

## Interface sources

The installed CLI supplied `--help`, `app-server generate-json-schema
--experimental`, `v1/InitializeResponse.json`, `v2/ThreadStartParams.json`,
`ThreadResumeParams.json`, `TurnStartParams.json`, `TurnSteerParams.json`,
`McpServerToolCallParams.json`, and `McpServerElicitationRequestParams.json`.
Generate them into a temporary directory when validating a different version.

The [official plugin packaging documentation](https://developers.openai.com/plugins/build/plugins)
describes `.codex-plugin/plugin.json` compatibility manifests, local marketplace
catalogs and hook trust; the installed CLI workflow above was exercised directly.
Package presence never establishes hook trust or pane authority.

## Implementation handover

`src/adapters/mod.rs` defines `AgentAdapter`, `Capabilities` and `NativeEvent`.
`src/adapters/codex.rs` implements bounded Unix WebSocket RPC, per-connection
initialization, exact-thread resume, native event filtering, approval observation
without replies, conservative retirement, generation checks, duplicate request
protection, and unknown delivery without automatic retries. Accepted input stays
`Submitted`; an acknowledgement does not claim that input started execution.
The public constructor has no capability bypass: start/steer/queue remain gated.
The private send engine is exercised only through adapter unit tests.

The shipped `plugins/codex/` package installed successfully using the actual
Codex CLI in a disposable home. Its `.mcp.json` `env_vars` shape was also verified
with a native-installed probe MCP process. No user configuration was modified.

At the requested handover, six adapter unit tests and the plugin package test
had passed. Two additional unit tests for other-thread retirement and stale
generation/changed-payload replay were saved afterward and have not yet been
run or formatted. The ten Python regressions passed before the latest command,
switch and plugin-MCP probe extensions; rerun them after resuming work.

Use the prepared toolchain environment for the remaining checks:

```sh
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CC="$HOME/.local/bin/prism-zig-cc"
export AR="$HOME/.local/bin/prism-zig-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$HOME/.local/bin/prism-zig-cc"
cargo test --manifest-path features/prism/Cargo.toml --lib adapters
cargo test --manifest-path features/prism/Cargo.toml --test codex
python3 -m unittest discover -s features/prism/tests -p probe_codex_test.py
```

An independent review of the final adapter/plugin implementation is still
required. The earlier review covered the original probe corrections, not these
new adapter files or probe extensions. No commit was created by this agent.
