import { MOD_KEY, h } from './dom.js';
import { questionsFor, typedQuestion } from './rows.js';

const OUTCOME_LABELS = {
  pending: 'shared',
  submitted: 'sent',
  queued: 'queued',
  started: 'started',
  failed: 'failed',
  refused: 'refused',
  dropped: 'dropped',
  unknown: 'unknown',
};

/**
 * The list view of one tab: a card per row, the focused card expanded with its actions and,
 * when asking, the question editor. Cards are kept per item ID and rebuilt only when what
 * they show changed, so updates do not shift the layout or steal the editor's focus.
 *
 * `actions`: onFocus(id), onAsk(row), onHide(row), onDrop(row), onSend(row, question),
 * onCancelAsk(), onDraft(text).
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
   * `model`: {document, rows, focus, questions, ask: {id, text, quoteLabel} | null,
   * readOnly: why commands are unavailable, or null while the conversation is live}.
   */
  render(model) {
    const seen = new Set();
    model.rows.forEach((row, index) => {
      seen.add(row.id);
      const focused = row.id === model.focus;
      const asking = focused && model.ask?.id === row.id;
      const questions = questionsFor(model.questions, model.document.id, row.id);
      const signature = JSON.stringify([row, focused, asking, asking && model.ask.quoteLabel, model.readOnly, questions]);
      let entry = this.cards.get(row.id);
      if (!entry) {
        entry = { element: h('div', { class: 'card', role: 'listitem', onclick: () => this.actions.onFocus(row.id) }) };
        this.cards.set(row.id, entry);
      }
      if (entry.signature !== signature) {
        entry.signature = signature;
        this.fill(entry.element, row, focused, asking ? model.ask : null, questions, model.readOnly);
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
    const body = this.cards.get(id)?.element.querySelector('.card-body');
    const selection = window.getSelection();
    const inside = body && selection && body.contains(selection.anchorNode) && body.contains(selection.focusNode);
    if (!inside || selection.isCollapsed) {
      return '';
    }
    return selection.toString();
  }

  focusEditor() {
    this.editor?.querySelector('textarea')?.focus();
  }

  fill(card, row, focused, ask, questions, readOnly) {
    card.className = `card kind-${row.kind}${focused ? ' focused' : ''}`;
    if (focused) {
      card.setAttribute('aria-current', 'true');
    } else {
      card.removeAttribute('aria-current');
    }
    card.dataset.id = row.id;

    const badges = [];
    if (row.isAccepted) {
      badges.push(h('span', { class: 'badge accepted', text: 'accepted' }));
    }
    if (row.staleAcceptance) {
      badges.push(h('span', { class: 'badge stale', text: 'changed since accepted' }));
    }
    if (row.changed) {
      badges.push(h('span', { class: 'badge changed', text: 'new' }));
    }
    if (questions.length > 0) {
      badges.push(h('span', { class: 'badge asked', text: questions.length === 1 ? 'asked' : `asked ×${questions.length}` }));
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
    for (const question of questions) {
      parts.push(questionBlock(question));
    }
    // Rebuilding the asking card (its item changed) keeps the draft, the keyboard and the caret.
    const previous = this.editor?.querySelector('textarea');
    const typing = ask && previous && document.activeElement === previous;
    const caret = typing ? [previous.selectionStart, previous.selectionEnd] : null;
    parts.push(ask ? this.askEditor(row, ask, readOnly) : this.cardActions(row, readOnly));
    card.replaceChildren(...parts.filter(Boolean));
    if (typing) {
      const textarea = this.editor.querySelector('textarea');
      textarea.focus();
      textarea.setSelectionRange(...caret);
    }
  }

  cardActions(row, readOnly) {
    this.editor = null;
    return h(
      'div',
      { class: 'card-actions' },
      h('button', {
        class: 'button primary',
        disabled: Boolean(readOnly),
        title: readOnly ?? 'Ask about this item (Enter)',
        text: 'Ask',
        // Keeps a text selection inside the card, which becomes the quote.
        onmousedown: (event) => event.preventDefault(),
        onclick: (event) => {
          event.stopPropagation();
          this.actions.onAsk(row);
        },
      }),
      h('button', {
        class: 'button',
        title: 'Hide in this tab (H)',
        text: 'Hide',
        onclick: (event) => {
          event.stopPropagation();
          this.actions.onHide(row);
        },
      }),
      h('button', {
        class: 'button danger',
        disabled: Boolean(readOnly),
        title: readOnly ?? 'Drop for everyone; U undoes (X)',
        text: 'Drop',
        onclick: (event) => {
          event.stopPropagation();
          this.actions.onDrop(row);
        },
      }),
      h('span', { class: 'hint', text: readOnly ?? '' }),
    );
  }

  askEditor(row, ask, readOnly) {
    const textarea = h('textarea', {
      placeholder: 'Your question (optional): the CLI prompt box receives the item, the quote and this text',
      'aria-label': 'Question',
      value: ask.text,
      oninput: () => this.actions.onDraft(textarea.value),
      onkeydown: (event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          this.actions.onCancelAsk();
        } else if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
          event.preventDefault();
          this.actions.onSend(row, textarea.value);
        }
      },
    });
    this.editor = h(
      'div',
      { class: 'ask', onclick: (event) => event.stopPropagation() },
      h('div', { class: 'quote', text: ask.quoteLabel }),
      textarea,
      h(
        'div',
        { class: 'row' },
        h('span', { class: 'hint' }, h('kbd', { text: MOD_KEY }), ' ', h('kbd', { text: 'Enter' }), ' sends · ', h('kbd', { text: 'Esc' }), ' cancels'),
        h('button', { class: 'button', text: 'Cancel', onclick: () => this.actions.onCancelAsk() }),
        h('button', {
          class: 'button primary',
          disabled: Boolean(readOnly),
          text: 'Send to CLI',
          onclick: () => this.actions.onSend(row, textarea.value),
        }),
      ),
    );
    return this.editor;
  }
}

/** A shared question with its delivery state and answer, as cards and the diagram panel show it. */
export function questionBlock(question) {
  const typed = typedQuestion(question);
  const state = question.answer ? 'answered' : (OUTCOME_LABELS[question.outcome.state] ?? question.outcome.state);
  return h(
    'div',
    { class: 'card-question' },
    h('div', { class: 'q', text: typed || 'Shared as a reference' }),
    question.answer ? h('div', { class: 'a', text: question.answer }) : null,
    h('div', { class: 'state', text: state }),
  );
}
