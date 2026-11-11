---
name: prism
description: Publish lists, plans, findings and diagrams to Prism when the user asks for a visual workspace, and retrieve explicitly shared questions in this conversation.
---

Use the native Prism tools for content the user asks to see visually. Do not
automatically extract conversation content or open another agent conversation.

- To attach Prism without publishing content, call `prism_inspect` with `{}`.
- Call `prism_inspect` before revising existing content; preserve stable IDs and
  use its current document revision when calling `prism_publish` or `prism_action`.
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
- Close or reopen a whole document with `prism_action` (`close` or `reopen`, no
  `ids`) only when asked; `prism_inspect` marks closed documents, and publishing
  to a closed document reopens it.
- Retrieve questions only when the user explicitly asks, using `prism_questions`;
  answer in this exact conversation, then revise the referenced content if asked.
  Keep the answer in the conversation and save it to the relevant md files when
  it matters. Answers are not recorded in Prism, so for now do not call
  `prism_answer`; the app changes only when you publish the revised item.
- Native caller identity comes from the CLI transport. Never supply conversation,
  process, server identity or binding credentials as model arguments.
- Subagents and forked threads must return their findings to the bound parent
  conversation for publication. Do not reuse the parent's Prism binding, spoof
  its identity, or retry a refusal from a different thread. Plugin inheritance
  does not establish authority to publish into the parent's workspace.
- Start the normal CLI. The first Prism tool attaches to this conversation, and
  the Prism desktop app shows its workspace in a tab; later calls reuse it.
- If attachment fails, show the tool's exact instruction. Never relaunch the
  CLI through Prism, guess another binding, or start a replacement conversation.
- If native sending is unavailable, ask the user to explicitly share the question
  in Prism and invoke this skill to retrieve it. Never claim a queued or unknown
  delivery started, and never retry an unknown delivery automatically.

Prism's shared Rust application owns content operations and persistence. This
plugin supplies native tool discovery and instructions; it grants no hook trust
and does not establish attachment authority by itself.
