import { MOD_KEY, h } from './dom.js';
import { ACCEPT_NOTE, BOOKMARK_NOTE, DROPPED_NOTE, bookmarkIcon, kindText, stateBadges, questionState } from './list.js';
import { EXPLANATION_STATES, askedQuestions, explanationOf, itemBookmarks, itemMark, questionsFor, quoteFound, quotedText, typedQuestion } from './rows.js';

/** The pane's section with an item's explanation. */
const EXPLANATION_TITLE = 'Explanation';
/** The pane's section with an item's bookmarks, and what a bookmark of the whole item says there. */
const BOOKMARKS_TITLE = 'Bookmarks';
const WHOLE_ITEM = 'The whole item';
/** A bookmark's quote the item's text no longer contains. */
const TEXT_CHANGED = 'text changed';

/**
 * The detail area of one tab's side pane, in the list and in the diagram: the focused item
 * with its text, actions and questions. Without a focused item (only in the diagram, where
 * the user picks one) it shows an overview of the diagram.
 *
 * `actions`: onAsk(), onExplain(), onBookmark(), onAccept(), onDrop(), onRemoveBookmark(id).
 */
export class DetailPanel {
  constructor(actions) {
    this.actions = actions;
    this.model = null;
    this.signature = null;
    // Focusable, so that its text can be scrolled with the keyboard.
    this.element = h('div', { class: 'detail-panel', role: 'region', tabindex: '0', 'aria-label': 'Item details' });
  }

  /**
   * `model`: {document, rows, focus: the focused item's ID or null, selected: Set of selected
   * item IDs, questions, readOnly: why commands are unavailable, or null, explainBlocked: why
   * Explain alone is, or null, ended and now: for the state of explain requests, counts:
   * [boxes, arrows, badges] of the diagram, or null}. Rebuilt only when what it shows changed.
   */
  render(model) {
    this.model = model;
    const { readOnly, explainBlocked = null, counts } = model;
    const row = model.rows.find((candidate) => candidate.id === model.focus) ?? null;
    const selected = model.rows.filter((candidate) => model.selected.has(candidate.id));
    const questions = row ? questionsFor(model.questions, model.document.id, row.id) : [];
    const explanation = explanationOf(questions, model);
    const bookmarks = row ? itemBookmarks(model.document, row.id) : [];
    // Bookmark is pressed when every item it acts on is bookmarked as a whole.
    const marked = (selected.length > 0 ? selected : [row]).every((item) => item && itemMark(model.document, item.id));
    const signature = JSON.stringify([row, questions, explanation?.status, bookmarks, marked, selected.map((item) => [item.number, item.title, item.dropped]), readOnly, explainBlocked, row ? null : [model.document.title, counts]]);
    if (signature === this.signature) {
      return;
    }
    this.signature = signature;
    const actions = row || selected.length > 0 ? this.panelActions(row, selected, readOnly, explainBlocked, marked) : null;
    this.element.replaceChildren(
      ...(row
        ? itemPanel(row, questions, explanation, actions, bookmarks.length > 0 ? bookmarksSection(row, bookmarks, readOnly, this.actions.onRemoveBookmark) : null)
        : overviewPanel(model.document.title, counts, selected, actions)),
    );
  }

  /** The text the user selected inside the panel's text of the focused item. */
  selectedText(id) {
    return id === this.model?.focus ? selectionInside(this.element.querySelector('.panel-body')) : '';
  }

  /** Whether a selection reaches into the focused item's text from beyond it, so it is no quote of it. */
  selectionBeyond(id) {
    const body = this.element.querySelector('.panel-body');
    const selection = window.getSelection();
    if (id !== this.model?.focus || !body || !selection || selection.isCollapsed || selection.rangeCount === 0) {
      return false;
    }
    return selection.getRangeAt(0).intersectsNode(body) && !selectionInside(body);
  }

  /**
   * The buttons for the selected items, else the focused one. Dropped items take no part:
   * with only dropped ones the buttons are disabled and say why; `explainBlocked` disables
   * Explain alone. Bookmark takes dropped items too and is pressed when all are `marked`.
   */
  panelActions(row, selected, readOnly, explainBlocked, marked) {
    const actionable = selected.filter((item) => !item.dropped).length;
    const onlyDropped = selected.length > 0 ? actionable === 0 : Boolean(row?.dropped);
    const unavailable = readOnly ?? (onlyDropped ? DROPPED_NOTE : null);
    const leftOut = selected.length - actionable;
    const hint = unavailable ?? (selected.length > 0 && leftOut > 0 ? `${leftOut} dropped ${leftOut === 1 ? 'item takes' : 'items take'} no part` : null);
    return h(
      'div',
      { class: 'panel-actions' },
      itemActions(selected.length > 0 ? actionable : 0, unavailable, explainBlocked, { count: selected.length, marked, readOnly }, this.actions),
      hint ? h('span', { class: 'hint', text: hint }) : null,
    );
  }
}

