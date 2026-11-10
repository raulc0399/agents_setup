import { MOD_KEY, h } from './dom.js';
import { questionsFor, typedQuestion } from './rows.js';

/**
 * What a shared question's outcome means for the user. Only shared questions cross to the
 * app: `pending` is shared and nothing more is known (the Claude mod places it in the prompt
 * box, Codex retrieves it when asked), `submitted` is the mod's report that it filled the
 * prompt box. `short` goes on a collapsed card, `label` and the outcome's reason under the
 * question; `tone` colors both.
 */
const QUESTION_STATES = {
  pending: { short: 'shared', label: 'Shared with the conversation', tone: 'shared' },
  submitted: { short: 'in prompt box', label: 'Placed in the CLI prompt box', tone: 'prompt' },
  queued: { short: 'queued', label: 'Queued; start not confirmed', tone: 'warning' },
  started: { short: 'started', label: 'Started', tone: 'started' },
  failed: { short: 'failed', label: 'Failed', tone: 'error' },
  refused: { short: 'refused', label: 'Refused', tone: 'error' },
  dropped: { short: 'dropped', label: 'Dropped', tone: 'error' },
  unknown: { short: 'unknown', label: 'Unknown', note: 'Sending again may duplicate it.', tone: 'warning' },
};
/** An answer recorded by the agent wins over the delivery outcome. */
const ANSWERED = { short: 'answered', label: 'Answered', tone: 'answered' };
export const ACCEPT_NOTE = 'Acceptance is feedback for the agent, never permission to run tools.';

/**
 * The list view of one tab: a card per row, the focused card expanded with its actions and,
 * when asking, the question editor. Cards are kept per item ID and rebuilt only when what
 * they show changed, so updates do not shift the layout or steal the editor's focus.
 *
 * `actions`: onFocus(id), onToggle(id), onAsk(), onAccept(), onHide(), onDrop(),
 * onSend(question), onCancelAsk(), onDraft(text).
 */
export class ListView {
  constructor(actions) {
    this.actions = actions;
    this.cards = new Map();
    this.list = h('div', { class: 'cards', role: 'list', 'aria-label': 'Items' });
    this.element = h('div', { class: 'view', tabindex: '0' }, this.list);
    this.editor = null;
  }

  /**
   * `model`: {document, rows, focus, selected: Set of selected item IDs, questions,
   * ask: {id, text, quoteLabel} | null, readOnly: why commands are unavailable, or null
   * while the conversation is live}.
   */
  render(model) {
    const seen = new Set();
    model.rows.forEach((row, index) => {
      seen.add(row.id);
      const focused = row.id === model.focus;
      const selected = model.selected.has(row.id);
      const asking = focused && model.ask?.id === row.id;
      const questions = questionsFor(model.questions, model.document.id, row.id);
      // The focused card's buttons name how many items they act on.
      const selection = focused ? model.selected.size : null;
      const signature = JSON.stringify([row, focused, selected, selection, asking, asking && [model.ask.quoteLabel, model.ask.sending], model.readOnly, questions]);
      let entry = this.cards.get(row.id);
      if (!entry) {
        entry = {
          element: h('div', {
            class: 'card',
            role: 'listitem',
            onclick: (event) => (event.metaKey || event.ctrlKey ? this.actions.onToggle(row.id) : this.actions.onFocus(row.id)),
          }),
        };
        this.cards.set(row.id, entry);
      }
      if (entry.signature !== signature) {
        entry.signature = signature;
        this.fill(entry.element, row, { focused, selected, selection, ask: asking ? model.ask : null, questions, readOnly: model.readOnly });
      }
      if (this.list.children[index] !== entry.element) {
        this.list.insertBefore(entry.element, this.list.children[index] ?? null);
      }
    });

    for (const [id, entry] of this.cards) {
      if (!seen.has(id)) {
        entry.element.remove();
        this.cards.delete(id);
      }
    }
  }

  /** Keeps the focused card in sight after keyboard navigation. */
  reveal(id) {
    this.cards.get(id)?.element.scrollIntoView({ block: 'nearest' });
  }

  /** The text the user selected inside the item's own body, if both ends of it are there. */
  selectedText(id) {
    return selectionInside(this.cards.get(id)?.element.querySelector('.card-body'));
  }

  focusEditor() {
    this.editor?.querySelector('textarea')?.focus();
  }

