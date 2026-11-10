import { firstLine } from './format.js';

/**
 * What the list shows for a document, as the terminal list did: elements in publication
 * order, then links, then groups; archived, hidden and filtered-out ones left out.
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

function records(document) {
  const titleOf = (id) => document.elements[id]?.title ?? 'missing';
  const elements = Object.values(document.elements)
    .filter((item) => !item.archived)
    .sort((left, right) => (left.order ?? 0) - (right.order ?? 0) || byId(left, right))
    .map((item) => ({
      id: item.id,
      kind: item.kind,
      title: item.title,
      excerpt: firstLine(item.body),
      body: item.body,
      source: item.source,
      details: item.source ? `${item.body}\nSource: ${item.source}` : item.body,
      revision: item.semantic_revision,
      accepted: item.accepted_revision,
    }));
  const links = Object.values(document.relationships)
    .filter((item) => !item.archived)
    .sort(byId)
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
      };
    });
  const groups = Object.values(document.groups)
    .filter((item) => !item.archived)
    .sort(byId)
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
      };
    });
  return [...elements, ...links, ...groups];
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
      changed: hasOlderContent && record.revision === latest && record.accepted !== record.revision,
    }));
}

/** Number of items a document has that are not archived, hidden or not. */
export function itemCount(document) {
  return records(document).length;
}

/** IDs of the items a document has that are not archived, hidden or not. */
export function activeIds(document) {
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

/** The question the user typed: what follows the quoted reference after a blank line. */
export function typedQuestion(question) {
  const separator = question.text.indexOf('\n\n');
  return separator === -1 ? '' : question.text.slice(separator + 2).trim();
}

/** Documents in a stable order: by title, then ID. */
export function sortedDocuments(documents) {
  return [...documents].sort((left, right) => left.title.localeCompare(right.title) || byId(left, right));
}
