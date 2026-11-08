# Prism: implementation plan for Codex CLI

Status: planning only; saving this document does not authorize implementation.

## Purpose

Prism helps the user visualize, explore, and understand things discussed with Codex, then bring selections, questions, and decisions back into the same conversation.

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

- In tmux, the first Prism tool call opens a pane to the right of the calling Codex CLI pane; later calls reuse it.
- The Codex CLI conversation stays on the left; interactive Prism views appear on the right.
- Outside tmux, running `prism` starts a tmux session with that layout and launches both interfaces.
- An existing Codex conversation can be resumed explicitly through the launcher; opening Prism must not silently create a different conversation.
- Opening Prism from an existing session requires a verified association between the conversation and its tmux pane.
- If that association is unavailable, show the launch/resume instruction instead of guessing which pane to split.

## Proposed architecture

- Implement a Rust application in `features/prism/`, using Ratatui and Crossterm for terminal rendering and input.
- Use one binary with launcher, `prism ui`, and `prism mcp` entry points.
- Add a local stdio MCP server through `prism mcp`, exposing tools to open Prism, create/update visual content, and inspect current content and selections.
- Connect the Prism UI and Codex CLI to the same Codex app-server and explicit thread ID.
- Use WebSocket-framed JSON-RPC over a local Unix socket for Prism's app-server connection.
- Persist visual documents and view state in a project-local `.prism/state.db` SQLite database, scoped by conversation ID.
- Keep launcher/session binding, storage, Codex communication, tool handling, and rendering in separate modules.
- Implement built-in list and diagram renderers first; add further renderers/actions without duplicating storage or communication.

### Proposed source layout

| Path under `features/prism/` | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | Reproducible application dependencies |
| `src/main.rs` | Command routing and startup |
| `src/session.rs` | tmux launch, pane identity, server/thread binding, cleanup |
| `src/model.rs` | Documents, elements, relationships, views, selections, revisions |
| `src/store.rs` | SQLite persistence, transactions, conflict detection, undo |
| `src/codex.rs` | App-server connection, events, questions, reconnect handling |
| `src/mcp.rs` | Structured tool inputs, validation, content publication and inspection |
| `src/ui/mod.rs` | Event loop, navigation, view switching, shared actions |
| `src/ui/list.rs` | List rendering and selection |
| `src/ui/diagram.rs` | Diagram layout, rendering, and hit detection |
| `tests/` | Storage, protocol, interaction, and end-to-end checks |
| `README.md` | Setup, launch, tool registration, controls, recovery |

## Communication with Codex

- **Chat to Prism:** the user asks Codex to visualize something; Codex calls a Prism tool with structured content, relationships, and view instructions.
- **Publication:** the tool validates the payload, commits a revision, opens/reuses the correct pane, and returns the affected document and element IDs to Codex.
- **Rendering:** Prism reads committed content changes and refreshes every affected view.
- **Prism to chat:** the user selects visual content, edits a question in Prism, and explicitly sends it to the bound conversation.
- **Question context:** include document ID, revision, selected element/relationship IDs, relevant visible content, and the user's question.
- **Idle conversation:** send the question through `turn/start` with the bound `threadId`.
- **Active conversation:** use `turn/steer` with the observed `expectedTurnId`; handle a stale turn explicitly rather than silently redirecting the message.
- **Response:** stream app-server events into Prism's conversation status; the CLI shows the conversation, and Codex can revise the visualization through the Prism tool.
- **Reconnect:** restore the same binding and refresh current state; do not automatically resend a question whose delivery is uncertain.
- **Approvals:** determine which connected client receives actionable requests and keep them answerable without duplicate responses or changing the user's approval policy.

The proposed MCP tools publish visual content; app-server carries conversation messages and events. Prism's question editor is its own input surface; filling the Codex CLI's prompt box is not assumed to be available.

## Content and view rules

