// Prism native mod for Claude Code. Identity comes from $.session.id(); the trusted
// launcher binding (PRISM_BINDING_FILE) and the authenticated Unix bridge own everything else.

const INSTRUCTION = 'Prism requires a trusted tmux binding. Resume this exact conversation with: prism --agent claude --resume <sessionId>. An unregistered CLI cannot safely identify its pane.';
const SUBAGENT_REFUSAL = 'Prism tools are available only to the main conversation the user is watching, not to subagents.';
const PROTOCOL_VERSION = 1;
const COMMAND_NAME = 'prism';
const TOOL_FIELDS = {
  publish: ['request_id', 'document_id', 'expected_revision', 'title', 'elements', 'relationships', 'groups'],
  inspect: ['document_id', 'cursor'],
  action: ['document_id', 'expected_revision', 'action'],
  questions: ['request_id', 'cursor'],
};

export function register(on, options) {
  on('session.start', async ($, e, next) => {
    const tools = JSON.parse(await $.fs.read(decodeURIComponent(new URL('./tools.json', import.meta.url).pathname)));
    for (const tool of tools) {
      await $.tool.register({name: tool.name, description: tool.description, inputSchema: tool.inputSchema, isDeferred: false});
    }
    await $.command.register({name: COMMAND_NAME, description: 'Open or refresh the Prism pane beside this conversation'});
    return next(e);
  });

  // A failed guard refuses instead of letting the call fall through to the engine.
  on('tool.call', {tool: 'mcp__prism__publish'}, async ($, e) => callTool($, e, 'publish')).catch(refuseTool);
  on('tool.call', {tool: 'mcp__prism__inspect'}, async ($, e) => callTool($, e, 'inspect')).catch(refuseTool);
  on('tool.call', {tool: 'mcp__prism__action'}, async ($, e) => callTool($, e, 'action')).catch(refuseTool);
  on('tool.call', {tool: 'mcp__prism__questions'}, async ($, e) => callTool($, e, 'questions')).catch(refuseTool);

  on('command.run', {command: 'prism'}, async ($, e) => openPane($)).catch(($, e, next) => (next.called ? next(e) : {text: 'Prism failed to reach its bridge. ' + INSTRUCTION}));

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

async function openPane($) {
  const outcome = await callBridge($, await $.session.id(), 'inspect', {});
  if (outcome.error !== undefined) {
    return {text: outcome.error};
  }
  const documents = outcome.response.data.total;
  const pane = outcome.response.pane;
  if (pane.error !== undefined) {
    return {text: `Prism lists ${documents} published documents in this conversation, but its pane could not open: ${pane.error}`};
  }
  return {text: `Prism pane ${pane.id} is open beside this conversation with ${documents} published documents.`};
}

// Resolves to {response} on HTTP 200 or {error} otherwise. Never includes the binding path or token.
// The bridge bounds its own response size and answers 413 with an {error} body.
async function callBridge($, sessionId, method, params) {
  const instruction = INSTRUCTION.replace('<sessionId>', sessionId);
  const bindingPath = await $.env.get('PRISM_BINDING_FILE');
  if (!bindingPath) {
    return {error: instruction};
  }
  let binding;
  try {
    binding = JSON.parse(await $.fs.read(bindingPath));
  } catch {
    return {error: instruction};
  }
  if (binding.retired || binding.agent !== 'claude' || typeof binding.token !== 'string') {
    return {error: instruction};
  }

  const body = JSON.stringify({version: PROTOCOL_VERSION, generation: binding.generation, thread_id: sessionId, operation: {method, params}});
  let response;
  try {
    response = await $.http.fetch('http://localhost/call', {
      method: 'POST',
      socketPath: bindingPath.slice(0, bindingPath.lastIndexOf('/')) + '/bridge.sock',
      headers: {'Content-Type': 'application/json', Authorization: 'Bearer ' + binding.token},
      body,
    });
  } catch {
    return {error: 'Prism bridge is unreachable. ' + instruction};
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
