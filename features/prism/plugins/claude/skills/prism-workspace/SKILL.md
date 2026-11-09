---
name: prism-workspace
description: Publish lists, plans, findings and diagrams to Prism when the user asks for a visual workspace, and retrieve explicitly shared questions in this conversation.
---

Use the native Prism tools for content the user asks to see visually. Do not
automatically extract conversation content or open another agent conversation.

- Call `mcp__prism__inspect` before revising existing content; preserve stable
  IDs and use its current document revision when calling `mcp__prism__publish`
  or `mcp__prism__action`.
- When listing documents or shared questions, repeat with returned `next_cursor`
  as `cursor` until null; retrieve individual records by their stable IDs.
- Publish semantic content and relationships; the Prism UI owns local layout,
  selection and unsent drafts. Never request private drafts or private selections.
- Treat acceptance as content feedback, never permission to implement changes.
- Retrieve questions only when the user explicitly asks, using
  `mcp__prism__questions`; answer in this exact conversation, then revise the
  referenced content if asked.
- The `/prism` command opens or refreshes the Prism pane without a model turn;
  tell the user about it when they only want to see the current workspace.
- Prism tools refuse calls from subagents; call them from the main conversation.
- Native caller identity is the current Claude session ID, read by the mod
  itself. Never supply conversation, pane, server identity or binding
  credentials as model arguments.
- Start the normal CLI inside tmux. The first Prism tool attaches to this
  conversation and opens the application to its right; later calls reuse it.
- If attachment fails, show the tool's exact instruction. Never relaunch the
  CLI through Prism, guess another pane, or start a replacement conversation.
- Native sending from Prism is disabled for Claude: ask the user to explicitly
  share the question in Prism, then retrieve it with `mcp__prism__questions`.
  Never claim a queued or unknown delivery started, and never retry an unknown
  delivery automatically.

Prism's shared Rust application owns content operations and persistence. This
plugin supplies native tool discovery, the `/prism` command and lifecycle
reporting; it grants no hook trust and does not establish pane authority by
itself.
