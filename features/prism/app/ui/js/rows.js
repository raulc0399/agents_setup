import { flowText } from './format.js';

/**
 * What the list shows for a document: every element, link and group in the document's one
 * publication sequence, so a publication never renumbers it; hidden and filtered-out ones
 * left out. Archived (dropped) ones stay in their place, marked `dropped`.
 */

export const KIND_LABELS = {
  text: 'Text',
  step: 'Step',
  finding: 'Finding',
  question: 'Question',
  link: 'Link',
  group: 'Group',
};

const DIRECTION_LABELS = {
  directed: 'directed',
  undirected: 'undirected',
  bidirectional: 'bidirectional',
};

function byId(left, right) {
  return left.id < right.id ? -1 : left.id > right.id ? 1 : 0;
}

/** Record kinds in the order they sort at one position, as `Document::list_order` in prism-core. */
const RANKS = { element: 0, link: 1, group: 2 };

/**
 * Every record's position, as `Document::positions` in prism-core: an element's `order`; a
 * link or group stored before it had one gets one after the highest stored position, links
 * before groups, each by ID. A document stored before keeps the numbering it always showed.
 */
function positions(document) {
  const elements = Object.values(document.elements);
  const links = Object.values(document.relationships).sort(byId);
  const groups = Object.values(document.groups).sort(byId);
  const stored = [...elements.map((item) => item.order ?? 0), ...[...links, ...groups].map((item) => item.order).filter((order) => order !== null && order !== undefined)];
  let next = stored.length > 0 ? Math.max(...stored) + 1 : 0;
  const result = new Map(elements.map((item) => [item.id, item.order ?? 0]));
  for (const item of [...links, ...groups]) {
    if (item.order === null || item.order === undefined) {
      result.set(item.id, next);
      next += 1;
    } else {
      result.set(item.id, item.order);
    }
  }
  return result;
}

function records(document) {
  const titleOf = (id) => document.elements[id]?.title ?? 'missing';
  const position = positions(document);
  const elements = Object.values(document.elements)
    .map((item) => ({
      id: item.id,
      kind: item.kind,
      title: item.title,
      excerpt: flowText(item.body),
      body: item.body,
      source: item.source,
      details: item.source ? `${item.body}\nSource: ${item.source}` : item.body,
      revision: item.semantic_revision,
      created: item.created_revision,
      accepted: item.accepted_revision,
      dropped: item.archived,
      rank: RANKS.element,
    }));
  const links = Object.values(document.relationships)
    .map((item) => {
      const endpoints = `${titleOf(item.from)} -> ${titleOf(item.to)}`;
      const details = `${endpoints}\nDirection: ${DIRECTION_LABELS[item.direction] ?? item.direction}`;
      return {
        id: item.id,
        kind: 'link',
        title: item.label,
        excerpt: endpoints,
        body: details,
        source: null,
        details,
        revision: item.semantic_revision,
        created: item.created_revision,
        accepted: item.accepted_revision,
        dropped: item.archived,
        rank: RANKS.link,
      };
    });
  const groups = Object.values(document.groups)
    .map((item) => {
      const details = `Members:\n${item.members.map((id) => `- ${titleOf(id)}`).join('\n')}`;
      return {
        id: item.id,
        kind: 'group',
        title: item.title,
        excerpt: `${item.members.length} members`,
        body: details,
        source: null,
        details,
        revision: item.semantic_revision,
        created: item.created_revision,
        accepted: item.accepted_revision,
        dropped: item.archived,
        rank: RANKS.group,
      };
    });
  return [...elements, ...links, ...groups].sort(
    (left, right) => position.get(left.id) - position.get(right.id) || left.rank - right.rank || byId(left, right),
  );
}

/**
 * Whether a record matches the tab's filter, as in the terminal list: case-insensitive, over
 * "<kind>: <title>", the item's text and its ID.
 */
function matches(record, filter) {
  if (!filter) {
    return true;
  }
  const label = `${KIND_LABELS[record.kind] ?? record.kind}: ${record.title}`;
  return [label, record.details, record.id].some((text) => text.toLowerCase().includes(filter));
}

