import { MOD_KEY, h } from './dom.js';
import { ACCEPT_NOTE, DROPPED_NOTE, stateBadges, questionState } from './list.js';
import { questionsFor, typedQuestion } from './rows.js';

/**
 * The detail area of one tab's side pane, in the list and in the diagram: the focused item
 * with its text, actions and questions. Without a focused item (only in the diagram, where
 * the user picks one) it shows an overview of the diagram.
 *
 * `actions`: onAsk(), onAccept(), onDrop().
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
   * item IDs, questions, readOnly: why commands are unavailable, or null, counts: [boxes,
   * arrows, badges] of the diagram, or null}. Rebuilt only when what it shows changed.
   */
  render(model) {
    this.model = model;
    const { readOnly, counts } = model;
    const row = model.rows.find((candidate) => candidate.id === model.focus) ?? null;
    const selected = model.rows.filter((candidate) => model.selected.has(candidate.id));
    const questions = row ? questionsFor(model.questions, model.document.id, row.id) : [];
    const signature = JSON.stringify([row, questions, selected.map((item) => [item.number, item.title, item.dropped]), readOnly, row ? null : [model.document.title, counts]]);
    if (signature === this.signature) {
      return;
    }
    this.signature = signature;
    const actions = row || selected.length > 0 ? this.panelActions(row, selected, readOnly) : null;
    this.element.replaceChildren(
      ...(row ? itemPanel(row, questions, actions) : overviewPanel(model.document.title, counts, selected, actions)),
    );
  }

  /** The text the user selected inside the panel's text of the focused item. */
  selectedText(id) {
    return id === this.model?.focus ? selectionInside(this.element.querySelector('.panel-body')) : '';
  }

  /**
   * The buttons for the selected items, else the focused one. Dropped items take no part:
   * with only dropped ones the buttons are disabled and say why.
   */
  panelActions(row, selected, readOnly) {
    const actionable = selected.filter((item) => !item.dropped).length;
    const onlyDropped = selected.length > 0 ? actionable === 0 : Boolean(row?.dropped);
    const unavailable = readOnly ?? (onlyDropped ? DROPPED_NOTE : null);
    const leftOut = selected.length - actionable;
    const hint = unavailable ?? (selected.length > 0 && leftOut > 0 ? `${leftOut} dropped ${leftOut === 1 ? 'item takes' : 'items take'} no part` : null);
    return h(
      'div',
      { class: 'panel-actions' },
      itemActions(selected.length > 0 ? actionable : 0, unavailable, this.actions),
      hint ? h('span', { class: 'hint', text: hint }) : null,
    );
  }
}

/** The focused item; its `actions` come before its questions. */
function itemPanel(row, questions, actions) {
  const badges = stateBadges(row);
  return [
    h('div', { class: `panel-tag kind-${row.kind}` }, h('span', { class: 'kind', text: row.kindLabel }), h('span', { class: 'number', text: String(row.number) })),
    h('h2', { class: 'panel-title', text: row.title }),
    badges.length > 0 ? h('div', { class: 'badges' }, badges) : null,
    row.body ? h('div', { class: 'panel-body', text: row.body }) : null,
    row.source ? h('div', { class: 'card-source', text: row.source }) : null,
    actions,
    ...questions.map(questionBlock),
  ].filter(Boolean);
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
      'Click a box, an arrow’s number or a badge, or press ',
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
 * Ask, Accept and Drop for the focused item or, when `selection` items are selected, for
 * those; the labels say which. `unavailable`: why the buttons are disabled, or null.
 */
function itemActions(selection, unavailable, actions) {
  const many = selection > 0;
  const what = many ? `the ${selection} selected ${selection === 1 ? 'item' : 'items'}` : 'this item';
  const count = many ? ` ${selection}` : '';
  const button = (className, text, title, action) =>
    h('button', {
      class: `button ${className}`,
      disabled: Boolean(unavailable),
      title: unavailable ?? title,
      text,
      onclick: (event) => {
        event.stopPropagation();
        action();
      },
    });
  const ask = button('primary', many ? `Ask about${count}` : 'Ask', `Ask about ${what} (Enter)`, actions.onAsk);
  // Keeps a text selection inside the item, which becomes the quote.
  ask.addEventListener('mousedown', (event) => event.preventDefault());
  return [
    ask,
    button('', `Accept${count}`, `Accept ${what}. ${ACCEPT_NOTE} (A)`, actions.onAccept),
    button('danger', `Drop${count}`, `Drop ${what} for everyone; U undoes (X)`, actions.onDrop),
  ];
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
