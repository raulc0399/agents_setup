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
- Publish semantic content and relationships; the Prism UI owns local layout
  and selection. Never request private selections.
- Every element, relationship and group keeps the number of its first
  publication: what a publish adds goes after the last item and nothing
  existing moves, dropped items included, so the user's item numbers stay valid.
- List relationships in the order the data flows: diagrams number arrows in
  publication order and never renumber them. When a flow changes in the
  middle, publish the diagram as a new document rather than adding a mid-flow
  arrow to the old one. Do not put step numbers into relationship labels:
  Prism numbers the arrows.
- Treat acceptance as content feedback, never permission to implement changes.
- All Prism content and document state is shared through the bridge store: a
  close, drop, accept, question or answer is seen by you and every app view;
  there is no app-local document state.
- Close or reopen a whole document with `mcp__prism__action` (`close` or
  `reopen`, no `ids`) only when asked; `mcp__prism__inspect` marks closed
  documents, and publishing to a closed document reopens it.
- A prompt that quotes an item and carries `[prism:<request_id>]` came from
  Prism's ask action: the user asks about that item or the quoted text, with
  the question typed after the marker. Answer in this conversation and save the
  answer to the relevant md files when it matters. Answers are not recorded in
  Prism, so for now do not call `mcp__prism__answer`; the app changes only when
  the item itself changes, when you publish the revised item. Call
  `mcp__prism__questions` with the request ID when you need the frozen content
  snapshot.
- Retrieve other shared questions only when the user explicitly asks, using
  `mcp__prism__questions`; answer in this exact conversation, then revise the
  referenced content if asked.
- Relationships accept `manual: true` for manual or review steps; diagrams
  draw them dashed. Groups become lanes in diagrams, so group by where things
  run (device, service, operator) when drawing architecture or data flow.
- The `/prism` command reports the published documents and whether the Prism
  app is connected, without a model turn; tell the user about it when they only
  want to check the current workspace.
- Prism tools refuse calls from subagents; call them from the main conversation.
- Native caller identity is the current Claude session ID, read by the mod
  itself. Never supply conversation, process, server identity or binding
  credentials as model arguments.
- Start the normal CLI. The first Prism tool attaches to this conversation, and
  the Prism desktop app shows its workspace in a tab; later calls reuse it.
- If attachment fails, show the tool's exact instruction. Never relaunch the
  CLI through Prism, guess another binding, or start a replacement conversation.
- Native sending from Prism is disabled for Claude: ask the user to explicitly
  share the question in Prism, then retrieve it with `mcp__prism__questions`.
  Never claim a queued or unknown delivery started, and never retry an unknown
  delivery automatically.

Prism's shared Rust application owns content operations and persistence. This
plugin supplies native tool discovery, the `/prism` command and lifecycle
reporting; it grants no hook trust and does not establish attachment authority
by itself.
