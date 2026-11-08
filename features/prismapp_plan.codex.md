# Prism: implementation plan for Claude Code and Codex CLI

Status: planning only; saving this document does not authorize implementation.

## Purpose

Prism helps the user visualize, explore, and understand things discussed with Claude Code or Codex CLI, then bring selections, questions, and decisions back into the same conversation.

The product is a visual workspace. Its model starts with content, relationships, views, and discussion; it has no dependency on a tracker, `review-items.md`, or a predefined task lifecycle.

The interaction loop is: conversation → visual content → exploration and clarification → conversation → revised visual content.

## Initial capabilities

- Show lists from the conversation, including bugs, review findings, plans, questions, and explanations.
- Create diagrams for data flow, logic, plans, steps, and other relationships described in the conversation.
- Select individual elements, connections, groups, or a whole view to accept, remove, or discuss.
- Ask for clarification, alternatives, expansion, or changes with the selected content included as context.
- Open multiple views of the same underlying content or different content from the same conversation.
- Keep shared content consistent across views while allowing each view its own layout, filter, focus, and selection.
- Support later content types and actions through small internal interfaces for rendering and interaction.

## Where Prism appears

- In tmux, the first Prism tool call opens a pane to the right of the bound coding CLI pane; later calls reuse that live Prism pane.
- The Claude Code or Codex CLI conversation stays on the left; the shared Prism terminal UI appears on the right.
- Outside tmux, `prism --agent claude` or `prism --agent codex` starts a tmux session with that layout and launches both interfaces. Inside tmux, use the current window only after establishing the binding, or show the launch/resume instruction. Reuse an existing session/pane only for the exact CLI, conversation and server binding; never start a duplicate CLI or adopt by workspace alone.
- An existing conversation is resumed only with an explicit agent and native conversation ID. Opening Prism must not silently create or fork a different conversation.
- An existing CLI session needs a verified conversation/server/pane binding. Inherited `TMUX_PANE` and process ancestry are supporting evidence, not authority: a shared app-server can spawn MCP outside the calling CLI's process tree.
- Revalidate pane identity and layout before reuse; pane IDs alone do not survive tmux server restarts. Batch 1 verifies a server identity using socket path, server PID/start time and pane binding/generation tags, together with live pane and layout checks. A missing or ambiguous binding produces a launch/resume instruction, never a guessed split.
- CLI exit, conversation switching, plugin reload, or lost connection retires the live binding. Keep persisted content available and show disconnected/resume status rather than deleting it.

## Proposed architecture

- Implement one Rust application in `features/prism/`, using Ratatui and Crossterm for terminal rendering and input. Keep one crate with explicit modules; a Cargo workspace is not required.
- Use one binary with launcher, `prism ui`, `prism mcp`, and `prism bridge` entry points. `bridge` exposes the shared operations and delivery exchange needed by CLI-native extensions; it is not another agent runtime.
- Keep documents, storage, list/diagram rendering, selection, acceptance, archive/undo, question composition, and delivery tracking in common code. Both adapters call the same feature operations.
- Define `AgentAdapter` around binding, capability discovery, question submission, native events, and disconnect/recovery. Keep native method names, transport details, and lifecycle differences inside the corresponding adapter.
- Ship separate Codex and Claude extension packages. They are installed and loaded by their respective CLI; Prism does not load CLI plugins or duplicate its feature implementation inside them.
- Use native features where they help: Codex app-server events and verified steering; Claude in-process tools, commands, session identity, and queued prompt submission. The UI exposes the capabilities actually available for its current binding.
- Persist documents, view state, and delivery records in project-local `.prism/state.db`. Key content by a Prism-owned workspace/document ID with a unique project, agent kind and exact native conversation/thread association; keep live pane/server binding separate. Reconnecting the same conversation preserves its document IDs; a different conversation selects a different workspace.
- Keep one conversation per Prism workspace. Two agents in one project have separate bindings; cross-conversation content sharing is outside this release.

### Proposed source layout

