import { MOD_KEY, h } from './dom.js';
import { ACCEPT_NOTE, acceptanceBadges, questionState } from './list.js';
import { questionsFor, typedQuestion } from './rows.js';

/**
 * The detail area of one tab's side pane, in the list and in the diagram: the focused item
 * with its text, actions or ask editor, and its questions. Without a focused item (only in
 * the diagram, where the user picks one) it shows an overview of the diagram.
 *
 * `actions`: onAsk(), onAccept(), onHide(), onDrop(), onSend(question), onCancelAsk(),
 * onDraft(text).
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
   * item IDs, questions, ask: {text, quoteLabel, sending} | null, readOnly: why commands are
   * unavailable, or null, counts: [boxes, arrows, badges] of the diagram, or null}.
   * Rebuilt only when what it shows changed; a rebuild keeps the draft, the keyboard and the caret.
   */
  render(model) {
    this.model = model;
    const { ask, readOnly, counts } = model;
    const row = model.rows.find((candidate) => candidate.id === model.focus) ?? null;
    const selected = model.rows.filter((candidate) => model.selected.has(candidate.id));
    const questions = row ? questionsFor(model.questions, model.document.id, row.id) : [];
    const signature = JSON.stringify([row, questions, selected.map((item) => [item.number, item.title]), ask ? [ask.quoteLabel, ask.sending] : null, readOnly, row ? null : [model.document.title, counts]]);
    if (signature === this.signature) {
      return;
    }
    this.signature = signature;
    const previous = this.element.querySelector('.ask textarea');
    const typing = ask && previous && document.activeElement === previous;
    const caret = typing ? [previous.selectionStart, previous.selectionEnd] : null;
    const actions = row || selected.length > 0 || ask ? this.panelActions(selected.length, ask, readOnly) : null;
    this.element.replaceChildren(
      ...(row ? itemPanel(row, questions, actions) : overviewPanel(model.document.title, counts, selected, actions)),
    );
    if (typing) {
      const textarea = this.element.querySelector('.ask textarea');
      textarea.focus();
      textarea.setSelectionRange(...caret);
    }
  }

  focusEditor() {
    this.element.querySelector('.ask textarea')?.focus();
  }

  /** The text the user selected inside the panel's text of the focused item. */
  selectedText(id) {
    return id === this.model?.focus ? selectionInside(this.element.querySelector('.panel-body')) : '';
  }

  /** The ask editor while asking, else the buttons for the focused or selected items. */
  panelActions(selection, ask, readOnly) {
    if (ask) {
      return askEditor(ask, readOnly, this.actions);
    }
    return h('div', { class: 'panel-actions' }, itemActions(selection, readOnly, this.actions), readOnly ? h('span', { class: 'hint', text: readOnly }) : null);
  }
}

/** The focused item; `actions` (buttons or the ask editor) come before its questions. */
function itemPanel(row, questions, actions) {
  const badges = acceptanceBadges(row);
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
 * Ask, Accept, Hide and Drop for the focused item or, when `selection` items are selected,
 * for those; the labels say which.
 */
function itemActions(selection, readOnly, actions) {
  const many = selection > 0;
  const what = many ? `the ${selection} selected ${selection === 1 ? 'item' : 'items'}` : 'this item';
  const count = many ? ` ${selection}` : '';
  const button = (className, text, title, action, shared = true) =>
    h('button', {
      class: `button ${className}`,
      disabled: shared && Boolean(readOnly),
      title: shared && readOnly ? readOnly : title,
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
    button('', `Hide${count}`, `Hide ${what} in this tab (H)`, actions.onHide, false),
    button('danger', `Drop${count}`, `Drop ${what} for everyone; U undoes (X)`, actions.onDrop),
  ];
}

/**
 * The question editor: what it quotes, the typed question, send and cancel.
 * `ask`: {text, quoteLabel, sending}; `actions`: onSend(question), onCancelAsk(), onDraft(text).
 */
function askEditor(ask, readOnly, actions) {
  const textarea = h('textarea', {
    placeholder: 'Your question (optional): the CLI prompt box receives the items, the quote and this text',
    'aria-label': 'Question',
    value: ask.text,
    oninput: () => actions.onDraft(textarea.value),
    onkeydown: (event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        actions.onCancelAsk();
      } else if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        actions.onSend(textarea.value);
      }
    },
  });
  return h(
    'div',
    { class: 'ask', onclick: (event) => event.stopPropagation() },
    h('div', { class: 'quote', title: ask.quoteLabel, text: ask.quoteLabel }),
    textarea,
    h(
      'div',
      { class: 'row' },
      h('span', { class: 'hint' }, h('kbd', { text: MOD_KEY }), ' ', h('kbd', { text: 'Enter' }), ' sends · ', h('kbd', { text: 'Esc' }), ' cancels'),
      h('button', { class: 'button', text: 'Cancel', onclick: () => actions.onCancelAsk() }),
      h('button', {
        class: 'button primary',
        disabled: Boolean(readOnly) || ask.sending,
        text: ask.sending ? 'Sending…' : 'Send to CLI',
        onclick: () => actions.onSend(textarea.value),
      }),
    ),
  );
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
