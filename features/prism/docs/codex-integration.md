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
python3 tests/probe_codex.py --child-threads --model gpt-6.1-sol --auth-file /absolute/path/to/auth.json
```

The default run makes no model requests. `--child-threads` is a separate opt-in
model probe: it persists a parent with a READY turn, resumes both observers before
spawning one child, and verifies native parent/source/root-turn identity before
the first client accepts one exact harmless plugin tool approval. The second
observer answers none. It has bounded waits and the same isolated-home/server
cleanup as the other modes; do not combine it with model/TUI probe flags.
The model run makes one new bounded
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

Twenty-one tests cover unrelated thread/turn/server/tool results, native metadata
consistency, missing approval/request resolution/assistant evidence, prompt
echoes, expected versus unexpected resume failures, cleanup failures, and Linux
process ownership including direct exec, wrapper descendants and reused PIDs.
They also require exact plugin discovery and reject mismatched child parent,
spawn source, root turn, tool result and caller metadata.
Regression checks rejected the previous session/environment-based caller match
and positive-PID-only server check before their fixes. Cleanup attempts every
client and always terminates/reaps the launched app-server process.

The corrected no-model probe was rerun on 2026-11-08 and exited 0. Its reported
server PID and process birth matched the launched child; both thread MCP
processes and the installed plugin MCP process had that exact live server as
parent. Exact caller checks now require native top-level `threadId`; nested
`x-codex-turn-metadata.thread_id`, when supplied, must agree. Session IDs,
environment variables and arbitrary nested metadata cannot satisfy that check.
Earlier real-model/TUI observations below remain historical evidence; this
no-model rerun does not re-verify those capabilities.

## Observed contracts

| Check | Result | Evidence and implication |
| --- | --- | --- |
| Native transport | PASS | `codex app-server --listen unix://PATH`, WebSocket-framed JSON messages; socket mode 0600 inside a 0700 temporary directory. The CLI labels app-server experimental. |
| Actual server version | PASS | `initialize.result.userAgent` included `prism_probe_cli/0.167.0`; no dedicated version field exists. `thread.cliVersion` is creator provenance, not current server identity. |
| Server process | PASS, corrected rerun | `server/diagnostics.result.process.id` matched the owned launcher process and its Linux birth identity; the probe also accepts verified live descendants for npm launchers that spawn rather than exec. A positive PID alone cannot pass. |
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
| Switch to a new thread after input | PASS | After `/new` and a new prompt, both observer clients received `thread/started` for the replacement thread. The adapter retires its observation on this event; the bridge reconnects to its original bound thread without terminating the CLI. This does not verify current TUI focus. |
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
env_vars = ["PRISM_BINDING_FILE"]
```

The `env_vars` mechanism was tested with `PRISM_PROBE_BOOTSTRAP`; the shipped
plugin forwards `PRISM_BINDING_FILE`. The launcher puts this private binding-file
path in the owned app-server environment; secrets remain inside the 0600 file.
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
without replies, observer retirement, generation checks, duplicate request
protection, and unknown delivery without automatic retries. Accepted input stays
`Submitted`; an acknowledgement does not claim that input started execution.
The public constructor has no capability bypass: start/steer/queue remain gated.
The private send engine is exercised only through adapter unit tests.

### Review fixes: Codex Batch A (2026-11-09)

Observer connection/RPC/poll failures, socket closure, another thread starting,
and `thread/unsubscribed` disconnect observation without terminating the private
app-server or Codex TUI. The bridge retries the same bound thread with exponential
backoff from 1 to 30 seconds. One background connection worker keeps bounded
handshake/resume calls from blocking bridge health and pane lifecycle checks.
Reconnection rechecks the reported app-server PID; it never rebinds the workspace
or retries a user input. `thread/closed` for the bound thread still retires the
binding, including closure during resume when the reported server PID matches.

The UI reports observer disconnection while launcher readiness uses authenticated
bridge health independently. A successful observer connection logs the bounded,
escaped `initialize.userAgent` in `.prism/run/<uuid>/supervisor.log`.
Owned app-server shutdown sends SIGTERM, waits up to two seconds, then escalates
to SIGKILL and reaps the child if it has not exited.

The isolated no-model probe on installed `codex-cli 0.167.0` passed again,
including explicit observation of `plugin_probe` discovery and its process
ownership. That no-model run did not verify child-thread plugin inheritance or event delivery,
current TUI focus, model-turn approval ownership, or native send safety.
Start/Steer/Queue remain disabled. Fake-server regressions establish the recovery
behavior above; they do not establish new native Codex event contracts.

Batch A validation: 135 Rust tests, 16 Python probe regressions, nine Codex and
four Claude lifecycle tests, rustfmt, Clippy with warnings denied, build and
development prerequisites passed. Three additional isolated launcher checks
confirmed failed-handshake recovery, unrelated-thread session preservation and
closure during resume. Independent read-only review approved the corrected
closure-during-resume behavior after its regression failed before the fix.

### Review fixes: Codex Batch B (2026-11-09)

MCP drains oversized input lines and returns a bounded JSON-RPC error before
serving the next line; oversized responses also produce an error without exiting.
A caller from another thread receives `isError` before forwarding. The bridge
rechecks Codex caller identity in its serialized handler so concurrent first
callers cannot retire the thread that bound first. This does not establish which
native caller arrives first or verify subagent plugin inheritance.

Under its exclusive lease, the bridge replaces only an owned private socket
whose nonblocking connection attempt reports connection refused. A live or full
listener, symlink, or unexpected file is refused. Duplicate launch still refuses
a verified live CLI/native process when health fails, and names the existing
binding for bridge recovery; health failure never authorizes a duplicate session.

The launcher checks socket-path length before creating a runtime, removes its
new runtime on pre-spawn failure, and prunes retired runtimes under the launch
lock only when their native process, CLI and UI have exited and their bridge
lease is available. Runtimes with a live disconnected UI remain available.
The attached launcher retains and reaps its supervisor child.

HTTP headers have a 200 ms deadline, reducing idle-client stalls. Body reads and
response writes retain their bounded 15-second deadline; request handling is
still serialized, so this is not full concurrent-client isolation.

Batch B validation: 141 Rust tests, rustfmt, Clippy with warnings denied, build,
11 Codex launcher tests and four Claude lifecycle tests passed. Independent
read-only review approved the changes, including a concurrent-caller reproduction
that preserves the first binding and a runtime retained for a live UI.

### Review fixes: Codex Batch C (2026-11-09)

The probe now fails if `plugin_probe` is absent from native discovery; a similar
name cannot pass. Its process ownership check still runs when the exact plugin
is present. The Codex skill directs subagents/forks to return findings to the
bound parent rather than reuse its binding or retry a foreign-thread refusal.

The maintained `--child-threads` probe passed on installed `codex-cli 0.167.0`
with `gpt-6.1-sol`. Both observers subscribed to a persisted parent before the
spawn turn. Native `thread/read` linked the child through `parentThreadId`,
`source.subAgent.thread_spawn.parent_thread_id` and its turn's `rootTurnId`.
Both observers received the child's successful `plugin_probe.inspect_caller`
result and completed turn with matching native top-level/nested thread IDs and
nested turn ID. The exploratory full result also showed a shared parent/child
session ID and separate MCP process IDs: plugin access does not imply process
identity, and session ID cannot authorize the caller.

Neither subscribed observer captured child `thread/started` through parent-turn
completion in the successful probes. This is a bounded observation, not a claim
that child creation can never emit it. One exact approval was answered by the
simulated CLI; the second observer answered none. Other approval classes, current
TUI focus and native Start/Steer/Queue remain unverified and disabled.

Validation: 21 Python probe tests, 141 Rust tests, rustfmt and Clippy with
warnings denied passed. The default no-model and maintained child probes exited
0. The missing-plugin regression failed before its fix. Independent review also
identified a delayed-observer race; its regression failed before the probe was
changed to wait for both clients' correlated evidence within the existing deadline.

Remaining decisions: full concurrent-client isolation beyond the shorter header
deadline; optional portability of existing `/usr/bin/tmux`/`cat` test fixtures;
and tailored launcher guidance for an unregistered thread with no persisted
rollout (an empty thread cannot necessarily be resumed). Formal revised review
requires restoring `.reviews/prism-impl-2026-11-09/run.db` and its exports; the
worker handoff currently reports that the database does not exist.

The shipped `plugins/codex/` package installed successfully using the actual
Codex CLI in a disposable home. Its `.mcp.json` `env_vars` shape was also verified
with a native-installed probe MCP process. No user configuration was modified.

Thirteen adapter unit tests now pass, including fragmented handshake, fragmented
RPC response, slow event frames, a full Unix listener backlog, and slow readers. The previous 60ms RPC could take
853ms when bytes arrived continuously; reads and writes now enforce the same
absolute operation deadline beneath WebSocket framing. Unix connections fail
promptly when the listener queue is full. Shared nonblocking writes and deadline
polling prevent a slow recipient from extending bridge or native writes. Native Start, Steer and
Queue remain disabled, with no public bypass.

The bridge lists document and shared-question summaries in pages of at most 100.
Follow `next_cursor` using the optional `cursor` tool argument until it is null;
`total` counts the current collection. Question summaries cap text at 512 Unicode
characters and expose `text_truncated`; retrieve the full text and snapshot by
`request_id`. Pages also account for encoded summary bytes, including outcome reasons, so
JSON escaping cannot make later IDs inaccessible.
Cursor order is stable UUID order, not
creation order. Listings are live: restart from the beginning to include new
records inserted before an earlier cursor. Individual retrieval uses only
`document_id` or `request_id`, without a cursor. Private views and drafts remain
excluded from both listing and individual retrieval.

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

Independent review identified the pagination, fragmented-frame deadline and
probe-attribution defects addressed above; final integration review and release
evidence are recorded in `../IMPLEMENTATION.md`.

### On-demand attachment (2026-11-09)

The launcher requirement above describes the earlier implementation. The native
plugin now forwards `TMUX` and `TMUX_PANE` and checks the MCP process ancestry
against the exact pane before creating an attachment. `_meta.threadId` still
supplies the native caller identity; no pane or credential comes from model
arguments. An isolated ordinary Codex 0.167.0 TUI successfully opened the Prism
pane through `prism_inspect`, preserving its original conversation and pane.
See `docs/validation.md` and `tests/attachment.py` for the checks and limitations.
