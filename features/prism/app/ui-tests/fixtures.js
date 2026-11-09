// Fixture sessions for driving the UI in a plain browser; shapes follow prism-app-host's
// AppState and Content as the shell serializes them.

const LIVE = 'claude:6f1c2a9e-3b7d-4e51-9a0c-2d4e8f6b1a37';
const CODEX = 'codex:019a2b7c-55e1-7d20-8f43-1c9e0a6d2b85';
const ENDED = 'claude:b3e8d4f2-7a16-4c09-8e5b-9f2a1d7c6e40';
const CLOSED = 'claude:4d7a9c1e-2f58-4b36-a0e7-5c3b8d9f1e62';

const PLAN = '11111111-0000-4000-8000-000000000001';
const REVIEW = '11111111-0000-4000-8000-000000000002';
const NOTES = '11111111-0000-4000-8000-000000000003';

function element(id, kind, order, title, body, extra = {}) {
  return {
    id,
    title,
    body,
    kind,
    source: null,
    order,
    semantic_revision: 3,
    accepted_revision: null,
    archived: false,
    ...extra,
  };
}

function document(id, title, revision, elements, relationships = [], groups = []) {
  const byId = (items) => Object.fromEntries(items.map((item) => [item.id, item]));
  return { id, title, revision, elements: byId(elements), relationships: byId(relationships), groups: byId(groups) };
}

const planElements = [
  element('a0000000-0000-4000-8000-000000000001', 'text', 0, 'Goal', 'Replace the terminal UI with one desktop app for macOS and Windows. A tab is one conversation; a window is a group of tabs the user arranges.'),
  element('a0000000-0000-4000-8000-000000000002', 'step', 1, 'Listener, pairing file, session registry', 'One process, one listening port (7717), loopback by default.\nThe pairing file {port, token} is written with mode 0600 to ~/.claude/prism/app.json and ~/.codex/prism/app.json.\nSessions are keyed by conversation; a newer binding supersedes an older one.', { source: 'features/prism/app/host/src/registry.rs', accepted_revision: 3 }),
  element('a0000000-0000-4000-8000-000000000003', 'step', 2, 'Windows and tabs', 'One tab per conversation; tabs move between windows by drag or the context menu. A project remembers its window.'),
  element('a0000000-0000-4000-8000-000000000004', 'finding', 3, 'Liveness of a connection is not part of the protocol', 'A bridge whose container was killed without a TCP reset stays "live" until the operating system notices. The protocol has no heartbeat rule.', { source: 'prism-core/src/protocol.rs', semantic_revision: 4 }),
  element('a0000000-0000-4000-8000-000000000005', 'question', 4, 'Should hidden items sync between windows?', 'Hide is local view state per tab today. A user who moves a tab keeps its hidden items; two windows never show the same tab.'),
  element('a0000000-0000-4000-8000-000000000006', 'step', 5, 'List view', 'Cards with kind color, the focused card expanded, keyboard navigation, ask, hide and drop.', { accepted_revision: 2 }),
];

const planLinks = [
  {
    id: 'b0000000-0000-4000-8000-000000000001',
    from: 'a0000000-0000-4000-8000-000000000002',
    to: 'a0000000-0000-4000-8000-000000000003',
    label: 'arrangement uses sessions',
    direction: 'directed',
    manual: false,
    semantic_revision: 3,
    accepted_revision: null,
    archived: false,
  },
];

const planGroups = [
  {
    id: 'c0000000-0000-4000-8000-000000000001',
    title: 'Batch 4',
    members: ['a0000000-0000-4000-8000-000000000002', 'a0000000-0000-4000-8000-000000000003', 'a0000000-0000-4000-8000-000000000006'],
    semantic_revision: 3,
    accepted_revision: null,
    archived: false,
  },
];

const question = {
  request_id: 'd0000000-0000-4000-8000-000000000001',
  workspace_id: 'e0000000-0000-4000-8000-000000000001',
  generation: 1,
  snapshot: {
    selection: { document_id: PLAN, revision: 3, ids: ['a0000000-0000-4000-8000-000000000004'] },
    document_title: 'App plan',
    elements: {},
    relationships: {},
    groups: {},
  },
  text: 'Item 4: Liveness of a connection is not part of the protocol\n> A bridge whose container was killed…\n\nDo we need WebSocket pings for this?',
  mode: 'share',
  outcome: { state: 'pending' },
  answer: 'Not for batch 4: Docker Desktop resets the forwarded connection when a container stops. Pings would need a protocol rule first.',
};

const reviewElements = [
  element('a1000000-0000-4000-8000-000000000001', 'finding', 0, 'Pairing token printed in debug logs', 'The bridge logs the pairing file contents at debug level.', { semantic_revision: 1 }),
  element('a1000000-0000-4000-8000-000000000002', 'finding', 1, 'Snapshot frames are not size-checked on the app side', 'MAX_MESSAGE_BYTES is enforced by the WebSocket configuration.', { semantic_revision: 1 }),
];