- Give documents, elements, relationships, and views stable IDs; preserve identity across revisions and representations.
- Represent semantic content separately from list ordering, diagram coordinates, filters, and panel arrangement.
- Allow diagram nodes and list entries to refer to the same element; keep relationship IDs separately selectable.
- Store source conversation references when available so a visual selection retains its context.
- Require an expected revision for updates; report conflicts instead of overwriting a newer edit.
- Make repeated publication requests idempotent using a request ID.
- Keep selection and question drafts local to their view until the user sends them.
- Let acceptance apply to a particular content revision, with visible indication when that content changes.
- Treat acceptance as feedback about content; it does not itself authorize executing a plan or changing code.
- Preserve valid relationships when removing nodes, and include affected relationships in undo.

### Proposed defaults to confirm before their implementation

- Capture visual content on request rather than automatically extracting every list from chat.
- Remove archives the underlying content across views and supports undo; hiding content from one view is a separate action.
- Views referencing the same element share edits and acceptance; independently created content stays independent.
- Start with one bound Codex conversation per Prism workspace and multiple documents/views within it.

## Implementation batches

Each batch requires implementation approval, a context check before editing, and the narrowest relevant validation afterward. Work is sequential; no agent delegation is planned.

1. **Verify the integration contract.** Check the installed Codex version/schema, Rust toolchain, terminal, and tmux capabilities; establish exact server/thread/pane binding and MCP tool-call context; verify two clients can observe the intended thread, answer approvals, reconnect, and exchange an explicit test question. Record outcomes in this plan before depending on them. If a required capability fails, revise the design before building the affected feature.

2. **Build the content foundation and tools.** Add the Rust crate, content/view model, SQLite storage, and MCP publication/inspection operations. Validate payloads, stable IDs, references, transactional updates, duplicate requests, conflicting revisions, persistence, archive, and undo. Register the local MCP server through documented setup instructions.

3. **Deliver the first visual conversation loop.** Implement the launcher, pane reuse, list renderer, selection, acceptance/removal, question editor, and app-server client. Demonstrate a list created from chat, a clarification sent from a selected item, and a Codex revision reflected in Prism. Test missing files/configuration, narrow panes, terminal restoration, correct-thread delivery, and uncertain sends after disconnects.

4. **Add interactive diagrams.** Define the initial node/relationship payload, then implement deterministic layout, labels, keyboard/mouse selection, grouping, scrolling/panning, and a selected-element detail view. Reuse the same acceptance, removal, undo, and discussion actions. Test hit detection after scrolling/resizing, relationship cleanup, stale selections, and a diagram clarified and revised through chat.

5. **Add multiple views.** Provide tabs and splits inside Prism, including list and diagram views of shared content and views of separate documents. Test synchronized content/acceptance, independent layout/selection, archiving and undo across views, and restoration after restart.

6. **Validate and document the release.** Run the relevant Rust tests, formatting, lint, and build checks; exercise the full loop with a real Codex CLI and tmux session. Document installation, MCP configuration, first-use launch, resuming a conversation, controls, compatibility, and recovery. Confirm new renderer/action additions can reuse the existing model and communication modules.

## Completion criteria

- A user can request a visual explanation in Codex and see Prism open in the correct adjacent pane.
- Lists and diagrams support selection, acceptance, removal with undo, and clarification in the same chat.
- Codex can revise the selected visual content without losing its identity or unrelated user changes.
- Multiple views display shared or independent content with consistent behavior.
- Restarting restores persisted content; reconnecting preserves conversation identity and never silently duplicates a question.
- The terminal remains usable after Prism exits or fails, and unsupported launch conditions produce actionable instructions.

## Evidence and remaining verification

- Earlier local inspection found `codex-cli 0.167.0` with Unix-socket support in `codex app-server --listen` and `codex --remote`; recheck the implementation environment in batch 1.
- Earlier local inspection found tmux 3.3a with mouse and passthrough disabled, and no Rust/Cargo commands available; setup and terminal behavior still need validation.
- [Codex app-server documentation](https://learn.chatgpt.com/docs/app-server) describes connection initialization, thread operations, turn submission/steering, streaming events, and approvals.
- Shared-client behavior, reliable caller-to-pane identification, and tool invocation context remain integration checks; support for a socket alone does not establish them.
- Additional visual types, image rendering, cross-conversation views, and other agent integrations can be planned later.