/**
 * What the latest publication did to a record while older content exists: 'new' when it first
 * published it, 'changed' when it revised one published before (or stored without a created
 * revision); null when it did neither, the record is accepted or dropped, or it was accepted
 * before and already shows "changed since accepted".
 */
function latestChange(record, latest, hasOlderContent) {
  if (!hasOlderContent || record.revision !== latest || record.accepted === record.revision || record.dropped) {
    return null;
  }
  if (record.created === latest) {
    return 'new';
  }
  const acceptedBefore = record.accepted !== null && record.accepted !== undefined;
  return acceptedBefore ? null : 'changed';
}

/**
 * Visible rows, numbered from 1 in display order (the number an `ask` refers to).
 * `latestChange`: 'new', 'changed' or null, see `latestChange`.
 */
export function rows(document, hidden, filter = '') {
  const all = records(document);
  const revisions = all.map((record) => record.revision);
  const latest = Math.max(...revisions);
  const hasOlderContent = latest > Math.min(...revisions);
  const needle = filter.trim().toLowerCase();
  return all
    .filter((record) => !hidden.has(record.id) && matches(record, needle))
    .map((record, index) => ({
      ...record,
      number: index + 1,
      kindLabel: KIND_LABELS[record.kind] ?? record.kind,
      isAccepted: record.accepted === record.revision,
      staleAcceptance: record.accepted !== null && record.accepted !== undefined && record.accepted !== record.revision,
      latestChange: latestChange(record, latest, hasOlderContent),
    }));
}

/** Number of items a document has, dropped and hidden ones included. */
export function itemCount(document) {
  return records(document).length;
}

/**
 * Whether a document has something to draw beyond boxes: an arrow or a group. Dropped ones
 * count too: the diagram keeps drawing them where they were.
 */
export function hasDiagram(document) {
  return Object.keys(document.relationships).length > 0 || Object.keys(document.groups).length > 0;
}

/** IDs of the items a document has, dropped and hidden ones included. */
export function itemIds(document) {
  return new Set(records(document).map((record) => record.id));
}

/**
 * Shared questions that asked about this item, the latest first. Questions carry no time:
 * the document revision they were asked at orders them, then the request ID, which the app
 * makes time-ordered (UUID v7). Questions the terminal UI made have random IDs (UUID v4), so
 * among those asked at one revision the order is arbitrary.
 */
export function questionsFor(questions, documentId, itemId) {
  return questions
    .filter(
      (question) =>
        question.snapshot.selection.document_id === documentId && question.snapshot.selection.ids.includes(itemId),
    )
    .sort(
      (left, right) =>
        right.snapshot.selection.revision - left.snapshot.selection.revision ||
        byId({ id: right.request_id }, { id: left.request_id }),
    );
}

/** The `intent` of an explain request, as `Intent::Explain` in prism-core. */
export const EXPLAIN_INTENT = 'explain';
/** How `ask_text` in prism-core's `ask.rs` starts an item's reference line and its quoted lines. */
const REFERENCE_PREFIX = 'Item';
const QUOTE_PREFIX = '> ';

/** Of an item's questions, the ordinary ones: explain requests show as its explanation instead. */
export function askedQuestions(questions) {
  return questions.filter((question) => question.intent !== EXPLAIN_INTENT);
}

/**
 * How long an explain request on its way (shared or submitted, no answer) may wait before the
 * item says that no explanation arrived. Measured from its request ID, which the app makes
 * time-ordered (UUID v7); a request with another ID never times out.
 */
export const EXPLAIN_TIMEOUT_MS = 10 * 60 * 1000;
/** Outcomes that mean the request never reached the conversation. */
const UNDELIVERED = new Set(['failed', 'refused', 'dropped', 'unknown']);
/** The UUID version digit of a time-ordered ID, and its 48-bit time in milliseconds. */
const V7_VERSION_INDEX = 14;
const V7_VERSION = '7';

/**
 * What each state of an item's latest explain request shows: `badge` on its card (in capitals
 * on its box), `text` in the side pane. `waiting`: on its way; `prompt`: placed in the CLI
 * prompt box because it could not be submitted; `failed`: not delivered; `lost`: no answer
 * before the conversation ended or EXPLAIN_TIMEOUT_MS passed; `done`: explained.
 */