| Path under `features/prism/` | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Reproducible dependencies and toolchain |
| `src/main.rs` | Command routing and startup |
| `src/session.rs` | tmux launch, live pane identity, bindings, cleanup |
| `src/model.rs` | Documents, elements, relationships, groups, views, revisions |
| `src/store.rs` | SQLite persistence, transactions, conflicts, undo, delivery records |
| `src/actions.rs` | Common publication, inspection, content actions, question preparation |
| `src/adapters/mod.rs` | Shared adapter contract, capabilities, normalized outcomes |
| `src/adapters/codex.rs` | Codex app-server protocol, thread events, steering and recovery |
| `src/adapters/claude.rs` | Claude native bridge protocol, session events and delivery outcomes |
| `src/bridge.rs` | Validated local exchange between common operations and CLI extensions |
| `src/mcp.rs` | MCP transport and schemas delegating to shared actions |
| `src/ui/mod.rs` | Event loop, navigation, views, shared actions and question editor |
| `src/ui/list.rs` | List rendering and selection |
| `src/ui/diagram.rs` | Diagram layout, rendering, and hit detection |
| `plugins/codex/` | Codex-native manifest, skill, MCP configuration and optional hooks |
| `plugins/claude/` | Claude-native manifest, mod module, commands/tools and supporting skill |
| `tests/` | Shared model/storage/UI checks and separate adapter contracts/integration checks |
| `README.md` | Installation, CLI package setup, controls, compatibility and recovery |

### Process ownership

- Run one standalone `prism bridge` supervisor per live CLI binding. The launcher starts it before the CLI/UI in a provisional registration state; an existing CLI uses a trusted native-extension bootstrap. No live tool access or send is enabled until exact native conversation, server/CLI instance and pane identity are verified.
- The bridge owns the adapter/native connection, live token and generation in memory, and the single question dispatcher. `prism ui` and CLI-spawned Codex `prism mcp` are authenticated clients; Claude native tools use the mod bridge without requiring another MCP process. The bridge can publish before the UI pane exists.
- The bridge issues bootstrap credentials through a user-only launcher/native-extension channel, never model arguments or an unauthenticated handshake presenting only identity fields. Batch 1 verifies this channel and each agent's caller proof; ancestry and tmux fields only corroborate it. Unproven registration takes the explicit launcher/resume path.
- The bridge observes native lifecycle/connection events and CLI process/pane liveness. Switching, reload or disconnection invalidates the generation and pending work before any re-registration; CLI exit retires the binding and stops its bridge. Native queue cancellation remains a verification gate. The UI can display persisted content and disconnected status after retirement.

### Shared binding and delivery contract

- A durable binding identifies the project/workspace, agent kind, and exact native conversation/thread. A live binding adds server or CLI instance, tmux server identity, CLI/Prism pane IDs, a binding token, and a generation.
- The launcher records the selected CLI/server, exact native conversation and created/attached pane; verified native registration can establish or refresh the binding for existing sessions. Model-supplied tool arguments cannot override its agent, conversation, project, or pane. Reject stale tokens and ambiguous caller identity.
- The bridge negotiates its protocol version and capabilities. Use user-only local sockets or authenticated loopback access; reject requests outside the live binding and keep tokens out of published content/logs.
- A question contains its request ID, binding generation, document ID/revision, selected stable IDs, a frozen content snapshot, the user's text, and the selected delivery mode.
- Persist the question and its delivery state before attempting transport. Track pending, submitted/queued, started, failed/refused/dropped, and unknown outcomes; preserve the native reason and acknowledged turn ID when available.
- A local write or queue acknowledgement is not proof that a turn started. Never automatically resend an unknown result; reconcile only through authoritative native correlation, otherwise retain it for explicit user retry with the duplication risk visible. Explicit retry creates a new request ID/attempt with the current verified generation; the original unknown record remains visible.
- Recheck identity before dispatch. On clear, resume, branch, reload, or disconnect, invalidate old-generation work; never redirect it to the replacement conversation. Native queue cancellation and session-switch races are batch 1 gates.
- Keep approvals and permission policy with the coding CLI. Prism acceptance is content feedback, not permission to run tools. Enable native sends only after their approval ownership is verified.

## Communication with the coding agent