const codexElements = [
  element('a2000000-0000-4000-8000-000000000001', 'step', 0, 'Export invoices', 'Reads the invoice table in pages of 500 rows.', { semantic_revision: 1 }),
  element('a2000000-0000-4000-8000-000000000002', 'finding', 1, 'Currency rounding differs from the ledger', 'Rounding half-even in the export, half-up in the ledger.', { semantic_revision: 1 }),
];

const endedElements = [
  element('a3000000-0000-4000-8000-000000000001', 'text', 0, 'Summary of the cleared conversation', 'The conversation was cleared with /clear; this content stays readable until the tab is closed.', { semantic_revision: 1 }),
];

function status(overrides = {}) {
  return {
    address: '127.0.0.1:7717',
    listening: true,
    listen_error: null,
    warnings: [],
    pairing: [
      { path: '/Users/me/.claude/prism/app.json', error: null },
      { path: '/Users/me/.codex/prism/app.json', error: null },
    ],
    problem: null,
    settings_file: '/Users/me/Library/Application Support/local.prism.desktop/settings.json',
    ...overrides,
  };
}

function session(key, project, host, link, loaded = true) {
  const [agent, conversation] = key.split(':');
  return { key, host, project, agent, conversation, link, loaded };
}

const SESSIONS = [
  session(LIVE, '/workspace/agents_setup', 'd_claude', { state: 'live' }),
  session(CODEX, '/workspace/billing-service', 'd_codex', { state: 'disconnected' }),
  session(ENDED, '/workspace/agents_setup', 'd_claude', { state: 'ended', reason: 'conversation cleared' }),
  session(CLOSED, '/workspace/website', 'd_claude', { state: 'live' }),
];

const CONTENTS = {
  [LIVE]: {
    documents: [
      document(PLAN, 'App plan', 3, planElements, planLinks, planGroups),
      document(REVIEW, 'Review findings', 1, reviewElements),
      document(NOTES, 'Notes', 1, []),
    ],
    questions: [question],
  },
  [CODEX]: { documents: [document('22222222-0000-4000-8000-000000000001', 'Invoice export', 1, codexElements)], questions: [] },
  [ENDED]: { documents: [document('33333333-0000-4000-8000-000000000001', 'Summary', 1, endedElements)], questions: [] },
  [CLOSED]: { documents: [], questions: [] },
};

function arrangement(active, views = {}) {
  const labels = Object.fromEntries(
    SESSIONS.map(({ key, host, project, agent, conversation }) => [key, { host, project, agent, conversation }]),
  );
  return {
    windows: [
      { id: 'w1', tabs: [LIVE, CODEX, ENDED], active },
      { id: 'w2', tabs: [], active: null },
    ],
    project_windows: { '/workspace/agents_setup': 'w1', '/workspace/billing-service': 'w1' },
    closed: [CLOSED],
    labels,
    views,
    focused: 'w1',
    next_window: 2,
  };
}

export const KEYS = { LIVE, CODEX, ENDED, CLOSED, PLAN, REVIEW };
export const CONTENTS_FOR_TESTS = CONTENTS;

/** Fixture per scenario: `state` (AppState) and `contents` per session key. */
export function fixture(scenario) {
  if (scenario === 'empty') {
    return {
      state: { status: status(), sessions: [], arrangement: { windows: [{ id: 'w1', tabs: [], active: null }], project_windows: {}, closed: [], labels: {}, views: {}, focused: 'w1', next_window: 1 } },
      contents: {},
    };
  }
  if (scenario === 'port-error') {
    const error = 'Port 7717 on 127.0.0.1 is already in use. Another Prism app is probably running; quit it, or set another port in the settings file.';
    return {
      state: { status: status({ listening: false, listen_error: error, pairing: [] }), sessions: [], arrangement: fixture('empty').state.arrangement },
      contents: {},
    };
  }
  if (scenario === 'no-tabs') {
    const state = { status: status(), sessions: SESSIONS, arrangement: arrangement(LIVE) };
    state.arrangement.windows[0].tabs = [];
    state.arrangement.windows[0].active = null;
    state.arrangement.windows[1].tabs = [LIVE];
    state.arrangement.windows[1].active = LIVE;
    return { state, contents: CONTENTS };
  }
  const active = { ended: ENDED, disconnected: CODEX }[scenario] ?? LIVE;
  const views = scenario === 'focused' ? { [LIVE]: { document: PLAN, focus: 'a0000000-0000-4000-8000-000000000004', hidden: ['a0000000-0000-4000-8000-000000000005'], mode: 'list' } } : {};
  return { state: { status: status(), sessions: SESSIONS, arrangement: arrangement(active, views) }, contents: CONTENTS };
}