export const EXPLANATION_STATES = {
  waiting: { badge: 'explaining…', text: () => 'Explaining…' },
  prompt: { badge: 'in prompt box', text: () => 'In the CLI prompt box: press Enter there' },
  failed: { badge: 'not delivered', text: (reason) => `Could not be delivered${reason ? `: ${reason}` : ''}; E asks again` },
  lost: { badge: 'not explained', text: () => 'No explanation arrived; E asks again' },
  done: { badge: 'explained', text: () => null },
};

/** When a time-ordered request ID was made, in milliseconds since the epoch; null for another ID. */
export function requestTime(id) {
  if (id[V7_VERSION_INDEX] !== V7_VERSION) {
    return null;
  }
  return parseInt(id.slice(0, 8) + id.slice(9, 13), 16);
}

/**
 * What an item shows of its explain requests, from its questions the latest first
 * (`questionsFor`): `status`, the state of the latest one (EXPLANATION_STATES), with the
 * `reason` of an outcome that failed, and `answered`, the latest one with an answer, which
 * replaces every older explanation. `ended`: the conversation ended; `now`: the time.
 * Null when it was never explained.
 */
export function explanationOf(questions, { ended = false, now = Date.now() } = {}) {
  const requests = questions.filter((question) => question.intent === EXPLAIN_INTENT);
  if (requests.length === 0) {
    return null;
  }
  const latest = requests[0];
  return {
    status: latest.answer ? 'done' : requestStatus(latest, ended, now),
    reason: latest.outcome.reason ?? null,
    answered: requests.find((question) => question.answer) ?? null,
  };
}

function requestStatus(question, ended, now) {
  if (UNDELIVERED.has(question.outcome.state)) {
    return 'failed';
  }
  if (ended) {
    return 'lost';
  }
  if (question.outcome.state === 'submitted') {
    return 'prompt';
  }
  const time = requestTime(question.request_id);
  return time !== null && now - time > EXPLAIN_TIMEOUT_MS ? 'lost' : 'waiting';
}

/** When the next explain request on its way among `questions` times out, or null. */
export function nextExplainTimeout(questions, now = Date.now()) {
  const due = questions
    .filter((question) => question.intent === EXPLAIN_INTENT && !question.answer && !UNDELIVERED.has(question.outcome.state) && question.outcome.state !== 'submitted')
    .map((question) => requestTime(question.request_id))
    .filter((time) => time !== null && time + EXPLAIN_TIMEOUT_MS >= now)
    .map((time) => time + EXPLAIN_TIMEOUT_MS);
  return due.length > 0 ? Math.min(...due) : null;
}

/** The text a question quoted of the item it numbered `number`: the quoted lines below its reference line. */
export function quotedText(question, number) {
  const lines = question.text.split('\n');
  const start = lines.findIndex((line) => line.startsWith(`${REFERENCE_PREFIX} ${number}: `));
  if (start === -1) {
    return '';
  }
  const quoted = [];
  for (const line of lines.slice(start + 1)) {
    if (!line.startsWith(QUOTE_PREFIX)) {
      break;
    }
    quoted.push(line.slice(QUOTE_PREFIX.length));
  }
  return quoted.join('\n');
}

/**
 * The question typed in the app's former question editor: what follows the quoted reference
 * after a blank line. Empty for questions asked since, whose question is typed in the CLI.
 */
export function typedQuestion(question) {
  const separator = question.text.indexOf('\n\n');
  return separator === -1 ? '' : question.text.slice(separator + 2).trim();
}

/** Documents in a stable order: by title, then ID. */
export function sortedDocuments(documents) {
  return [...documents].sort((left, right) => left.title.localeCompare(right.title) || byId(left, right));
}

/**
 * The open or the closed documents of a tab's content, in a stable order. Whether a document
 * is closed is shared state from the store (`content.closed`), the same in every window.
 */
export function documentsOf(content, closed) {
  const closedIds = new Set(content?.closed ?? []);
  return sortedDocuments((content?.documents ?? []).filter((document) => closedIds.has(document.id) === closed));
}