- **Chat to Prism:** the agent's native package exposes publication/inspection tools backed by the same shared Prism actions.
- **Publication:** validate the bound request and payload, then atomically commit its content revision, request ID, payload identity and response. Return stable IDs and revision.
- **Pane opening:** open/reuse the verified pane separately from the content transaction. If opening fails, report the committed revision plus the pane error; retrying the same publication returns its stored result without another revision.
- **Rendering:** refresh all affected views from committed revisions. Define bounded change polling or notifications and short transactions with bounded busy handling; concurrent UI/tool activity must not lose edits.
- **Prism to chat:** the user selects content, edits a question, and explicitly sends it through the bound adapter. Show queued, started, failed, or unknown status truthfully.
- **Inspection:** agents can inspect published content. Unsent selections and question drafts remain private from agent-facing Prism inspection/bridge APIs; explicit send/share creates the selection snapshot available to the agent. This is exposure control, not isolation from same-user filesystem access.
- **Response:** adapters translate native status/events; the coding CLI shows its conversation and the agent updates visual content through the shared publication operation.

### Codex adapter and CLI plugin

- Package native instructions/skills, MCP configuration for `prism mcp`, and optional lifecycle hooks in `plugins/codex/`. Use the installed CLI's supported manifest and installation workflow; do not implement plugin installation through experimental app-server plugin endpoints.
- Prefer a verified shared app-server and exact `thread.id`, with both Codex TUI and Prism attached. One explicit launch candidate is `codex app-server --listen unix://PATH` paired with `codex --remote unix://PATH`; a verified existing daemon is also eligible.
- Use WebSocket-framed JSON-RPC over the selected local Unix socket, initialize each connection, and attach/resume the exact thread to receive events. `thread.sessionId` is not the routing key: forked threads can share it.
- Use `turn/start` and verified `turn/steer` with `expectedTurnId` as native capabilities. Handle both idle-to-active and active-to-idle races; the installed schema allows `turn/start` to steer an already-active turn. Use bounded timeouts and refresh thread state after steering errors; reconcile actual outcomes instead of promising a new turn from a stale idle observation. A timeout never triggers automatic resend.
- Hooks can supplement lifecycle registration but are not sole routing authority: they may be disabled or untrusted, and a subagent hook may carry its parent's session ID. Probe real MCP caller context before choosing per-binding server instances or another verified native registration path.
- Test multi-client event synchronization and approval ownership before enabling sends. Prism never answers approval requests; if the CLI cannot retain actionable approvals, disable that send mode rather than inventing forwarding or answering approvals automatically.
- `codex queue --thread` is an optional, explicitly labeled queued-delivery mode only after exact-thread and acknowledgement behavior are verified. Pass the exact thread UUID through `--thread` and the bound server endpoint through `--remote`; never automatically use `--resume`, and disable this mode if that server cannot be targeted. Queue success alone is not a completed conversation turn.

### Claude adapter and CLI mod

- Package `.claude-plugin/plugin.json`, `hooks/hooks.json`, and a native hooks module under `plugins/claude/`. The mod runs inside Claude Code; it registers commands/tools, captures native session identity, observes lifecycle, and submits questions. Shared Rust operations own the product behavior.
- Use `$.session.id()` with the live binding generation and compare it with the bound identity before every dispatch. Observed documentation sends `session.end` for exit, clear, resume, branch (mapped to the resume reason) and logout; in-process switches do not rerun `session.start`, while a fresh load, enable, worker respawn or new CLI process does. Invalidate on end/reload and reacquire a verified binding. Batch 1 checks these transitions against the installed build.
- The Claude adapter baseline targets the interactive CLI. Prefer `$.http.fetch` with `socketPath` over the bridge's user-only Unix socket if the loaded build supports it; retain supported `$.process.run`/`$.process.spawn` exchange as fallback. Do not assume Node socket imports. Batch 1 verifies the chosen transport, framing, cancellation and acknowledgements against the loaded build; bundled declarations and public API documentation differ.
- Use `$.prompt.submit({ text })` from background processing for explicit user sends; do not await it inside a hook that blocks an active turn. Interpret its receipt using the verified installed-build contract: public documentation says resolution at turn start, while bundled 2.1.318 declarations permit queued resolution. A receipt alone never marks started; require authoritative request-to-turn correlation and preserve dropped/refused outcomes. Batch 1 tests this behavior. Preserve native mod attribution and show the initiating user action in the question context.
- Keep native queued submission distinct from Codex steering. Surface dropped/refused results after interruption, and do not automatically retry a question the user stopped. Test session switching while a prompt is queued before enabling busy-session submission.
- Use native commands and lightweight status indicators to open/focus Prism or show delivery state; keep lists, diagrams, content actions, and persistence in the shared application.
- Check the exact installed mod declarations and activation policy. A plugin can remain installed with its skills/MCP components while its mod is disabled; installation alone does not establish capability.
- If native submission is unavailable, offer an explicitly invoked CLI skill/command that retrieves a user-shared question into that same session. Do not silently open an SDK session or a different conversation. Channels are a documented, gated alternative for later evaluation, not the baseline; Prism does not enable channel permission relay, and Claude approvals remain in its CLI.

