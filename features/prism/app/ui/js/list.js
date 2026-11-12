import { h } from './dom.js';
import { EXPLANATION_STATES, askedQuestions, explanationOf, questionsFor } from './rows.js';

/**
 * What a shared question's outcome means for the user. Only shared questions cross to the
 * app: `pending` is shared and nothing more is known (the Claude mod places it in the prompt
 * box, Codex retrieves it when asked), `submitted` is the mod's report that it filled the
 * prompt box. `short` goes on a card, `label` and the outcome's reason under the
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
/** Class and badge of a dropped item, which keeps its card. */
const DROPPED = 'dropped';
/** Why Ask, Explain, Accept and Drop refuse a dropped item; it stays in the list until U undoes the drop. */
export const DROPPED_NOTE = 'Dropped items cannot be asked about, explained, accepted or dropped again; U undoes the drop.';

/**
 * The list view of one tab: a compact card per row; the tab's side pane shows the focused
 * one with its actions. A dropped item keeps its card and number, greyed out with a
 * "dropped" badge. Cards are kept per item ID and rebuilt only when what they show changed,
 * so updates do not shift the layout.
 *
 * `actions`: onFocus(id), onToggle(id).
 */
export class ListView {
  constructor(actions) {
    this.actions = actions;
    this.cards = new Map();
    this.list = h('div', { class: 'cards', role: 'list', 'aria-label': 'Items' });
    this.element = h('div', { class: 'view', tabindex: '0' }, this.list);
  }

  /**
   * `model`: {document, rows, focus, selected: Set of selected item IDs, questions, ended:
   * whether the conversation ended, now: the time, for the state of explain requests}.
   */
  render(model) {
    const seen = new Set();
    model.rows.forEach((row, index) => {
      seen.add(row.id);
      const focused = row.id === model.focus;
      const selected = model.selected.has(row.id);
      const questions = questionsFor(model.questions, model.document.id, row.id);
      const explanation = explanationOf(questions, model);
      const signature = JSON.stringify([row, focused, selected, questions, explanation?.status]);
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
        this.fill(entry.element, row, { focused, selected, questions, explanation });
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

  fill(card, row, { focused, selected, questions, explanation }) {
    card.className = `card kind-${row.kind}${row.dropped ? ` ${DROPPED}` : ''}${focused ? ' focused' : ''}${selected ? ' selected' : ''}`;
    if (focused) {
      card.setAttribute('aria-current', 'true');
    } else {
      card.removeAttribute('aria-current');
    }
    card.setAttribute('aria-label', `${selected ? 'Selected: ' : ''}${row.number} ${row.kindLabel} ${row.title}${row.dropped ? ` (${DROPPED})` : ''}`);
    card.dataset.id = row.id;

    const badges = stateBadges(row);
    if (row.latestChange) {
      badges.push(h('span', { class: 'badge changed', text: row.latestChange }));
    }
    const asked = askedQuestions(questions);
    if (asked.length > 0) {
      const latest = questionState(asked[0]);
      badges.push(
        h('span', {
          class: `badge asked tone-${latest.tone}`,
          title: `Latest question: ${latest.text}`,
          text: `${asked.length === 1 ? 'asked' : `asked ×${asked.length}`} · ${latest.short}`,
        }),
      );
    }
    if (explanation) {
      badges.push(explanationBadge(explanation));
    }
    const head = h(
      'div',
      { class: 'card-head' },
      h('span', { class: 'number', text: String(row.number) }),
      h('span', { class: 'kind', text: kindText(row) }),
      h('span', { class: 'title', text: row.title }),
      h('span', { class: 'badges' }, badges),
    );
    card.replaceChildren(...[head, row.excerpt ? h('div', { class: 'excerpt', text: row.excerpt }) : null].filter(Boolean));
  }
}

/** The kind a card and the panel tag show: "Step 4" for a link drawn as an arrow, the number its pill shows. */
export function kindText(row) {
  return row.step !== null ? `Step ${row.step}` : row.kindLabel;
}

/** "dropped", "accepted" or "changed since accepted", as cards and the detail panel show them. */
export function stateBadges(row) {
  const badges = [];
  if (row.dropped) {
    badges.push(h('span', { class: `badge ${DROPPED}`, title: `Dropped for everyone. ${DROPPED_NOTE}`, text: DROPPED }));
  }
  if (row.isAccepted) {
    badges.push(h('span', { class: 'badge accepted', title: `Accepted. ${ACCEPT_NOTE}`, text: 'accepted' }));
  }
  if (row.staleAcceptance) {
    badges.push(h('span', { class: 'badge stale', title: 'Changed after it was accepted; accept it again to confirm', text: 'changed since accepted' }));
  }
  return badges;
}

/** The state of an item's latest explain request (EXPLANATION_STATES), as its card shows it. */
function explanationBadge({ status, reason }) {
  const state = EXPLANATION_STATES[status];
  return h('span', { class: `badge explanation state-${status}`, title: state.text(reason) ?? 'Explained: the side pane shows the explanation', text: state.badge });
}

/** `short`, the full `text` with the outcome's reason, and the `tone` of a shared question. */
export function questionState(question) {
  const outcome = question.outcome;
  const state = question.answer ? ANSWERED : (QUESTION_STATES[outcome.state] ?? { short: outcome.state, label: outcome.state, tone: 'warning' });
  const detail = question.answer ? null : (outcome.reason ?? outcome.turn_id ?? null);
  const text = [detail ? `${state.label}: ${detail}` : state.label, state.note].filter(Boolean).join('. ');
  return { short: state.short, text, tone: state.tone };
}
