# Claude native integration gates

Observed 2026-11-08 on Linux arm64. CLI and `$.session.version()` both report
2.1.319; runtime build timestamp is `2026-11-05T18:14:18Z`.

## Reproduce

```sh
npm install --global --prefix "$HOME/.local" @anthropic-ai/claude-code@2.1.319
python3 features/prism/tests/probe_claude.py --claude "$HOME/.local/bin/claude"
```

The official npm package works here without running its optional postinstall.
The probe creates a private temporary project, `CLAUDE_CONFIG_DIR`, plugin, and
Unix socket; supplies a fresh explicit session UUID; strips credentials from the
child environment; and makes no model request. It does not read or change existing
conversations or user settings. It removes only its own temporary artifacts.

## Runtime results

| Gate | Result | Evidence |
| --- | --- | --- |
| Native package validation | PASS | `claude plugin validate` accepts manifest and module |
| Mod activation in `--print` | PASS | Registered command executes without authentication |
| Exact conversation identity | PASS | Explicit `--session-id`, `$.session.id()`, and output `session_id` agree |
| Native tool registration/call | PASS | Tool appears in `$.tool.list()` and its native hook returns matching identity and nonce |
| User-only Unix HTTP | PASS | `$.http.fetch` reaches private mode-0600 socket; response body echoes the bound identity |
| Bootstrap credential transport | PASS, narrow | Fresh launcher environment token read with `$.env.get`; bridge rejects missing authorization |
| Session exit event | PASS, narrow | Print command exit emits `session.end` with same session ID and `reason: "other"` |
| Interactive activation and approval routing | NOT VERIFIED | Interactive startup requires login; no account credentials are available |
| Queued submission and authoritative request-to-turn correlation | NOT VERIFIED | No authenticated idle/busy model turn was run |
| Stop, clear, resume, branch, reload races and queue cancellation | NOT VERIFIED | Requires authenticated interactive tests |
| Existing-session caller/pane binding | NOT VERIFIED | Probe proves launcher-established identity only |

**Interactive sends must remain disabled until the corresponding gates pass.**
The successful local command/tool checks do not establish interactive approval
ownership, model-driven tool permission behavior, or complete live-pane binding.
An attempted ordinary prompt returned `Not logged in · Please run /login` with
`is_error: true`, `terminal_reason: "api_error"`, and zero API duration.

## Verified package and runtime shapes

The probe's package root contains `.claude-plugin/plugin.json`, `hooks/hooks.json`,
and `hooks/register.js` (the shipped package uses `hooks/prism.js`). The manifest needs `name`, with `version` and `description`
recommended. `hooks/hooks.json` uses `{"modules":["./register.js"]}`; paths are
relative to that file. The ES module exports `register(on, options)`.

Register with `$.tool.register({name, description, inputSchema, isDeferred: false})`
inside `session.start`. A plugin named `prism` registering `publish` produces
`mcp__prism__publish`; match `on('tool.call', {tool: 'mcp__prism__publish'}, ...)`.
Arguments are flattened onto `e`, alongside `tool` and `tool_use_id`. Return
`{result: JSON.stringify(value)}`. A native `$.tool.call(...)` returned both
`result` and `text` containing that string. The command API is
`$.command.register({name, description})`, with `command.run` matched by `command`;
return `{text}` to display a command result without a model turn.

The transport exercised by the probe is:

```js
const response = await $.http.fetch('http://localhost/probe', {
  method: 'POST', socketPath,
  headers: {'Content-Type': 'application/json', Authorization: 'Bearer ' + token},
  body: JSON.stringify({sessionId, nonce})
});
```

`response` exposes `status`, `ok`, `headers`, and `text`; `text` is already a
string, not a Fetch API method. Missing authorization produces a normal HTTP
response with status 401 from the probe server. No Node socket import is needed.
Native validation requires literal `$.namespace.method(...)` calls and literal
hook names; passing `$` to a top-level helper in the same module is allowed.

## Installed declarations and unverified delivery semantics

Running `/plugin-authoring` in the isolated CLI extracted its bundled declarations
even though the subsequent model request failed authentication. The inspected file
was `/tmp/claude-1000/bundled-skills/2.1.319/06a0d92269215db308863f384899965a/plugin-authoring/types/claude-code.d.ts`.
Its first line identifies **2.1.319**, superseding the plan's earlier 2.1.318
observations. The header labels this API **EARLY ACCESS**, subject to change
without notice. The file is a generated local reference, not a project dependency.

`HttpInit.socketPath` is declared. The process fallback is declared as
`$.process.run(argv, {cwd, env, stdin, timeoutMs})`, returning
`{exitCode, stdout, stderr}`; this process API is now runtime-tested by `tests/attachment.py` for on-demand
attachment from the real `/prism` command.
`HttpInit` has no cancellation signal or timeout field; do not invent one.
The bridge must keep native HTTP handlers bounded and retire stale generations.

