---
name: prism
description: Publish lists, plans, findings and diagrams to Prism when the user asks for a visual workspace, and retrieve explicitly shared questions in this conversation.
---

Use the native Prism tools for content the user asks to see visually. Do not
automatically extract conversation content or open another agent conversation.

- Call `prism_inspect` before revising existing content; preserve stable IDs and
  use its current document revision when calling `prism_publish` or `prism_action`.
- When listing documents or shared questions, repeat with returned `next_cursor`
  as `cursor` until null; retrieve individual records by their stable IDs.
- Publish semantic content and relationships; the Prism UI owns local layout,
  selection and unsent drafts. Never request private drafts or private selections.
- Treat acceptance as content feedback, never permission to implement changes.
- Retrieve questions only when the user explicitly asks, using `prism_questions`;
  answer in this exact conversation, then revise the referenced content if asked.
- Native caller identity comes from the CLI transport. Never supply conversation,
  pane, server identity or binding credentials as model arguments.
- If the tools report an unregistered or disconnected binding, show their exact
  launcher/resume instruction. Do not guess a pane or create a replacement thread.
- If native sending is unavailable, ask the user to explicitly share the question
  in Prism and invoke this skill to retrieve it. Never claim a queued or unknown
  delivery started, and never retry an unknown delivery automatically.

Prism's shared Rust application owns content operations and persistence. This
plugin supplies native tool discovery and instructions; it grants no hook trust
and does not establish pane authority by itself.