/** The focused item; its `actions` come before its `bookmarks`, its `explanation` and its questions. */
function itemPanel(row, questions, explanation, actions, bookmarks) {
  const badges = stateBadges(row);
  return [
    h('div', { class: `panel-tag kind-${row.kind}` }, h('span', { class: 'kind', text: kindText(row) }), h('span', { class: 'number', text: row.step !== null ? `· link ${row.number}` : String(row.number) })),
    h('h2', { class: 'panel-title', text: row.title }),
    badges.length > 0 ? h('div', { class: 'badges' }, badges) : null,
    row.body ? h('div', { class: 'panel-body', text: row.body }) : null,
    row.source ? h('div', { class: 'card-source', text: row.source }) : null,
    actions,
    bookmarks,
    explanation ? explanationSection(row, explanation) : null,
    ...askedQuestions(questions).map(questionBlock),
  ].filter(Boolean);
}

/**
 * The latest explanation of an item as plain text, a paragraph per block of lines, below the
 * text its request quoted when that was a selection in this item (`quoted`), not the whole
 * item. While a newer request is not answered, its state (EXPLANATION_STATES) comes first;
 * the older explanation stays until the newer one arrives.
 */
function explanationSection(row, { status, reason, answered }) {
  const quote = answered?.quoted === row.id ? quotedText(answered, row.number) : '';
  const state = EXPLANATION_STATES[status].text(reason);
  return h(
    'section',
    { class: 'panel-explanation', 'aria-label': EXPLANATION_TITLE },
    h('h3', { text: EXPLANATION_TITLE }),
    state ? h('p', { class: `explanation-state state-${status}`, text: state }) : null,
    quote ? h('blockquote', { class: 'explanation-quote', text: quote }) : null,
    answered ? h('div', { class: 'explanation-text' }, paragraphs(answered.answer)) : null,
  );
}

/**
 * An item's bookmarks: the whole item, and each quoted selection as written, marked when the
 * item's text no longer contains it; each with its remove control, unavailable while `readOnly`.
 */
function bookmarksSection(row, bookmarks, readOnly, onRemove) {
  return h(
    'section',
    { class: 'panel-bookmarks', 'aria-label': BOOKMARKS_TITLE },
    h('h3', {}, bookmarkIcon(), `${BOOKMARKS_TITLE} (${bookmarks.length})`),
    h(
      'ul',
      {},
      bookmarks.map((bookmark) =>
        h(
          'li',
          { class: 'panel-bookmark', 'data-id': bookmark.id },
          bookmark.quote ? h('blockquote', { class: 'bookmark-quote', text: bookmark.quote }) : h('span', { class: 'bookmark-whole', text: WHOLE_ITEM }),
          quoteFound(row, bookmark) ? null : h('span', { class: 'chip text-changed', title: 'The item’s text no longer contains this quote', text: TEXT_CHANGED }),
          h('button', {
            class: 'close',
            disabled: Boolean(readOnly),
            title: readOnly ?? 'Remove this bookmark for everyone; U undoes',
            'aria-label': `Remove the bookmark${bookmark.quote ? ` of “${bookmark.quote}”` : ' of the whole item'}`,
            text: '×',
            onclick: (event) => {
              event.stopPropagation();
              onRemove(bookmark.id);
            },
          }),
        ),
      ),
    ),
  );
}

/** Blocks of lines separated by blank lines, each a paragraph that keeps its line breaks. */
function paragraphs(text) {
  return text
    .trim()
    .split(/\n[ \t]*\n/)
    .filter((paragraph) => paragraph.trim() !== '')
    .map((paragraph) => h('p', { text: paragraph }));
}