`PromptSubmitArgs` accepts `text` but reserves `origin`, `turnId`, `wait`, and
`context` for the engine/hooks. `$.prompt.submit({text})` preserves plugin attribution.
`PromptSubmitResult` is `{text, context?, origin?}` or `{drop: string}`: there is no
request ID, queued/started discriminator, or acknowledged turn ID in this receipt.
The declarations explicitly allow resolution after queueing behind another turn.
Do not mark a delivery started from this receipt or correlate the next `turn.start`
by timing alone. Interactive delivery needs independently verified correlation.

The declarations describe `$.clock.every(ms, callback)`/`after(...)` cancellation
through the returned `cancel()` and cancellation of old waits on hot reload.
Background polling must recheck the native session identity before dispatch.
The declarations identify `session.end` on exit/clear/resume/logout and reserve a
shared 1.5-second default shutdown budget; clear changes the ID without another
`session.start`. Those switch/reload behaviors remain runtime gates.

## References

- [Official installation options](https://code.claude.com/docs/en/setup)
- [Native mod packaging and declarations](https://code.claude.com/docs/en/plugins/mods/reference)
- [Native tools, background submission and process/network API](https://code.claude.com/docs/en/plugins/mods/api)
- [Mod activation and policy](https://code.claude.com/docs/en/plugins/mods/overview)

## Interactive contracts verified 2026-11-08

Verified on the installed CLI 2.1.319 (Linux arm64) with isolated
`CLAUDE_CONFIG_DIR` sessions; the plugin in `plugins/claude/` relies on exactly
these facts.

- Session IDs are canonical UUIDs. `claude --session-id <uuid>` creates a new
  session with that ID (interactive and `-p`); reusing a persisted ID fails with
  "Session ID ... is already in use". `claude --resume <uuid>` resumes exactly
  that persisted session.
- Package: `.claude-plugin/plugin.json` `{name, version, description}`,
  `hooks/hooks.json` `{"modules": ["./prism.js"]}` (paths relative to that
  file), ES module `export function register(on, options)`; `options` is an
  empty object. `import.meta.url` is the module's `file://` URL, so sibling
  files load through `new URL('./tools.json', import.meta.url).pathname` and
  `$.fs.read(path)`.
- Native validation: every `$` use is a literal `$.noun.method(...)` call; `$`
  may only be passed to a top-level function declaration of the same module;
  hook names are literals. A skill named like the plugin (`prism`) claims
  `/prism:prism`, and `$.command.register({name: 'prism'})` is then refused, so
  the bundled skill directory and frontmatter name are `prism-workspace`.
- APIs: `$.tool.register({name, description, inputSchema, isDeferred: false})`
  inside `session.start`; plugin `prism` registering `publish` yields
  `mcp__prism__publish`, matched by `on('tool.call', {tool: 'mcp__prism__publish'}, ...)`;
  tool arguments are flattened onto `e` beside `tool` and `tool_use_id`;
  `{result: JSON.stringify(value)}` answers, `{deny: text}` refuses.
  `$.command.register({name, description})` with `on('command.run', {command: 'prism'}, ...)`
  returning `{text}` runs without a model turn. `$.session.id()`,
  `$.env.get(name)`, `$.fs.read(path)` and
  `$.http.fetch(url, {method, socketPath, headers, body})` -> `{status, ok, headers, text}`
  (`text` is a string); missing authorization is an ordinary 401 response.
- Lifecycle: `session.start` `e = {cwd, surface: 'terminal', isInteractive: true}`
  interactively (`surface: null`, `isInteractive: false` under `-p`). `/clear`
  fires `session.end` with reason `clear`, `e.sessionId` the old ID and
  `$.session.id()` still the old ID at that moment; no new `session.start`
  fires, registered commands/tools persist and later calls see the new ID.
  `/exit` -> `prompt_input_exit`; `-p` completion or SIGTERM -> `other`; also
  `resume` and `logout`.
- `$.prompt.submit` receipts carry no request or turn correlation.
- Installation: `claude plugin validate <dir>`; `plugins/claude` is both plugin
  and marketplace (`.claude-plugin/marketplace.json`, name `prism-local`, plugin
  source `./`); `claude plugin marketplace add <dir>`,
  `claude plugin install prism@prism-local`, `claude plugin list` (read from the
  directory), `claude plugin uninstall prism@prism-local`,
  `claude plugin marketplace remove prism-local`.
- End to end (`tests/attachment.py`, real CLI in print mode, no tmux, no
  authentication, no model request): `/prism` attaches through `prism attach`,
  the binding records the Claude CLI process (`server_pid`, the parent of
  `prism attach`) with its start time, the command reports the published
  documents and the app link (retrying, pairing file not found, in the test),
  and the print run's `session.end` retires the binding while the CLI process
  stays alive. The launcher-based `tests/claude_lifecycle.py` was removed with
  the launcher in batch 7 of `docs/app-plan.md`.

### Delivery gate status

| Gate | Status |
| --- | --- |
| Share + same-session retrieval (`mcp__prism__questions`) | ENABLED |
| Start / Steer / Queue | DISABLED: the prompt receipt lacks request-to-turn correlation |
| Interactive approval ownership, busy-queue cancellation | NOT VERIFIED (authenticated interactive runs required) |
