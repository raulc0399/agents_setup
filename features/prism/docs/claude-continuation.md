# Claude integration record and continuation notes

> Superseded 2026-11-10 by batch 7 of `docs/app-plan.md`: the terminal UI, the
> tmux pane handling and the `prism --agent` launcher described here are removed;
> the Prism desktop app shows the workspace. Current state: `README.md`,
> `plugins/claude/README.md` and `docs/validation.md`.

Written 2026-11-08, completed 2026-11-09. The implementation session survived
four container restarts; this file is the durable record of the approved plan,
the verified Claude Code facts and the delivery state. The shared product and
Codex source baseline remains `f476550` (docs commit `9afa5b4`).

## What a container restart destroys

Only the repository mount `/workspace/agents_setup` survives. Each restart wiped
`$HOME/.cargo`, `$HOME/.local/opt`, the `prism-zig-*` wrappers, `/tmp` (scratch
files, isolated `CLAUDE_CONFIG_DIR`s, the Python venv) and all background agents.
The Claude CLI (`$HOME/.local/bin/claude`, 2.1.319), its login, tmux 3.3a and
`uv` survived. Reinstall the toolchain first (about five minutes) exactly as in
`docs/development-setup.md`, then:

```sh
cd /workspace/agents_setup/features/prism
. "$HOME/.cargo/env"
export CC="$HOME/.local/bin/prism-zig-cc" AR="$HOME/.local/bin/prism-zig-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$HOME/.local/bin/prism-zig-cc"
python3 -m venv /tmp/prism-validation-venv && /tmp/prism-validation-venv/bin/pip install websockets==15.0.1
```

Keep long builds and tests in background tasks and keep scratch files inside
`features/prism/target/tmp/` if the session is unstable. `git` may need
`git config --global --add safe.directory /workspace/agents_setup` after a restart.

## Approved plan and decisions (user approved "go all")

- Claude launch path: `prism --agent claude [--resume UUID]`; the supervisor
  spawns no server, pre-binds the conversation (fresh UUID or explicit resume),
  and the tmux pane runs `prism native-cli --claude PATH [--claude-config-dir D]
  [--resume-conversation]`, which execs `claude --session-id UUID` or
  `claude --resume UUID` with `PRISM_BINDING_FILE` in its environment.
- Binding identity: `LiveBinding::new(project, agent, endpoint, pid, tmux, cli)`;
  for Claude the recorded native process is the CLI pane process (same PID and
  birth after exec), endpoint `claude-cli://<pane_id>`. The bridge never
  SIGTERMs a Claude process; `/clear`, `/resume`, `/logout` retire the binding
  but the CLI pane stays open (Codex behavior unchanged).
- Bridge glue: `Operation::Lifecycle(LifecycleEvent::SessionEnd { reason })`,
  wire `{"method":"lifecycle","params":{"event":"session_end","reason":"clear"}}`,
  accepted only for Claude bindings and refused before any binding state is
  persisted for other agents; it retires the binding through `ClaudeAdapter`
  and invalidates the generation while preserving Share records. `poll_native`
  dispatches by `binding.agent` via `NativeObserver { Codex, Claude }`;
  messages name the bound agent.
- Native Claude Start/Steer/Queue stay disabled: `$.prompt.submit` returns no
  request or turn ID, so no authoritative request-to-turn correlation exists.
  Share plus same-session retrieval (`mcp__prism__questions`, `/prism`) is the
  supported workflow. Codex gates unchanged.
- Real-model loop: a 0600 copy of `~/.claude/.credentials.json` plus the
  onboarding fields of `~/.claude.json` in a temporary isolated
  `CLAUDE_CONFIG_DIR`, deleted afterwards; consumes Claude usage. The shipped
  launcher never touches credentials.

## Verified Claude CLI 2.1.319 facts (runtime-checked)