## Content and view rules

- Give documents, elements, relationships, groups, and views stable IDs; preserve identity across revisions and representations. Elements belong to a document; relationships reference valid endpoints, and groups identify their members.
- Define the initial element content, relationship direction/label and grouping vocabulary before storage is built. Mint IDs only for new content; updates address existing IDs.
- Separate semantic content from list ordering, diagram coordinates, filters, panel arrangement and local selection. A whole-view content action expands to currently visible IDs at the selected revision; closing a view never archives content.
- Allow diagram nodes and list entries to refer to the same element; relationship IDs remain separately selectable. Store source conversation references when available.
- Require the expected document revision for content mutations. Reject stale updates with the current revision and current state or a way to fetch it; reread and reapply intended edits rather than overwriting unrelated changes. Automatic per-element merging is not required.
- Scope publication request IDs to the durable binding and retain their committed results. Reuse with a different payload is a conflict; replay of the same request returns the stored result. Define retention before any pruning.
- Attach acceptance to shared content and its accepted content revision; show stale acceptance when that content changes. Layout/filter changes do not change semantic acceptance. Acceptance never authorizes implementation.
- Archive an element and its affected relationships consistently across views; reject references that would silently revive archived content. Archiving a relationship affects that relationship only; archiving a group affects its grouping record unless its members are explicitly selected. Hiding from one view is separate from archiving shared content.
- Undo records the affected IDs and previous states for user content actions, including prior acceptance when undoing acceptance or archive. Reject conflicting undo after intervening changes; do not overwrite agent edits or unrelated content. View-local changes stay separate.
- Preserve view-local selections/drafts through UI restart as private state; sharing occurs only on explicit action. Invalidate stale selections visibly.
- Validate payload/text/node/edge bounds and render terminal control characters safely. All actions must be keyboard-accessible; mouse interaction is an additional supported input path.

### Proposed defaults to confirm before their implementation

- Capture visual content on request rather than automatically extracting every list from chat.
- Remove archives the underlying content across views and supports undo; hiding content from one view is a separate action.
- Views referencing the same element share edits and acceptance; independently created content stays independent.
- Bind one Claude or Codex conversation per workspace, with multiple documents/views inside it.

## Implementation batches

Each batch requires implementation approval, a context check confirming applicable proposed defaults before editing, and the narrowest relevant validation afterward. Work is sequential; no implementation delegation is planned. The review teams do not own application files.

1. **Verify both native integration contracts.** Organize Claude, Codex and prerequisite setup into focused per-host substeps with narrow checks under this batch's approval. Keep probes as maintained validation artifacts or documentation. Record host, date, installed CLI and actual server versions, native schemas/declarations, plugin/mod activation, Rust toolchain, terminal and tmux capabilities. For Codex, use a supported running-server version interface and test hooks bundled in a CLI-installed plugin; do not assume a dedicated initialize version field. Establish exact native conversation/server/pane binding, trusted tool-call context and supported bridge transport for each agent. Exercise idle/busy questions, stop, switching/resuming/branching, reload, reconnect and approval ownership. Pin/setup missing prerequisites within the approved implementation batch. Record pass/fail, including the selected transport's documented experimental status, and disable unsupported modes; revise the affected design before building on failed assumptions.

2. **Build the common foundation.** Add the Rust crate, minimum content/view model, versioned SQLite schema/store, shared actions, MCP schemas and adapter/bridge contracts. Define normalized delivery modes/capabilities and outcome states here; select database concurrency settings against the required checks. Validate stable IDs, references, expected revisions, atomic request/result replay, changed-payload conflicts, archive/undo, concurrent updates, bounded inputs and delivery-state persistence. Include the commit-succeeded/pane-open-failed case. Keep native package installation in its adapter batch.

