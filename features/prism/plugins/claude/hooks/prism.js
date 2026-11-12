// Prism native mod for Claude Code. Identity comes from $.session.id(); the trusted
// on-demand attachment and the authenticated Unix bridge route every call, and the Prism
// desktop app shows the workspace.

const INSTRUCTION = 'Prism could not attach to this conversation. Check that prism is on PATH, then call a Prism tool or /prism again.';
const bindings = new Map();
const SUBAGENT_REFUSAL = 'Prism tools are available only to the main conversation the user is watching, not to subagents.';
const PROTOCOL_VERSION = 1;
const COMMAND_NAME = 'prism';
// Shared questions are polled and placed in the prompt box; the user types the question after
// them and sends it from there, and the answer stays in the conversation. An explain request
// needs nothing typed: it is submitted as a prompt of its own, and the agent records the
// explanation in Prism with the answer tool. A submission that is refused or fails goes to the
// prompt box instead, where the user sends it.
const QUESTION_POLL_MS = 1000;
const EXPLAIN_INTENT = 'explain';
const filledQuestions = new Set();
const refusedSubmissions = new Set();
const TOOL_FIELDS = {
  publish: ['request_id', 'document_id', 'expected_revision', 'title', 'elements', 'relationships', 'groups', 'lanes'],
  inspect: ['document_id', 'cursor'],
  action: ['document_id', 'expected_revision', 'action'],
  questions: ['request_id', 'cursor'],
  answer: ['request_id', 'text'],
};

export function register(on, options) {
  on('session.start', async ($, e, next) => {
    const tools = JSON.parse(await $.fs.read(decodeURIComponent(new URL('./tools.json', import.meta.url).pathname)));
    for (const tool of tools) {
      await $.tool.register({name: tool.name, description: tool.description, inputSchema: tool.inputSchema, isDeferred: false});
    }
    await $.command.register({name: COMMAND_NAME, description: 'Show how this conversation reaches the Prism app'});
    $.clock.every(QUESTION_POLL_MS, () => deliverQuestions($));
    return next(e);
  });

  // A failed guard refuses instead of letting the call fall through to the engine.
  on('tool.call', {tool: 'mcp__prism__publish'}, async ($, e) => callTool($, e, 'publish')).catch(refuseTool);
  on('tool.call', {tool: 'mcp__prism__inspect'}, async ($, e) => callTool($, e, 'inspect')).catch(refuseTool);
  on('tool.call', {tool: 'mcp__prism__action'}, async ($, e) => callTool($, e, 'action')).catch(refuseTool);
  on('tool.call', {tool: 'mcp__prism__questions'}, async ($, e) => callTool($, e, 'questions')).catch(refuseTool);
  on('tool.call', {tool: 'mcp__prism__answer'}, async ($, e) => callTool($, e, 'answer')).catch(refuseTool);

  on('command.run', {command: 'prism'}, async ($, e) => showStatus($)).catch(($, e, next) => (next.called ? next(e) : {text: 'Prism failed to reach its bridge. ' + INSTRUCTION}));

  on('session.end', async ($, e, next) => {
    // Best effort: the bridge retires the binding; a missing bridge changes nothing here.
    // The ending session's own ID is reported; $.session.id() may already be the successor's.
    try {
      await callBridge($, e.sessionId, 'lifecycle', {event: 'session_end', reason: String(e.reason)});
    } catch {
    }
    return next(e);
  });
}

function refuseTool($, e, next) {
  return next.called ? next(e) : {deny: 'Prism failed to reach its bridge. ' + INSTRUCTION};
}

async function callTool($, e, method) {
  // Subagent loops share the session ID but not the user's attention; only the main loop publishes.
  if (e.agentId !== undefined) {
    return {deny: SUBAGENT_REFUSAL};
  }
  // Tool arguments arrive flattened beside engine fields; only the schema's properties travel.
  const params = {};
  for (const field of TOOL_FIELDS[method]) {
    if (e[field] !== undefined) {
      params[field] = e[field];
    }
  }
  const outcome = await callBridge($, await $.session.id(), method, params);
  if (outcome.error !== undefined) {
    return {deny: outcome.error};
  }
  return {result: JSON.stringify(outcome.response)};
}

// Attaches like a tool call, then reports the published documents and the bridge's link to the
// desktop app from the read-only `status` operation.
async function showStatus($) {
  const sessionId = await $.session.id();
  const inspected = await callBridge($, sessionId, 'inspect', {});
  if (inspected.error !== undefined) {
    return {text: inspected.error};
  }
  const documents = `Prism lists ${inspected.response.data.total} published documents in this conversation.`;
  const status = await callBridge($, sessionId, 'status', {});
  if (status.error !== undefined) {
    return {text: `${documents} Its app link state is unavailable: ${status.error}`};
  }
  return {text: `${documents} ${describeLink(status.response.data.app)}`};
}

