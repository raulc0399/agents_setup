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
 * Visible rows, numbered from 1 in display order (the number an `ask` refers to).
 * `changed`: revised by the latest publication while older content exists.
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
      changed: hasOlderContent && record.revision === latest && record.accepted !== record.revision && !record.dropped,
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