  fill(card, row, { focused, selected, selection, ask, questions, readOnly }) {
    card.className = `card kind-${row.kind}${focused ? ' focused' : ''}${selected ? ' selected' : ''}`;
    if (focused) {
      card.setAttribute('aria-current', 'true');
    } else {
      card.removeAttribute('aria-current');
    }
    card.setAttribute('aria-label', `${selected ? 'Selected: ' : ''}${row.number} ${row.kindLabel} ${row.title}`);
    card.dataset.id = row.id;

    const badges = acceptanceBadges(row);
    if (row.changed) {
      badges.push(h('span', { class: 'badge changed', text: 'new' }));
    }
    if (questions.length > 0) {
      const latest = questionState(questions[0]);
      badges.push(
        h('span', {
          class: `badge asked tone-${latest.tone}`,
          title: `Latest question: ${latest.text}`,
          text: `${questions.length === 1 ? 'asked' : `asked ×${questions.length}`} · ${latest.short}`,
        }),
      );
    }
    const head = h(
      'div',
      { class: 'card-head' },
      h('span', { class: 'number', text: String(row.number) }),
      h('span', { class: 'kind', text: row.kindLabel }),
      h('span', { class: 'title', text: row.title }),
      h('span', { class: 'badges' }, badges),
    );

    if (!focused) {
      card.replaceChildren(...[head, row.excerpt ? h('div', { class: 'excerpt', text: row.excerpt }) : null].filter(Boolean));
      return;
    }

    const parts = [head, h('div', { class: 'card-body', text: row.body })];
    if (row.source) {
      parts.push(h('div', { class: 'card-source', text: row.source }));
    }
    // Rebuilding the asking card (its item changed) keeps the draft, the keyboard and the caret.
    const previous = this.editor?.querySelector('textarea');
    const typing = ask && previous && document.activeElement === previous;
    const caret = typing ? [previous.selectionStart, previous.selectionEnd] : null;
    this.editor = ask ? askEditor(ask, readOnly, this.actions) : null;
    // Actions before the questions, as in the diagram panel.
    parts.push(this.editor ?? h('div', { class: 'card-actions' }, itemActions(selection, readOnly, this.actions), h('span', { class: 'hint', text: readOnly ?? '' })));
    for (const question of questions) {
      parts.push(questionBlock(question));
    }
    card.replaceChildren(...parts.filter(Boolean));
    if (typing) {
      const textarea = this.editor.querySelector('textarea');
      textarea.focus();
      textarea.setSelectionRange(...caret);
    }
  }
}

/** The selected text if both of its ends lie inside `container`; empty otherwise. */
export function selectionInside(container) {
  const selection = window.getSelection();
  const inside = container && selection && container.contains(selection.anchorNode) && container.contains(selection.focusNode);
  if (!inside || selection.isCollapsed) {
    return '';
  }
  return selection.toString();
}

/** "accepted" or "changed since accepted", as cards and the diagram panel show them. */
export function acceptanceBadges(row) {
  const badges = [];
  if (row.isAccepted) {
    badges.push(h('span', { class: 'badge accepted', title: `Accepted. ${ACCEPT_NOTE}`, text: 'accepted' }));
  }
  if (row.staleAcceptance) {
    badges.push(h('span', { class: 'badge stale', title: 'Changed after it was accepted; accept it again to confirm', text: 'changed since accepted' }));
  }
  return badges;
}

/**
 * Ask, Accept, Hide and Drop for the focused item or, when `selection` items are selected,
 * for those; the labels say which.
 */
export function itemActions(selection, readOnly, actions) {
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
export function askEditor(ask, readOnly, actions) {
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

/** `short`, the full `text` with the outcome's reason, and the `tone` of a shared question. */
export function questionState(question) {
  const outcome = question.outcome;
  const state = question.answer ? ANSWERED : (QUESTION_STATES[outcome.state] ?? { short: outcome.state, label: outcome.state, tone: 'warning' });
  const detail = question.answer ? null : (outcome.reason ?? outcome.turn_id ?? null);
  const text = [detail ? `${state.label}: ${detail}` : state.label, state.note].filter(Boolean).join('. ');
  return { short: state.short, text, tone: state.tone };
}

/** A shared question with its delivery state and answer, as cards and the diagram panel show it. */
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