function overviewPanel(title, counts, selected, actions) {
  const [boxes, arrows, badges] = counts ?? [0, 0, 0];
  const chip = (count, one, many) => h('span', { class: 'chip', text: `${count} ${count === 1 ? one : many}` });
  return [
    h('div', { class: 'panel-tag' }, h('span', { class: 'kind', text: 'Diagram' })),
    h('h2', { class: 'panel-title', text: title }),
    counts ? h('div', { class: 'chips' }, chip(boxes, 'box', 'boxes'), chip(arrows, 'arrow', 'arrows'), chip(badges, 'badge', 'badges')) : null,
    selected.length > 0
      ? h(
          'div',
          { class: 'panel-selection' },
          h('h3', { text: `${selected.length} selected` }),
          h('ul', {}, selected.map((item) => h('li', {}, h('span', { class: 'number', text: String(item.number) }), ' ', item.title))),
        )
      : null,
    actions,
    h(
      'p',
      { class: 'hint' },
      'Click a box, an arrow’s step number or a badge, or press ',
      h('kbd', { text: 'Tab' }),
      '; ',
      h('kbd', { text: 'Space' }),
      ' or ',
      h('kbd', { text: MOD_KEY }),
      ' and click selects several. Drag or scroll to pan; ',
      h('kbd', { text: MOD_KEY }),
      ' and scroll or pinch to zoom.',
    ),
  ].filter(Boolean);
}

/** The selected text if both of its ends lie inside `container`; empty otherwise. */
function selectionInside(container) {
  const selection = window.getSelection();
  const inside = container && selection && container.contains(selection.anchorNode) && container.contains(selection.focusNode);
  if (!inside || selection.isCollapsed) {
    return '';
  }
  return selection.toString();
}

/**
 * Ask, Explain, Bookmark, Accept and Drop for the focused item or, when `selection` items are
 * selected, for those; the labels say which. `unavailable`: why the buttons are disabled, or
 * null; `explainBlocked`: why Explain is, or null. `bookmark`: {count of the selected items,
 * dropped ones included, whether all are `marked`, `readOnly`}, as Bookmark acts on them all.
 */
function itemActions(selection, unavailable, explainBlocked, bookmark, actions) {
  const many = selection > 0;
  const what = many ? `the ${selection} selected ${selection === 1 ? 'item' : 'items'}` : 'this item';
  // The count shows only for several items; one selected item reads like the focused one.
  const count = selection > 1 ? ` (${selection})` : '';
  const button = (className, text, title, action, blocked = null) =>
    h('button', {
      class: `button ${className}`,
      disabled: Boolean(unavailable ?? blocked),
      title: unavailable ?? blocked ?? title,
      text,
      onclick: (event) => {
        event.stopPropagation();
        action();
      },
    });
  const ask = button('primary', `Ask${count}`, `Ask about ${what} (Enter)`, actions.onAsk);
  const explain = button('', `Explain${count}`, `Explain ${what}; the explanation shows here (E)`, actions.onExplain, explainBlocked);
  const marking = bookmarkButton(bookmark, actions.onBookmark);
  // Keeps a text selection inside the item, which becomes the quote.
  for (const quoting of [ask, explain, marking]) {
    quoting.addEventListener('mousedown', (event) => event.preventDefault());
  }
  return [
    ask,
    explain,
    marking,
    button('', `Accept${count}`, `Accept ${what}. ${ACCEPT_NOTE} (A)`, actions.onAccept),
    button('danger', `Drop${count}`, `Drop ${what} for everyone; U undoes (X)`, actions.onDrop),
  ];
}

/**
 * Bookmark: a toggle for the items it acts on, available for dropped ones too, not while
 * `readOnly`. It shows the ribbon and the count, its name stays for assistive technology, so
 * the five actions keep to one line in the narrowest pane.
 */
function bookmarkButton({ count, marked, readOnly }, onBookmark) {
  const what = count > 0 ? `the ${count} selected ${count === 1 ? 'item' : 'items'}` : 'this item';
  const title = `${marked ? 'Remove the bookmark of' : 'Bookmark'} ${what}; with text selected in the item, bookmark that text (B). ${BOOKMARK_NOTE}`;
  return h('button', {
    class: 'button bookmark',
    'aria-pressed': String(marked),
    disabled: Boolean(readOnly),
    title: readOnly ?? title,
    onclick: (event) => {
      event.stopPropagation();
      onBookmark();
    },
  }, bookmarkIcon(), h('span', { class: 'visually-hidden', text: 'Bookmark' }), count > 1 ? ` (${count})` : null);
}

/** A shared question with its delivery state and answer, as the detail panel shows it. */
export function questionBlock(question) {
  const typed = typedQuestion(question);
  const state = questionState(question);
  const items = question.snapshot.selection.ids.length;
  return h(
    'div',
    { class: `card-question tone-${state.tone}` },
    h('div', { class: 'q', text: typed || 'Shared as a reference' }),
    question.answer ? h('div', { class: 'a', text: question.answer }) : null,
    h('div', { class: 'state', text: items > 1 ? `${state.text} · asked about ${items} items` : state.text }),
  );
}
