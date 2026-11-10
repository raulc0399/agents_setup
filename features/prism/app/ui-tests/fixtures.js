// Fixture sessions for driving the UI in a plain browser; shapes follow prism-app-host's
// AppState and Content as the shell serializes them.

const LIVE = 'claude:6f1c2a9e-3b7d-4e51-9a0c-2d4e8f6b1a37';
const CODEX = 'codex:019a2b7c-55e1-7d20-8f43-1c9e0a6d2b85';
const ENDED = 'claude:b3e8d4f2-7a16-4c09-8e5b-9f2a1d7c6e40';
const CLOSED = 'claude:4d7a9c1e-2f58-4b36-a0e7-5c3b8d9f1e62';

const PLAN = '11111111-0000-4000-8000-000000000001';
const REVIEW = '11111111-0000-4000-8000-000000000002';
const NOTES = '11111111-0000-4000-8000-000000000003';
const RELEASE = '11111111-0000-4000-8000-000000000004';

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

const SYNC_ITEM = 'a0000000-0000-4000-8000-000000000005';
const TABS_ITEM = 'a0000000-0000-4000-8000-000000000003';

/** A question about the sync item (and `others`), asked at `revision`; request IDs are time-ordered (UUID v7). */
function syncQuestion(request, revision, typed, outcome, { answer = null, others = [] } = {}) {
  return {
    request_id: `019a3f10-0000-7000-8000-${request}`,
    workspace_id: 'e0000000-0000-4000-8000-000000000001',
    generation: 1,
    snapshot: {
      selection: { document_id: PLAN, revision, ids: [SYNC_ITEM, ...others] },
      document_title: 'App plan',
      elements: {},
      relationships: {},
      groups: {},
    },
    text: `Item 5: Should hidden items sync between windows?\n> Hide is local view state per tab today.\n\n${typed}`,
    mode: 'share',
    outcome,
    answer,
  };
}

/** Latest first: 4, 3, 2, 1, then the one asked at an older revision despite its larger ID. */
const syncQuestions = [
  syncQuestion('000000000001', 3, 'Would a per-window hide confuse users?', { state: 'failed', reason: 'binding retired; explicitly resume the conversation' }),
  syncQuestion('000000000002', 3, 'Does the tab list need the same rule?', { state: 'pending' }, { others: [TABS_ITEM] }),
  syncQuestion('000000000003', 3, 'Can hide follow the tab when it moves?', { state: 'submitted' }, { answer: 'Yes: hidden items are view state of the tab, and that view moves with it.' }),
  syncQuestion('000000000004', 3, 'Should the filter be shared as well?', { state: 'submitted' }),
  syncQuestion('00000000000f', 2, 'Is this still open?', { state: 'refused', reason: 'native sending from Prism is disabled for Claude' }),
];

/** Release flow: three lanes, five arrows (one manual), two gate badges. */
const releaseItem = (number) => `a4000000-0000-4000-8000-00000000000${number}`;
const releaseLink = (number) => `b4000000-0000-4000-8000-00000000000${number}`;
const releaseGroup = (number) => `c4000000-0000-4000-8000-00000000000${number}`;

const releaseElements = [
  element(releaseItem(1), 'step', 0, 'Capture readings', 'Sensor app records a reading every minute and queues it offline.', { source: 'device/src/capture.rs', semantic_revision: 2 }),
  element(releaseItem(2), 'step', 1, 'Upload batch', 'Sends the queued readings over HTTPS once the device is online.', { semantic_revision: 2 }),
  element(releaseItem(3), 'step', 2, 'Validate batch', 'Schema and range checks reject readings outside the sensor limits.', { semantic_revision: 2, accepted_revision: 2 }),
  element(releaseItem(4), 'step', 3, 'Review anomalies', 'An operator checks flagged readings before they are released.', { semantic_revision: 2 }),
  element(releaseItem(5), 'step', 4, 'Publish dashboard', 'Released readings appear on the public dashboard within a minute.', { semantic_revision: 2, accepted_revision: 1 }),
  element(releaseItem(6), 'text', 5, 'Archive raw batches', 'Raw uploads are kept for 90 days for audits.', { semantic_revision: 2 }),
  element(releaseItem(7), 'finding', 6, 'Clock drift on old devices', 'Devices older than 2021 drift up to 40 s a day; timestamps need the server clock.', { semantic_revision: 2 }),
  element(releaseItem(8), 'question', 7, 'Who approves a release?', 'The review step needs a named approver per shift.', { semantic_revision: 2 }),
];

function relationship(number, from, to, label, manual = false) {
  return {
    id: releaseLink(number),
    from: releaseItem(from),
    to: releaseItem(to),
    label,
    direction: 'directed',
    manual,
    semantic_revision: 2,
    accepted_revision: null,
    archived: false,
  };
}

const releaseLinks = [
  relationship(1, 1, 2, 'queue'),
  relationship(2, 2, 3, 'upload'),
  relationship(3, 3, 4, 'flag anomalies'),
  relationship(4, 4, 5, 'approve release', true),
  relationship(5, 3, 6, 'store raw batch'),
  relationship(6, 7, 1, 'affects'),
  relationship(7, 8, 4, 'blocks'),
];

const releaseGroups = [
  ['Device', [1, 2]],
  ['Server staging', [3, 4]],
  ['Server live', [5, 6]],
].map(([title, members], index) => ({
  id: releaseGroup(index + 1),
  title,
  members: members.map(releaseItem),
  semantic_revision: 2,
  accepted_revision: null,
  archived: false,
}));