3. **Deliver the first visual conversation loop in focused sub-batches.** Validate launcher/pane creation/reuse and terminal cleanup first; then list rendering, selection, acceptance/archive/undo and the question editor against shared operations; then Codex adapter/plugin and Claude adapter/mod separately. Each sub-batch gets its own approval and narrow checks. Demonstrate chat publication → selected clarification → revision in the same CLI conversation for each agent, including disconnected/unknown delivery and session-switch races.

4. **Add interactive diagrams.** Implement deterministic layout, labels, keyboard/mouse selection, grouping, scrolling/panning and selected-content details using the foundation's model. Reuse all content actions. Test hit detection after scrolling/resizing, relationship cleanup, stale selections, bounded rendering and a diagram clarified/revised through each adapter.

5. **Add multiple views.** Provide tabs and splits inside Prism for shared or separate documents in the same bound conversation. Test synchronized content/acceptance, independent layout/selection/drafts, archive/undo conflicts across views, and restart restoration without exposing private drafts.

6. **Validate and document the release.** Run relevant Rust tests, formatting, lint and builds, plus each native package's supported validation. Exercise real Claude and Codex loops with tmux. Document installation/removal, binary and plugin versions, trust/activation, explicit resume, capabilities, controls, local diagnostics, dependency licensing and recovery. Version the database schema and preserve existing state during upgrades; removal of persisted content requires a separate explicit user action. Confirm new renderers/actions reuse common model and communication contracts.

## Completion criteria

- With each supported CLI in a launcher-bound or successfully registered session, a visual request opens/reuses the live Prism pane adjacent to the verified calling pane; simultaneous sessions and tmux restarts cannot redirect it.
- Lists and diagrams support selection, revision-specific acceptance, archive with undo, and clarification in the same conversation.
- Both agents revise shared visual content with stable identity; conflicts and conflicting undo preserve unrelated user changes.
- Multiple views show shared or independent documents with consistent content/acceptance and private view state.
- Delivery reports queued versus started accurately; stop, reconnect and conversation switching do not silently duplicate or redirect questions. Unknown outcomes remain visible.
- Native approvals remain answerable in the correct CLI without duplicate responses or policy changes; unavailable capabilities produce actionable instructions.
- Restart restores content and private drafts. Normal exit, errors and supported interruption paths restore terminal state; keyboard operation remains complete in narrow panes.

## Evidence and remaining verification

Evidence inspected on 2026-11-08; these are documentation/interface observations, not completed integration tests.

- Reviewer-1's container: `codex --version` reported `codex-cli 0.167.0`; CLI help exposed native plugins, Unix app-server/remote connections and queueing. Generated schemas distinguish exact thread IDs from shared session IDs. `TurnStartParams.turnTrigger` explicitly describes `turn/start` steering an already-active turn; the public app-server guide does not describe that case, so runtime behavior remains gated. `tmux -V` reported 3.3a; Claude and Cargo were unavailable there.
- Reviewer-2's separate container: `claude --version` reported 2.1.319; native MCP/plugin/hook interfaces and bundled mod reference/declarations versioned 2.1.318 were inspected. Record this separately from the binary version; package validation and loaded-build declarations do not replace runtime probes. Codex and the Rust toolchain were unavailable there. The installed build's declarations still need a runtime integration probe.
- [Codex app-server documentation](https://learn.chatgpt.com/docs/app-server) supplies the connection, thread, turn and approval protocol; multi-client ownership and caller identity remain verification gates.
- [Codex plugin packaging](https://developers.openai.com/plugins/build/plugins) describes native package manifests, skills, MCP and hooks; select the installed CLI's supported layout and trust workflow.
- [Claude mods reference](https://code.claude.com/docs/en/plugins/mods/reference) documents native tooling and session lifecycle; verify the exact installed declarations before implementation.
- [Claude mods API](https://code.claude.com/docs/en/plugins/mods/api) documents background prompt submission, interruption outcomes and supported process/network access.
- [Claude mod activation](https://code.claude.com/docs/en/plugins/mods/overview) documents version and policy conditions; plugin presence does not prove the mod is active.
- [Claude channels reference](https://code.claude.com/docs/en/channels-reference) documents external event delivery and optional permission relay; neither is required by the chosen native-mod baseline.
- Additional visual types, images, cross-conversation views and agents beyond Claude/Codex can be planned later.