function describeLink(app) {
  if (app === null) {
    return 'The bridge has no link to the Prism app; see bridge.log in its binding directory.';
  }
  const address = app.port === null ? app.host : `${app.host}:${app.port}`;
  const since = new Date(app.since_ms).toISOString();
  // A missing pairing file is the whole failure; otherwise the last error says what went wrong.
  let pairing = `last error: ${app.error}`;
  if (app.pairing_file !== null) {
    pairing = app.pairing_found ? `pairing file ${app.pairing_file} found; ${pairing}` : `pairing file ${app.pairing_file} not found`;
  }
  switch (app.state) {
    case 'connecting':
      return `Connecting to the Prism app at ${address}; the first attempt started at ${since}.`;
    case 'connected':
      return `Connected to the Prism app at ${address} since ${since}.`;
    case 'retrying':
      return `Not connected to the Prism app at ${address}; retrying since ${since}; ${pairing}.`;
    case 'superseded':
      return `The Prism app shows this conversation from a later session; this bridge stopped connecting at ${since}. To show this session there again, restart it: /exit, then claude -c.`;
    default:
      return `Waiting for the conversation to be bound before connecting to the Prism app at ${address}.`;
  }
}

// Places each newly shared Prism question in the prompt box once, or submits an explain request,
// then tells the bridge which it did: `prompt_filled` or `prompt_submitted`. One that fails is
// tried again at the next poll; the others go on. Only an already attached conversation is polled.
async function deliverQuestions($) {
  const sessionId = await $.session.id();
  if (!bindings.get(sessionId)) {
    return;
  }
  // The poll never re-attaches: only an explicit tool call or /prism does.
  const pending = await callBridge($, sessionId, 'pending', {}, true);
  if (pending.error !== undefined) {
    return;
  }
  for (const question of pending.response.data.questions) {
    if (filledQuestions.has(question.request_id)) {
      continue;
    }
    const text = question.text + ' [prism:' + question.request_id + ']';
    // Kept from the start: a submission resolves only once the session took it, and the
    // polls meanwhile must not send it again.
    filledQuestions.add(question.request_id);
    const event = question.intent === EXPLAIN_INTENT && !refusedSubmissions.has(question.request_id) ? await submitQuestion($, question.request_id, text) : await fillQuestion($, text);
    if (event === null) {
      filledQuestions.delete(question.request_id);
      continue;
    }
    await callBridge($, sessionId, 'lifecycle', {event, request_id: question.request_id}, true);
  }
}

// Submits an explain request: `prompt_submitted`. One that is refused or fails is placed in the
// prompt box instead, from then on, so it is never submitted twice.
async function submitQuestion($, requestId, text) {
  let submitted;
  try {
    submitted = await $.prompt.submit({text});
  } catch {
    submitted = {drop: 'submission failed'};
  }
  if (submitted.drop === undefined) {
    return 'prompt_submitted';
  }
  refusedSubmissions.add(requestId);
  return fillQuestion($, text);
}

// Appends to the prompt box: `prompt_filled`, or null when the box did not take it.
async function fillQuestion($, text) {
  try {
    const filled = await $.prompt.fill({text: text + '\n', mode: 'append'});
    return filled.isFilled ? 'prompt_filled' : null;
  } catch {
    return null;
  }
}

// Resolves to {response} on HTTP 200 or {error} otherwise. Never includes the binding path or token.
// The bridge bounds its own response size and answers 413 with an {error} body.
async function callBridge($, sessionId, method, params, isRetry = false) {
  let bindingPath = bindings.get(sessionId);
  if (!bindingPath && method !== 'lifecycle') {
    const attached = await $.process.run(['prism', 'attach', '--agent', 'claude', '--conversation', sessionId]);
    if (attached.exitCode !== 0) {
      return {error: attached.stderr.trim() || INSTRUCTION};
    }
    bindingPath = JSON.parse(attached.stdout).binding;
    bindings.set(sessionId, bindingPath);
  }
  if (!bindingPath) {
    return {error: INSTRUCTION};
  }
  let binding;
  try {
    binding = JSON.parse(await $.fs.read(bindingPath));
  } catch {
    if (bindings.get(sessionId) === bindingPath && method !== 'lifecycle' && !isRetry) {
      // A later attachment pruned this retired binding; attach once more.
      bindings.delete(sessionId);
      return callBridge($, sessionId, method, params, true);
    }
    return {error: INSTRUCTION};
  }
  if (binding.retired && bindings.get(sessionId) === bindingPath && method !== 'lifecycle' && !isRetry) {
    // The bridge of this conversation ended; forget its binding and attach once more.
    bindings.delete(sessionId);
    return callBridge($, sessionId, method, params, true);
  }
  if (binding.retired || binding.agent !== 'claude' || typeof binding.token !== 'string') {
    return {error: INSTRUCTION};
  }

  const body = JSON.stringify({version: PROTOCOL_VERSION, generation: binding.generation, thread_id: sessionId, operation: {method, params}});
  let response;
  try {
    response = await $.http.fetch('http://localhost/call', {
      method: 'POST',
      socketPath: '/tmp/prism-' + bindingPath.split('/').at(-2) + '/bridge.sock',
      headers: {'Content-Type': 'application/json', Authorization: 'Bearer ' + binding.token},
      body,
    });
  } catch {
    if (bindings.get(sessionId) === bindingPath && method !== 'lifecycle' && !isRetry) {
      // The bridge ended without retiring its binding; attaching again restarts it.
      bindings.delete(sessionId);
      return callBridge($, sessionId, method, params, true);
    }
    return {error: 'Prism bridge is unreachable. ' + INSTRUCTION};
  }
  let value;
  try {
    value = JSON.parse(response.text);
  } catch {
    return {error: 'Prism bridge returned an invalid response'};
  }
  if (response.status !== 200) {
    return {error: typeof value.error === 'string' ? value.error : 'Prism bridge request failed'};
  }
  return {response: value};
}