- Session IDs are canonical UUIDs. `--session-id <uuid>` creates a new session
  (interactive and `-p`); reusing a persisted ID fails ("Session ID ... is
  already in use"). `--resume <uuid>` resumes exactly that session; unknown IDs
  fail ("No conversation found with session ID").
- Package: `.claude-plugin/plugin.json`, `hooks/hooks.json` `{"modules":[...]}`,
  ES module `export function register(on, options)`; `options` is empty;
  `import.meta.url` works. Native validation requires literal `$.noun.method()`
  calls; `$` may only be passed to top-level function declarations of the same
  module (nested helpers fail to load). Hook names must be literals. A skill
  named like the plugin (`prism`) claims `/prism:prism` and the mod's `/prism`
  command is then refused, so the bundled skill is `prism-workspace`.
- APIs: `$.tool.register` only in `session.start`; plugin `prism` + tool
  `publish` = `mcp__prism__publish`; tool arguments flattened onto `e` with
  `tool`, `tool_use_id`; return `{result: string}` or `{deny: text}`.
  `$.command.register` + `command.run` returning `{text}` runs no model turn.
  `$.http.fetch(url, {method, socketPath, headers, body})` returns
  `{status, ok, headers, text}`; missing auth gives a normal 401.
- Lifecycle: `session.start` `{cwd, surface:'terminal', isInteractive:true}`;
  `/clear` fires `session.end` reason `clear` with the old `sessionId` while
  `$.session.id()` is still the old ID; no new `session.start`; registered
  tools/commands persist; later calls see the new ID. `/exit` gives
  `prompt_input_exit`; `-p` completion/SIGTERM gives `other`.
- Installation: `claude plugin validate DIR`; `plugins/claude` is both plugin and
  marketplace (`.claude-plugin/marketplace.json`, name `prism-local`, source
  `./`); `claude plugin marketplace add DIR`; `claude plugin install
  prism@prism-local`; installed mods activate without `--plugin-dir`. Fresh
  projects show a folder-trust prompt; a fresh config defaults to auto
  permission mode.
- `tests/probe_claude.py` passes on 2.1.319 (all eight checks).

## Delivered files

Shared integration: `src/main.rs` (Agent::Claude, `--claude`,
`--claude-config-dir`, `--resume-conversation`, `start_codex_server`,
`exec_claude`, pre-binding, Codex-only cleanup), `src/bridge.rs`
(`Operation::Lifecycle`, `NativeObserver`, `observe_lifecycle`, retained
retirement reason, agent-labelled messages), `src/session.rs` (`LiveBinding::new`
agent parameter), `src/model.rs` (`AgentKind::label/cli_name`),
`src/adapters/mod.rs`, `tests/bridge.rs`, `tests/core.rs`, `tests/launcher.rs`,
`tests/session.rs`.

Claude feature: `src/adapters/claude.rs`, `plugins/claude/` (`.claude-plugin/`
`plugin.json` and `marketplace.json`, `hooks/hooks.json`, `hooks/prism.js`,
`hooks/tools.json`, `skills/prism-workspace/SKILL.md`, `README.md`), `tests/claude.rs`,
`tests/claude_lifecycle.py`, `docs/claude-integration.md`, `tests/probe_claude.py`.

Documentation: `README.md`, `IMPLEMENTATION.md`, `HANDOVER.md`,
`docs/validation.md`, `docs/claude-handover.md` (status note), this file.

## Validation state

116 Rust tests, formatting, Clippy with warnings denied, release build, 16 Codex
probe regressions, 9 Codex lifecycle tests and 4 Claude lifecycle tests pass.
The real authenticated Claude loop (publish, pane, actions, stable-ID revision,
Share, same-session retrieval, `/clear` retirement with the pane kept, exact
resume, duplicate rejection, simultaneous sessions) passed; see
`docs/validation.md`. Two independent read-only reviews were performed; the
shared-integration findings were fixed by the integrator (see
`IMPLEMENTATION.md`).

## Not verified

Interactive approval ownership in default (non-auto) permission mode and
busy-queue cancellation; both gate only the disabled native send modes. Mod hot
reload keeps the same session ID and binding and was not exercised.