const releaseQuestion = {
  request_id: 'd0000000-0000-4000-8000-000000000002',
  workspace_id: 'e0000000-0000-4000-8000-000000000001',
  generation: 1,
  snapshot: {
    selection: { document_id: RELEASE, revision: 2, ids: [releaseItem(4)] },
    document_title: 'Release flow',
    elements: {},
    relationships: {},
    groups: {},
  },
  text: 'Item 4: Review anomalies\n> An operator checks flagged readings…\n\nCan small batches skip the review?',
  mode: 'share',
  outcome: { state: 'submitted' },
  answer: 'No: every batch with a flagged reading waits for the operator; batches without flags skip the queue.',
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
    version: '0.1.0',
    protocol: 2,
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
      document(RELEASE, 'Release flow', 2, releaseElements, releaseLinks, releaseGroups),
    ],
    questions: [question, releaseQuestion, ...syncQuestions],
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

export const VERSION_PROBLEM =
  'A bridge speaks Prism protocol 3, this app protocol 2. This app is older: build and install the newer Prism app.';

export const KEYS = { LIVE, CODEX, ENDED, CLOSED, PLAN, REVIEW, NOTES, RELEASE, SYNC_ITEM, TABS_ITEM };
export const RELEASE_IDS = { item: releaseItem, link: releaseLink, group: releaseGroup };

/** View state of the live tab per diagram scenario; the release flow opens in the diagram unless chosen otherwise. */
const DIAGRAM_VIEWS = {
  diagram: { focus: null, cameras: {} },
  'diagram-panel': { focus: releaseItem(4), cameras: {} },
  'diagram-ask': { focus: releaseItem(4), cameras: {} },
  'diagram-zoomed': { focus: releaseItem(7), cameras: { [RELEASE]: { x: 20, y: -20, scale: 1.9 } } },
  // A focused and a selected arrow between the same two lanes: marked by more than their color.
  'diagram-arrow': { focus: releaseLink(4), selected: [releaseLink(5)], cameras: {} },
  'diagram-checks': { focus: null, modes: { [RELEASE]: 'list' }, cameras: {} },
  'diagram-dropped': { focus: null, cameras: {} },
  'narrow-checks': { focus: releaseItem(4), cameras: {} },
};
export const CONTENTS_FOR_TESTS = CONTENTS;

/**
 * View state of the live tab per list scenario. The app plan has an arrow and a group, so it
 * would open in the diagram: the list scenarios chose the list for it before.
 */
const LIST_MODES = { [PLAN]: 'list' };
const LIST_VIEWS = {
  focused: { document: PLAN, focus: 'a0000000-0000-4000-8000-000000000004', hidden: [SYNC_ITEM] },
  questions: { document: PLAN, focus: SYNC_ITEM, hidden: [] },
  filter: { document: PLAN, focus: null, hidden: [], filter: 'pairing' },
  'action-checks': { modes: { ...LIST_MODES, [RELEASE]: 'list' } },
  // A document without arrows or groups: a list without a view switch.
  plain: { document: REVIEW },
  dropped: { document: PLAN, focus: 'a0000000-0000-4000-8000-000000000004' },
};
/** Scenarios whose live tab shows dropped content: document, element and relationship IDs. */
const DROPPED_SCENARIOS = {
  dropped: [PLAN, ['a0000000-0000-4000-8000-000000000004'], []],
  'diagram-dropped': [RELEASE, [releaseItem(6)], [releaseLink(5)]],
};

/** The fixture's contents with the given content of one document dropped (archived). */
function droppedContents([documentId, elements, relationships]) {
  const live = structuredClone(CONTENTS[LIVE]);
  const dropped = live.documents.find((candidate) => candidate.id === documentId);
  elements.forEach((id) => {
    dropped.elements[id].archived = true;
  });
  relationships.forEach((id) => {
    dropped.relationships[id].archived = true;
  });
  return { ...CONTENTS, [LIVE]: live };
}
/** Scenarios whose live tab has two documents closed: the strip with its Closed (N) control. */
const CLOSED_SCENARIOS = ['documents', 'closed-menu'];
/** Scenarios whose live tab has no saved view state: each document opens as its content says. */
const FRESH_VIEWS = ['view-checks'];

/** Fixture per scenario: `state` (AppState) and `contents` per session key. */
export function fixture(scenario) {
  if (scenario === 'empty') {
    return {
      state: { status: status(), sessions: [], arrangement: { windows: [{ id: 'w1', tabs: [], active: null }], project_windows: {}, closed: [], labels: {}, views: {}, focused: 'w1', next_window: 1 } },
      contents: {},
    };
  }
  if (scenario === 'mismatch' || scenario === 'status-checks') {
    return {
      state: { status: status({ problem: VERSION_PROBLEM }), sessions: [], arrangement: fixture('empty').state.arrangement },
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
  if (CLOSED_SCENARIOS.includes(scenario)) {
    const contents = { ...CONTENTS, [LIVE]: { ...CONTENTS[LIVE], closed: [NOTES, REVIEW] } };
    return { state: { status: status(), sessions: SESSIONS, arrangement: arrangement(LIVE, { [LIVE]: { modes: LIST_MODES } }) }, contents };
  }
  const active = { ended: ENDED, disconnected: CODEX }[scenario] ?? LIVE;
  const views = FRESH_VIEWS.includes(scenario) ? {} : { [LIVE]: { modes: LIST_MODES, ...LIST_VIEWS[scenario] } };
  if (DIAGRAM_VIEWS[scenario]) {
    views[LIVE] = { document: RELEASE, hidden: [], ...DIAGRAM_VIEWS[scenario] };
  }
  const contents = DROPPED_SCENARIOS[scenario] ? droppedContents(DROPPED_SCENARIOS[scenario]) : CONTENTS;
  return { state: { status: status(), sessions: SESSIONS, arrangement: arrangement(active, views) }, contents };
}
