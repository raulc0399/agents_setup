import { DiagramView, LEVELS, layoutKey } from './diagram.js';
import { MOD_KEY, h } from './dom.js';
import { agentLabel, formatTime, projectName, shortConversation } from './format.js';
import { ACCEPT_NOTE, BOOKMARK_NOTE, DROPPED_NOTE, ListView, bookmarkIcon } from './list.js';
import { DetailPanel } from './panel.js';
import { EXPLAIN_INTENT, EXPLAIN_TIMEOUT_MS, bookmarkCount, collapsed, documentsOf, explanationOf, hasDiagram, itemBookmarks, itemCount, itemIds, itemMark, nextExplainTimeout, quoteFound, rows as buildRows, sessionBookmarks } from './rows.js';

/** View state is saved this long after the last change, not on every key press. */
const VIEW_SAVE_DELAY_MS = 300;
/** After a tab was dropped outside every window, wait this long for another window to claim it. */
const DRAG_OUT_CHECK_DELAY_MS = 250;
const TAB_DRAG_PREFIX = 'prism-tab:';
/** A reply can arrive before `send_command` returned its ID; it waits this long to be matched. */
const EARLY_REPLY_KEEP_MS = 10000;
/**
 * Per tab, saved by the host. `modes`: list or diagram per document ID, chosen with V; none
 * shows the default for its content. `cameras`: the diagram's detail level, zoom and pan per
 * document ID; one without zoom and pan, or none, fits it. `filter`: text the items must contain; `selected`: item IDs selected for one action.
 * Split view: `split` is the right pane's own `{document, focus, selected, filter}` while the
 * tab shows two documents side by side (null: one); the top-level fields are the left pane's.
 * `ratio`: the left pane's share of the width, null for half; `activePane`: 'left' or 'right'.
 */
const DEFAULT_VIEW = { document: null, focus: null, hidden: [], filter: '', selected: [], modes: {}, cameras: {}, kept: {}, split: null, ratio: null, activePane: 'left' };
/**
 * A pane is named by its ID: the tab key for the left (or only) pane, the key with
 * RIGHT_PANE_SUFFIX for the right one. PANE_FIELDS are a pane's own; the tab shares the rest.
 */
const SIDES = ['left', 'right'];
const SIDE_LABELS = { left: 'Left', right: 'Right' };
const RIGHT_PANE_SUFFIX = '|right';
const PANE_FIELDS = ['document', 'focus', 'selected', 'filter'];
/**
 * Two panes are shown when each gets at least CONTENT_MIN_WIDTH beside the narrowest side
 * pane; otherwise (and in the narrow layout) only the active one. The divider between them is
 * DIVIDER_WIDTH wide (`--divider-width` in styles.css); its arrow keys move it by PANE_KEY_STEP.
 */
const DIVIDER_WIDTH = 1;
const DEFAULT_SPLIT_RATIO = 0.5;
/**
 * Ctrl+` or F6 makes the other pane active; Ctrl+` is matched by key position, as ` is a dead
 * key on some layouts.
 */
const PANE_SWITCH_CODE = 'Backquote';
const PANE_SWITCH_LABEL = 'Ctrl+`';
const PANE_SWITCH_KEY = 'F6';
/** What a document shows: its entry in `modes`, else the diagram when it has something to draw; V switches. */
const VIEWS = ['list', 'diagram'];
const VIEW_LABELS = { list: 'List', diagram: 'Diagram' };
/**
 * The side pane's width in CSS pixels, one per window, saved with its arrangement (none: the
 * default): at least PANE_MIN_WIDTH, so its controls stay usable, at most what leaves the
 * content, or each of two panes shown side by side, CONTENT_MIN_WIDTH, which keeps the
 * diagram's zoom controls on its canvas. The arrow keys on the pane's edge change it by
 * PANE_KEY_STEP.
 */
const PANE_DEFAULT_WIDTH = 320;
const PANE_MIN_WIDTH = 280;
const CONTENT_MIN_WIDTH = 400;
const PANE_KEY_STEP = 16;
/** The narrow layout of styles.css: the pane goes below the content, without a width of its own. */
const NARROW_LAYOUT = '(max-width: 760px)';

/** Only the Claude mod delivers explain requests; for other agents Explain is disabled with this. */
const EXPLAIN_AGENT = 'claude';
const EXPLAIN_NEEDS_MOD = 'Explain needs the Claude mod; ask in Codex instead';
const LINK_LABELS = { live: 'Live', disconnected: 'Disconnected', ended: 'Ended', waiting: 'Waiting for bridge' };
const READ_ONLY_REASONS = {
  disconnected: 'Read-only until the bridge reconnects',
  waiting: 'Read-only until the bridge connects',
  ended: 'Read-only: this conversation ended',
};
/** Keys, action and rank: when the status bar is short of room, the highest ranks go first. */
const KEY_HINTS = {
  list: [
    [['J', 'K'], 'move', 1],
    [['Space'], 'select', 2],
    [['Shift+A'], 'all', 5],
    [['Enter'], 'ask', 1],
    [['E'], 'explain', 2],
    [['B'], 'bookmark', 3],
    [['A'], 'accept', 2],
    [['X'], 'drop', 2],
    [['U'], 'undo', 2],
    [['Shift+B'], 'bookmarks', 5],
    [['H'], 'hide', 3],
    [['/'], 'filter', 3],
    [['Esc'], 'clear', 4],
    [['[', ']'], 'documents', 4],
    [['W'], 'close', 4],
    [['V'], 'diagram', 3],
  ],
  diagram: [
    [['J', 'K'], 'move', 1],
    [['Space'], 'select', 2],
    [['Enter'], 'ask', 1],
    [['E'], 'explain', 2],
    [['B'], 'bookmark', 3],
    [['A'], 'accept', 2],
    [['X'], 'drop', 2],
    [['U'], 'undo', 2],
    [['Shift+B'], 'bookmarks', 5],
    [['+', '-'], 'zoom', 3],
    [['0'], 'fit', 4],
    [[`${MOD_KEY}+0`], '100%', 5],
    [['1', '2', '3'], 'detail', 3],
    [['Esc'], 'clear', 4],
    [['H'], 'hide', 3],
    [['/'], 'filter', 3],
    [['S'], 'export', 4],
    [['W'], 'close', 4],
    [['V'], 'list', 3],
  ],
};
/** Keys of the split view, after those of the view: one pane, or two. */
const SPLIT_HINTS = {
  single: [[['\\'], 'split', 4]],
  split: [
    [[PANE_SWITCH_LABEL, PANE_SWITCH_KEY], 'other pane', 3],
    [['\\'], 'unsplit', 4],
  ],
};
/** Diagram keys that zoom, animated (`diagramKeys`), and keys that only modify others: neither stops a zoom on its way. */
const ZOOM_KEYS = new Set(['+', '=', '-', '0']);
const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'CapsLock']);
const SESSION_GROUPS = [
  ['Live', ['live']],
  ['Disconnected', ['disconnected', 'waiting']],
  ['Ended', ['ended']],
];

export function start(backend, root) {
  const app = new App(backend, root);
  app.start();
  return app;
}

function errorText(error) {
  switch (error.code) {
    case 'stale_revision':
      return `The content changed meanwhile (now revision ${error.current}) and was reloaded. Check it and try again.`;
    case 'not_found':
      return `Not found: ${error.message}`;
    case 'invalid':
      return `Invalid: ${error.message}`;
    case 'request_conflict':
      return 'This request was already sent with different content.';
    case 'undo_conflict':
      return 'Undo conflicts with later changes.';
    default:
      return error.message ?? 'The command failed.';
  }
}

class App {
  constructor(backend, root) {
    this.backend = backend;
    this.root = root;
    this.state = null;
    this.contents = new Map();
    this.loading = new Set();
    this.views = new Map();
    this.listViews = new Map();
    this.diagramViews = new Map();
    /**
     * Per pane, `{key, steps}`: the arrows' step numbers for what it lists (`layoutKey`), from
     * prism-core; null while asked.
     */
    this.sized = new Map();
    this.panels = new Map();
    this.shownPanel = null;
    this.focusIndex = new Map();
    this.saveTimers = new Map();
    this.pending = new Map();
    this.earlyReplies = new Map();
    /** Tabs with an ask on its way: a new one waits for its reply, so a question is not shared twice. */
    this.asking = new Set();
    /** Renders again when the next explain request on its way times out (`nextExplainTimeout`). */
    this.explainTimer = null;
    /**
     * Explain requests this window sent, by request ID, until their explanation arrived, they
     * failed or were lost, EXPLAIN_TIMEOUT_MS passed or their tab left: `{key, documentId,
     * itemId, number, sentAt}` of the first item asked about. In memory only.
     */
    this.explaining = new Map();
    /**
     * The document this window closed last, `{key, id, revision, side}` with the revision its
     * close produced and the pane it was shown in while split (else null), until the next
     * command: U reopens it there.
     */
    this.lastClosed = null;
    /**
     * The document of this window's last bookmark change, `{key, id, revision}` with the
     * revision it produced, until the next command: U undoes it there, also from another
     * document's pane (a bookmark removed in the list).
     */
    this.lastBookmarked = null;
    /** A jump to a bookmark in a document reopened for it, `{key, documentId, id}`, until its content arrived. */
    this.pendingJump = null;
    /** What the bookmarks list shows, so it is rebuilt only when that changed. */
    this.bookmarksSignature = null;
    /** Commands sent so far; a close only becomes the last one if nothing else went out after it. */
    this.commandCount = 0;
    this.message = null;
    this.sessionsOpen = false;
    this.menu = null;
    this.signatures = new Map();
    this.scrolls = new Map();
    /** The pane width the user chose for this window, null for the default; read once from the arrangement. */
    this.paneWidth = null;
    this.paneWidthRestored = false;
    /** While the pane's edge or the split divider is dragged: `{id, x, from, handle, apply}` of the press. */
    this.drag = null;
    /** Whether the body showed two panes at the last render, so a change limits the side pane again. */
    this.shownSplit = false;
    this.paneSaveTimer = null;
    this.narrowLayout = window.matchMedia(NARROW_LAYOUT);

    this.tabs = h('div', { class: 'tabs', role: 'tablist', 'aria-label': 'Conversations' });
    this.sessionsButton = h('button', {
      class: 'button quiet',
      'aria-haspopup': 'dialog',
      onclick: () => this.toggleSessions(),
    });
    this.tabstrip = h(
      'header',
      {
        class: 'tabstrip',
        ondragover: (event) => this.onDragOver(event),
        ondragleave: () => this.tabstrip.classList.remove('drop-target'),
        ondrop: (event) => this.onDrop(event),
      },
      this.tabs,
      h('div', { class: 'strip-actions' }, this.sessionsButton),
    );
    this.header = h('div', { class: 'tab-header' });
    // One input for the active pane's filter, kept across rebuilds of the side pane so typing goes on.
    this.filterInput = h('input', {
      class: 'filter',
      type: 'search',
      placeholder: 'Filter',
      spellcheck: 'false',
      'aria-label': 'Filter the items of this pane',
      title: 'Show only items whose kind, title, text or ID contains this; only this tab, in split view only its active pane (/)',
      oninput: () => this.updatePane(this.activePane(this.activeKey()), { filter: this.filterInput.value }),
      onkeydown: (event) => {
        if (event.key === 'Escape' || event.key === 'Enter') {
          event.preventDefault();
          this.leaveFilter();
        }
      },
    });
    // Updated in place, so a new revision does not rebuild the controls around the keyboard.
    this.info = h('span', { class: 'info' });
    // The body: the tab's list or diagram (or why there is none), or two side by side, and its side pane.
    this.content = h('div', { class: 'content' });
    this.content.style.setProperty('--divider-width', `${DIVIDER_WIDTH}px`);
    this.slots = Object.fromEntries(SIDES.map((side) => [side, this.paneSlot(side)]));
    // Between the two panes: drag it, or use the arrow keys, Home and End; a double-click shares the width equally.
    this.divider = h('div', {
      class: 'split-divider',
      role: 'separator',
      tabindex: '0',
      'aria-orientation': 'vertical',
      'aria-label': 'Width of the left document',
      title: 'Drag to share the width between the two documents; double-click shares it equally',
      onpointerdown: (event) => this.onDividerDown(event),
      onpointermove: (event) => this.onDragMove(event),
      onpointerup: (event) => this.endDrag(event),
      onpointercancel: (event) => this.endDrag(event),
      ondblclick: () => this.setSplitRatio(this.activeKey(), null),
      onkeydown: (event) => this.onDividerKey(event),
    });
    this.controls = h('div', { class: 'pane-controls' });
    // The pane's left edge: drag it, or use the arrow keys, Home and End; a double-click resets the width.
    this.separator = h('div', {
      class: 'pane-separator',
      role: 'separator',
      tabindex: '0',
      'aria-orientation': 'vertical',
      'aria-label': 'Side pane width',
      title: 'Drag to resize the side pane; double-click resets it',
      onpointerdown: (event) => this.onSeparatorDown(event),
      onpointermove: (event) => this.onDragMove(event),
      onpointerup: (event) => this.endDrag(event),
      onpointercancel: (event) => this.endDrag(event),
      ondblclick: () => this.setPaneWidth(null),
      onkeydown: (event) => this.onSeparatorKey(event),
    });
    this.pane = h('aside', { class: 'side-pane', 'aria-label': 'View and item details', hidden: true }, this.separator, this.controls);
    this.body = h('div', { class: 'body' }, this.content, this.pane);
    this.popover = h('div', { class: 'popover', role: 'dialog', 'aria-label': 'Sessions', hidden: true });
    this.main = h('main', { class: 'main' }, this.header, this.body, this.popover);
    this.statusMessage = h('div', { class: 'message', role: 'status' });
    this.keys = h('div', { class: 'keys' });
    this.statusbar = h('footer', { class: 'statusbar' }, this.statusMessage, this.keys);
    this.menuElement = h('div', { class: 'menu', role: 'menu', hidden: true });
    this.root.replaceChildren(this.tabstrip, this.main, this.statusbar, this.menuElement);
    this.keysSignature = null;
    new ResizeObserver(() => this.renderStatus()).observe(this.statusbar);
    // A window resized: the pane's width and the split are limited again.
    new ResizeObserver(() => {
      this.showPaneWidth();
      this.showSplit();
    }).observe(this.body);
  }

  /**
   * The place of one pane in the body, with its heading while split. A press or the keyboard
   * coming into it makes its pane the active one.
   */
  paneSlot(side) {
    const heading = h('div', { class: 'pane-heading', hidden: true });
    const body = h('div', { class: 'pane-body' });
    const element = h(
      'section',
      {
        class: 'pane-slot',
        // Focusable for a pane without items to show, so the keyboard stays in it.
        tabindex: '-1',
        onpointerdown: () => this.activateSide(side),
        onfocusin: () => this.activateSide(side),
      },
      heading,
      body,
    );
    return { side, name: `body-${side}`, element, heading, body, shown: null };
  }

  async start() {
    this.backend.onEvent((event) => this.onEvent(event));
    document.addEventListener('keydown', (event) => this.onKey(event));
    document.addEventListener('mousedown', (event) => this.dismiss(event));
    await this.refreshState();
  }

  // Data

  async refreshState() {
    this.state = await this.backend.state();
    this.restorePaneWidth();
    this.forgetViews();
    this.render();
    // The conversation may have ended: its requests on their way are lost.
    this.showArrivedExplanations();
  }

  /** At start, the pane width saved for this window; later this window's own is the latest. */
  restorePaneWidth() {
    const window = this.myWindow();
    if (this.paneWidthRestored || !window) {
      return;
    }
    this.paneWidthRestored = true;
    this.paneWidth = window.pane_width ?? null;
    this.showPaneWidth();
  }

  /** The lists, diagrams and detail panels of a tab that left this window are rebuilt if it comes back. */
  forgetViews() {
    const tabs = new Set(this.myWindow()?.tabs ?? []);
    for (const views of [this.listViews, this.panels, this.sized]) {
      for (const pane of [...views.keys()].filter((pane) => !tabs.has(paneTab(pane)))) {
        views.delete(pane);
      }
    }
    for (const [pane, view] of [...this.diagramViews].filter(([pane]) => !tabs.has(paneTab(pane)))) {
      view.dispose();
      this.diagramViews.delete(pane);
    }
    for (const [id, request] of [...this.explaining].filter(([, request]) => !tabs.has(request.key))) {
      this.explaining.delete(id);
    }
  }

  onEvent(event) {
    if (event.type === 'state') {
      this.refreshState();
    } else if (event.type === 'content') {
      if (event.key === this.activeKey()) {
        this.loadContent(event.key);
      } else {
        this.contents.delete(event.key);
      }
    } else if (event.type === 'reply') {
      this.onReply(event);
    }
  }

  /** Replaces the cached content once the new one arrived, so nothing flashes meanwhile. */
  async loadContent(key) {
    this.loading.add(key);
    try {
      const content = await this.backend.content(key);
      if (content) {
        // Without a pick of its own the tab shows the first document, which may close too.
        const shown = this.view(key).document ?? this.current(key)?.document.id ?? null;
        const beside = this.view(key).split?.document ?? null;
        this.contents.set(key, content);
        this.leaveClosedDocument(key, shown, beside);
        this.shownPanes(key).forEach((pane) => this.pruneSelection(pane));
      }
    } finally {
      this.loading.delete(key);
    }
    // A pane that went or came (a document closed or reopened) took the keyboard from the panes: the active one gets it.
    const keyboard = key === this.activeKey() && this.content.contains(document.activeElement);
    this.render();
    if (keyboard && !this.content.contains(document.activeElement)) {
      this.focusPane(this.activePane(key));
    }
    if (this.pendingJump?.key === key && key === this.activeKey()) {
      this.jumpTo(key, this.pendingJump.documentId, this.pendingJump.id);
    }
    this.showArrivedExplanations();
  }

  /**
   * A document shown is no longer open (closed here, by the agent or in another window). Split,
   * the pane whose document is still open stays alone. Otherwise the first open one takes over
   * as if picked, keeping the focus and selection of the one left.
   */
  leaveClosedDocument(key, shown, beside) {
    const open = documentsOf(this.contents.get(key), false);
    const isOpen = (id) => open.some((document) => document.id === id);
    if (beside && (!isOpen(beside) || beside === shown)) {
      this.storeUnsplit(key, 'left');
    } else if (beside && shown && !isOpen(shown)) {
      this.storeUnsplit(key, 'right');
      return;
    }
    const view = this.view(key);
    if (!shown || isOpen(shown)) {
      return;
    }
    const next = open[0]?.id ?? null;
    const kept = keepFor(view.kept, shown, view);
    const back = keptFor(kept, next);
    this.storeView(key, { document: next, focus: back.focus, selected: back.selected, kept });
  }

  /** Selected items that left with their document leave the selection; dropped ones stay in the list and stay selected. */
  pruneSelection(pane) {
    const current = this.current(pane);
    const items = current ? itemIds(current.document) : new Set();
    const selected = this.paneView(pane).selected;
    if (selected.some((id) => !items.has(id))) {
      this.storePane(pane, { selected: selected.filter((id) => items.has(id)) });
    }
  }

  myWindow() {
    return this.state?.arrangement.windows.find((window) => window.id === this.backend.windowId) ?? null;
  }

  activeKey() {
    return this.myWindow()?.active ?? null;
  }

  session(key) {
    return this.state.sessions.find((session) => session.key === key) ?? null;
  }

  view(key) {
    if (!this.views.has(key)) {
      // `mode` was one view for the whole tab, before `modes` held one per document.
      const { mode, ...saved } = this.state.arrangement.views[key] ?? {};
      this.views.set(key, { ...DEFAULT_VIEW, ...saved });
    }
    return this.views.get(key);
  }

  updateView(key, change) {
    this.storeView(key, change);
    this.render();
  }

  /** Records a view change and saves it shortly after, without rendering (zoom and pan). */
  storeView(key, change) {
    this.views.set(key, { ...this.view(key), ...change });
    clearTimeout(this.saveTimers.get(key));
    this.saveTimers.set(
      key,
      setTimeout(() => this.backend.setView(key, this.views.get(key)), VIEW_SAVE_DELAY_MS),
    );
  }

  // Panes

  /** Whether the tab shows two documents: its right pane's document is open and not the left one's. */
  isSplit(key) {
    const beside = this.view(key).split?.document;
    if (!beside) {
      return false;
    }
    // The left pane's document as `current` picks it, without building its rows.
    const open = documentsOf(this.contents.get(key), false);
    const left = open.find((document) => document.id === this.view(key).document) ?? open[0];
    return open.some((document) => document.id === beside) && left?.id !== beside;
  }

  /** The panes a tab shows: its left one, and the right one while split. */
  shownPanes(key) {
    return this.isSplit(key) ? SIDES.map((side) => paneId(key, side)) : [paneId(key, 'left')];
  }

  /** The pane the keys, the side pane and the document strip act on. */
  activePane(key) {
    return paneId(key, this.isSplit(key) ? this.view(key).activePane : 'left');
  }

  /** The other pane of a split tab. */
  otherPane(pane) {
    return paneId(paneTab(pane), paneSide(pane) === 'left' ? 'right' : 'left');
  }

  /** A pane's view: the tab's, with the right pane's own fields over it. */
  paneView(pane) {
    const view = this.view(paneTab(pane));
    return paneSide(pane) === 'right' ? { ...view, ...view.split } : view;
  }

  updatePane(pane, change) {
    this.storePane(pane, change);
    this.render();
  }

  /** Records a pane's view change: the right pane's own fields go to its `split`, the rest to the tab. */
  storePane(pane, change) {
    const key = paneTab(pane);
    if (paneSide(pane) === 'left') {
      this.storeView(key, change);
      return;
    }
    const split = this.view(key).split;
    const shared = Object.fromEntries(Object.entries(change).filter(([name]) => !PANE_FIELDS.includes(name)));
    const own = Object.fromEntries(Object.entries(change).filter(([name]) => PANE_FIELDS.includes(name)));
    // A right pane closed meanwhile (a reply arriving late) has nothing of its own left to change.
    this.storeView(key, split ? { ...shared, split: { ...split, ...own } } : shared);
  }

  /** The pane of a tab that shows a document, or null. */
  paneShowing(key, documentId) {
    return this.shownPanes(key).find((pane) => this.current(pane)?.document.id === documentId) ?? null;
  }

  /**
   * \ or the document menu: document `id` (null: the one after the left pane's) opens in a
   * right pane beside the tab's document, as the active pane, with the focus and selection
   * kept for it.
   */
  split(key, id) {
    const left = this.current(paneId(key, 'left'));
    if (!left) {
      return;
    }
    const index = left.documents.findIndex((document) => document.id === left.document.id);
    const beside = id ?? left.documents[(index + 1) % left.documents.length].id;
    if (beside === left.document.id) {
      this.setMessage('Only one document is open: there is nothing to show beside it.', 'error');
      return;
    }
    // U acts on the documents shown from now on.
    this.lastClosed = null;
    const back = keptFor(this.view(key).kept, beside);
    this.updateView(key, {
      document: left.document.id,
      split: { document: beside, focus: back.focus, selected: back.selected, filter: '' },
      activePane: 'right',
    });
    // Rebuilding the body took the keyboard from the pane that had it.
    this.focusPane(this.activePane(key));
  }

  /** Back to one pane showing the document of `keep`, with the keyboard; the focus and selection of the other are kept for its document. */
  unsplit(key, keep) {
    this.storeUnsplit(key, keep);
    this.render();
    this.focusPane(this.activePane(key));
  }

  storeUnsplit(key, keep) {
    const view = this.view(key);
    const right = view.split;
    const leaving = keep === 'right' ? view : right;
    const kept = keepFor(view.kept, leaving?.document, leaving);
    const stays = keep === 'right' && right ? Object.fromEntries(PANE_FIELDS.map((name) => [name, right[name]])) : {};
    this.storeView(key, { ...stays, split: null, activePane: 'left', kept });
  }

  /** \: two documents, or back to the left one. */
  toggleSplit(key) {
    if (this.isSplit(key)) {
      this.unsplit(key, 'left');
    } else {
      this.split(key, null);
    }
  }

  /** A press or the keyboard in a pane of the active tab makes it the active pane. */
  activateSide(side) {
    const key = this.activeKey();
    if (key && this.isSplit(key) && this.view(key).activePane !== side) {
      this.updateView(key, { activePane: side });
    }
  }

  /** Ctrl+` or F6: the other pane becomes active and takes the keyboard. */
  switchPane(key) {
    if (!this.isSplit(key)) {
      this.setMessage('One document is shown: \\ shows a second one beside it.');
      return;
    }
    const pane = this.otherPane(this.activePane(key));
    this.updateView(key, { activePane: paneSide(pane) });
    this.focusPane(pane);
  }

  // Views of a pane

  listView(pane) {
    if (!this.listViews.has(pane)) {
      this.listViews.set(
        pane,
        new ListView({
          onFocus: (id) => this.focus(pane, id),
          onToggle: (id) => this.toggleSelection(pane, id),
        }),
      );
    }
    return this.listViews.get(pane);
  }

  /** The detail area of the side pane for a pane, in both views. */
  detailPanel(pane) {
    if (!this.panels.has(pane)) {
      this.panels.set(pane, new DetailPanel(this.itemActions(pane)));
    }
    return this.panels.get(pane);
  }

  /** What the detail panel does with the focused or selected items. */
  itemActions(pane) {
    return {
      onAsk: () => this.ask(pane),
      onExplain: () => this.explain(pane),
      onBookmark: () => this.bookmark(pane),
      onAccept: () => this.accept(pane),
      onDrop: () => this.drop(pane),
      onRemoveBookmark: (id) => {
        const document = this.current(pane)?.document;
        if (document?.bookmarks?.[id]) {
          this.removeBookmark(paneTab(pane), document, document.bookmarks[id]);
        }
      },
    };
  }

  diagramView(pane) {
    if (!this.diagramViews.has(pane)) {
      this.diagramViews.set(
        pane,
        new DiagramView(
          {
            layout: (document, visible, detail) => this.diagramLayout(pane, document, visible, detail),
            onFocus: (id) => this.focus(pane, id),
            onToggle: (id) => this.toggleSelection(pane, id),
            onClear: () => this.focus(pane, null),
            // The overview in the detail panel counts the shapes of the new layout.
            onLayout: () => this.render(),
            onCamera: (documentId, camera) => {
              const cameras = { ...this.paneView(pane).cameras };
              if (camera) {
                cameras[documentId] = camera;
              } else {
                delete cameras[documentId];
              }
              this.storePane(pane, { cameras });
            },
          },
          this.detailPanel(pane).element,
        ),
      );
    }
    return this.diagramViews.get(pane);
  }

  /** Lays out a pane's diagram in prism-core; every response also brings the arrows' step numbers and the sizes. */
  async diagramLayout(pane, document, visible, detail) {
    const diagram = await this.backend.diagram(document, visible, detail);
    this.storeSized(pane, layoutKey(document, visible), diagram);
    return diagram;
  }

  /**
   * The list shows the arrows' step numbers without drawing the diagram: it asks for the
   * sizes alone, once per thing listed.
   */
  requestSized(pane, current) {
    const visible = current.rows.map((row) => row.id);
    const wanted = layoutKey(current.document, visible);
    if (Object.keys(current.document.relationships).length === 0 || this.sized.get(pane)?.key === wanted) {
      return;
    }
    this.sized.set(pane, { key: wanted, steps: null });
    // Without them the list shows no steps; the next change of what it lists asks again.
    this.diagramLayout(pane, current.document, visible, null).catch(() => {});
  }

  /** Keeps and shows steps that arrived for what the pane lists now; those of what it listed before are dropped. */
  storeSized(pane, wanted, { steps }) {
    const current = this.current(pane);
    if (!current || layoutKey(current.document, current.rows.map((row) => row.id)) !== wanted) {
      return;
    }
    const entry = this.sized.get(pane);
    if (entry?.key === wanted && entry.steps !== null) {
      return;
    }
    this.sized.set(pane, { key: wanted, steps });
    this.render();
  }

  /** What prism-core gave for what a pane lists now, or null. */
  sizedFor(pane, document, rows) {
    const entry = this.sized.get(pane);
    return entry?.key === layoutKey(document, rows.map((row) => row.id)) && entry.steps !== null ? entry : null;
  }

  /** The document, its visible rows, the focused and the selected rows of a pane; null before content. */
  current(pane) {
    const content = this.contents.get(paneTab(pane));
    const documents = documentsOf(content, false);
    if (documents.length === 0) {
      return null;
    }
    const view = this.paneView(pane);
    const document = documents.find((candidate) => candidate.id === view.document) ?? documents[0];
    const listed = buildRows(document, new Set(view.hidden), view.filter);
    // A link drawn as an arrow also has the step number its pill shows, once prism-core gave it.
    const steps = this.sizedFor(pane, document, listed)?.steps ?? {};
    const rows = listed.map((row) => ({ ...row, step: steps[row.id] ?? null }));
    let focusIndex = rows.findIndex((row) => row.id === view.focus);
    if (focusIndex === -1 && rows.length > 0) {
      // The focused item left (hidden, filtered out): its neighbour takes over.
      focusIndex = Math.min(this.focusIndex.get(pane) ?? 0, rows.length - 1);
    }
    this.focusIndex.set(pane, focusIndex);
    // Only what is shown counts: a selected item hidden or filtered out takes no part.
    const selected = new Set(view.selected);
    return { content, documents, document, rows, focus: rows[focusIndex] ?? null, selected: rows.filter((row) => selected.has(row.id)) };
  }

  /** What the pane's document shows: list, or diagram when it has something to draw. */
  mode(pane) {
    const document = this.current(pane)?.document;
    if (!document || !hasDiagram(document)) {
      return 'list';
    }
    return this.paneView(pane).modes[document.id] ?? 'diagram';
  }

  /** V: the other view of the pane's document; a document with nothing to draw stays a list. */
  switchMode(pane) {
    const current = this.current(pane);
    if (!current) {
      return;
    }
    if (!hasDiagram(current.document)) {
      this.setMessage('Nothing to draw: this document has no arrows or groups.');
      return;
    }
    this.setMode(pane, VIEWS[(VIEWS.indexOf(this.mode(pane)) + 1) % VIEWS.length]);
  }

  setMode(pane, mode) {
    const document = this.current(pane).document;
    this.updatePane(pane, { modes: { ...this.paneView(pane).modes, [document.id]: mode } });
  }

  // Rendering

  render() {
    if (!this.state) {
      return;
    }
    this.renderTabs();
    this.renderMain();
    this.renderStatus();
    this.renderSessions();
    this.renderMenu();
    this.scheduleExplainTimeout();
  }

  /** An explain request without an answer says so once it timed out: the next one due renders again. */
  scheduleExplainTimeout() {
    clearTimeout(this.explainTimer);
    const now = Date.now();
    const due = [...this.contents.values()].map((content) => nextExplainTimeout(content?.questions ?? [], now)).filter((time) => time !== null);
    this.explainTimer = due.length > 0 ? setTimeout(() => this.render(), Math.min(...due) - now + 1) : null;
  }

  /** Why Explain is unavailable in a tab: only the Claude mod delivers explain requests. */
  explainBlocked(key) {
    const agent = this.session(key)?.agent;
    return agent && agent !== EXPLAIN_AGENT ? EXPLAIN_NEEDS_MOD : null;
  }

  /** Rebuilds `container` only when `signature` changed, keeping keyboard focus otherwise. */
  patch(name, container, signature, build) {
    const text = JSON.stringify(signature);
    if (this.signatures.get(name) === text) {
      return;
    }
    this.signatures.set(name, text);
    container.replaceChildren(...build().filter(Boolean));
  }

  renderTabs() {
    const window = this.myWindow();
    const tabs = window?.tabs ?? [];
    const sessions = tabs.map((key) => this.session(key));
    const names = sessions.map((session) => (session ? projectName(session.project) : null));
    const shared = new Set(names.filter((name, index) => name && names.indexOf(name) !== index));
    // Rebuilding the strip must not take the keyboard away from a focused tab.
    const focused = this.tabs.contains(document.activeElement) ? document.activeElement.closest('.tab')?.dataset.key : null;
    this.patch('tabs', this.tabs, [tabs, window?.active, sessions], () =>
      tabs.map((key, index) => this.tabElement(key, sessions[index], key === window.active, shared.has(names[index]))),
    );
    if (focused && !this.tabs.contains(document.activeElement)) {
      [...this.tabs.children].find((tab) => tab.dataset.key === focused)?.focus();
    }
    const live = this.state.sessions.filter((session) => session.link.state === 'live').length;
    const problem = this.state.status.problem;
    this.patch('sessions-button', this.sessionsButton, [live, this.state.sessions.length, problem], () => [
      // A bridge that could not connect opens no tab; the mark points to the panel that says why.
      problem ? h('span', { class: 'dot disconnected', title: problem, role: 'img', 'aria-label': `Problem: ${problem}` }) : null,
      'Sessions',
      h('span', { class: 'count', text: String(live) }),
    ]);
    this.sessionsButton.setAttribute('aria-expanded', String(this.sessionsOpen));
  }

  /** `ambiguous`: another tab here shows the same project, so the conversation is named too. */
  tabElement(key, session, active, ambiguous) {
    const state = session?.link.state ?? 'waiting';
    const name = session ? projectName(session.project) : key;
    const agent = session ? agentLabel(session.agent) : '';
    const detail = ambiguous ? `${agent} · ${shortConversation(session.conversation)}` : agent;
    const tab = h(
      'div',
      {
        class: 'tab',
        role: 'tab',
        'data-key': key,
        tabindex: active ? '0' : '-1',
        'aria-selected': String(active),
        draggable: true,
        title: session
          ? `${session.project} · ${agent} · ${session.host} · ${LINK_LABELS[state]}\nconversation ${session.conversation}`
          : key,
        onclick: () => this.activate(key),
        oncontextmenu: (event) => {
          event.preventDefault();
          this.openMenu(key, event.clientX, event.clientY);
        },
        onkeydown: (event) => this.onTabKey(event, key, tab),
        ondragstart: (event) => {
          event.dataTransfer.setData('text/plain', TAB_DRAG_PREFIX + key);
          event.dataTransfer.effectAllowed = 'move';
        },
        ondragend: (event) => this.onDragEnd(event, key),
      },
      h('span', { class: `dot ${state}` }),
      h('span', { class: 'label' }, name, ' ', h('span', { class: 'agent', text: detail })),
      h('button', {
        class: 'close',
        title: 'Close tab',
        'aria-label': `Close ${name}`,
        text: '×',
        onclick: (event) => {
          event.stopPropagation();
          this.backend.closeTab(key);
        },
      }),
    );
    return tab;
  }

  renderMain() {
    const key = this.activeKey();
    if (!key) {
      this.patch('header', this.header, null, () => []);
      this.header.hidden = true;
      this.pane.hidden = true;
      this.hideSlots();
      this.patch('body', this.content, ['empty', this.state.status, this.state.sessions, this.myWindow()?.id], () => [
        this.emptyWindow(),
      ]);
      return;
    }
    this.header.hidden = false;

    const session = this.session(key);
    const state = session?.link.state ?? 'waiting';
    if (session?.loaded && !this.contents.has(key) && !this.loading.has(key)) {
      this.loadContent(key);
    }
    const pane = this.activePane(key);
    const current = this.current(pane);
    const view = this.paneView(pane);
    const mode = this.mode(pane);
    const closed = documentsOf(this.contents.get(key), true);
    const bookmarks = bookmarkCount(this.contents.get(key));
    const panes = this.shownPanes(key);
    // While split, the document strip marks the document of each side.
    const sides = panes.length > 1 ? Object.fromEntries(panes.map((shown) => [this.current(shown).document.id, paneSide(shown)])) : {};
    this.patch(
      'header',
      this.header,
      [key, session, current?.documents.map((document) => [document.id, document.title]), current?.document.id, sides, closed.length, bookmarks],
      () => this.tabHeader(key, session, state, current, closed, sides, bookmarks),
    );

    this.pane.hidden = !current;
    if (!current) {
      this.hideSlots();
      this.patch('body', this.content, ['no-content', key, state, session?.loaded, session?.restored, this.contents.has(key), closed.length], () => [
        this.emptyTab(key, session, state, closed),
      ]);
      return;
    }
    this.renderControls(pane, current, view, mode);
    this.showSlots(key, panes.length > 1);
    for (const shown of panes) {
      this.renderPane(shown, state, shown === pane, panes.length > 1);
    }
  }

  /**
   * One pane in its slot: its list or diagram, or why there is none. The active pane's detail
   * panel goes to the side pane.
   */
  renderPane(pane, state, active, split) {
    const slot = this.slots[paneSide(pane)];
    const current = this.current(pane);
    const view = this.paneView(pane);
    const mode = this.mode(pane);
    const filter = view.filter.trim();
    slot.element.classList.toggle('active', active);
    slot.element.setAttribute('aria-label', `${split ? `${SIDE_LABELS[slot.side]} document` : 'Document'}: ${current.document.title}`);
    if (split && active) {
      slot.element.setAttribute('aria-current', 'true');
    } else {
      slot.element.removeAttribute('aria-current');
    }
    slot.heading.hidden = !split;
    if (split) {
      this.patch(`heading-${slot.side}`, slot.heading, [pane, current.document.title, filter], () => this.paneHeading(pane, current.document, filter));
    }
    if (current.rows.length === 0) {
      if (active) {
        this.showPanel(null);
      }
      this.showBody(slot, null);
      const hidden = view.hidden.length;
      const total = itemCount(current.document);
      this.patch(slot.name, slot.body, ['no-rows', pane, current.document.id, total, hidden, filter], () => [
        total === 0
          ? empty('This document has no items', null)
          : filter
            ? empty(
                `No items match “${filter}”`,
                hidden > 0 ? `Items hidden in this tab are not searched; Shift+H shows them.` : null,
                h('button', { class: 'button', text: 'Clear filter', onclick: () => this.updatePane(pane, { filter: '' }) }),
              )
            : empty(
                `All ${total} items are hidden in this tab`,
                null,
                h('button', { class: 'button', text: 'Show hidden', onclick: () => this.updateView(paneTab(pane), { hidden: [] }) }),
              ),
      ]);
      return;
    }
    const selected = new Set(current.selected.map((row) => row.id));
    const panel = this.detailPanel(pane);
    if (active) {
      this.showPanel(panel);
    }
    const details = {
      document: current.document,
      rows: current.rows,
      selected,
      questions: current.content.questions,
      readOnly: READ_ONLY_REASONS[state] ?? null,
      explainBlocked: this.explainBlocked(paneTab(pane)),
      ended: state === 'ended',
      now: Date.now(),
    };

    if (mode === 'diagram') {
      const diagramView = this.diagramView(pane);
      this.showBody(slot, diagramView, pane);
      // Only an item the user picked: without one the panel shows the overview.
      diagramView.render({ ...details, focus: view.focus, camera: view.cameras?.[current.document.id] ?? null });
      if (active) {
        panel.render({ ...details, focus: view.focus, counts: diagramView.counts() });
      }
      return;
    }
    this.requestSized(pane, current);
    const listView = this.listView(pane);
    this.showBody(slot, listView, pane);
    const focus = current.focus?.id ?? null;
    listView.render({ ...details, focus });
    if (active) {
      panel.render({ ...details, focus, counts: null });
    }
  }

  /** Above each pane while split: its side and document, whether its `filter` hides items, and its close control. */
  paneHeading(pane, document, filter) {
    const side = paneSide(pane);
    const other = paneSide(this.otherPane(pane));
    return [
      h('span', { class: 'pane-side', text: SIDE_LABELS[side] }),
      h('span', { class: 'pane-title', title: document.title, text: document.title }),
      filter ? h('span', { class: 'chip filtered', title: `Only items containing “${filter}” are shown (/)`, text: 'filtered' }) : null,
      h('button', {
        class: 'close',
        title: `Close this pane; the ${other} document stays`,
        'aria-label': `Close the ${side} pane`,
        text: '×',
        onclick: () => this.unsplit(paneTab(pane), other),
      }),
    ];
  }

  /**
   * The controls block at the top of the side pane, for the active pane: the view switch when
   * the document has something to draw, the filter, the counts with their actions, and the export.
   */
  renderControls(pane, current, view, mode) {
    const filtering = document.activeElement === this.filterInput;
    const caret = filtering ? [this.filterInput.selectionStart, this.filterInput.selectionEnd] : null;
    // A rebuild must not take the keyboard away from a focused button: its new copy takes it.
    const control = this.controls.contains(document.activeElement) ? document.activeElement.dataset.control : null;
    this.patch(
      'controls',
      this.controls,
      [pane, current.document.id, view.hidden.length, current.selected.length, view.selected.length, mode, hasDiagram(current.document)],
      () => this.paneControls(pane, current, view, mode),
    );
    if (filtering && document.activeElement !== this.filterInput) {
      this.filterInput.focus();
      this.filterInput.setSelectionRange(...caret);
    }
    if (control && !this.controls.contains(document.activeElement)) {
      this.controls.querySelector(`[data-control="${control}"]`)?.focus();
    }
    this.info.textContent = `${current.rows.length} ${current.rows.length === 1 ? 'item' : 'items'} · revision ${current.document.revision}`;
    // While typing the two agree; they differ after a tab or pane switch or Clear filter.
    if (this.filterInput.value !== view.filter) {
      this.filterInput.value = view.filter;
    }
    this.filterInput.classList.toggle('active', view.filter.trim() !== '');
  }

  /** Below the controls, the side pane shows the active pane's detail panel while there are items. */
  showPanel(panel) {
    if (this.shownPanel === panel) {
      return;
    }
    // The controls stay in place, so the filter keeps the keyboard.
    this.shownPanel?.element.remove();
    this.shownPanel = panel;
    if (panel) {
      this.pane.append(panel.element);
    }
  }

  /**
   * Shows a pane's list or diagram in its slot (null: none for now), keeping each pane's list
   * scroll position across switches.
   */
  showBody(slot, view, pane) {
    if (slot.shown?.view === view) {
      return;
    }
    if (slot.shown?.view instanceof ListView) {
      this.scrolls.set(slot.shown.pane, slot.shown.view.element.scrollTop);
    }
    this.signatures.delete(slot.name);
    slot.shown = view ? { view, pane } : null;
    if (view) {
      slot.body.replaceChildren(view.element);
    }
    if (view instanceof ListView) {
      view.element.scrollTop = this.scrolls.get(pane) ?? 0;
    }
  }

  /**
   * The body shows the slots of tab `key`: the left one, and the divider and the right one while
   * split. A right diagram not shown reads the saved camera and level again when it comes back,
   * as the left pane may have changed them for its document meanwhile.
   */
  showSlots(key, split) {
    const slots = split ? [this.slots.left.element, this.divider, this.slots.right.element] : [this.slots.left.element];
    if (!split) {
      this.showBody(this.slots.right, null);
      this.diagramViews.get(paneId(key, 'right'))?.forget();
    }
    if (this.content.children.length !== slots.length || slots.some((slot, index) => this.content.children[index] !== slot)) {
      this.signatures.delete('body');
      this.content.replaceChildren(...slots);
    }
    this.content.classList.toggle('split', split);
    if (split !== this.shownSplit) {
      // Two panes need more room beside the side pane than one.
      this.shownSplit = split;
      this.showPaneWidth();
    }
    this.showSplit();
  }

  /** The whole tab or window shows one message instead of the panes: they let go of their lists and diagrams. */
  hideSlots() {
    for (const slot of Object.values(this.slots)) {
      this.showBody(slot, null);
    }
    // The message is rebuilt on every render: it also shows state its signature leaves out (when a tab was seen or ended).
    this.signatures.delete('body');
    this.content.classList.remove('split', 'compact');
    this.shownSplit = false;
  }

  tabHeader(key, session, state, current, closed, sides, bookmarks) {
    const agent = session ? agentLabel(session.agent) : '';
    const parts = [
      h(
        'div',
        { class: 'session-line' },
        h('h1', { text: session ? projectName(session.project) : key }),
        h('span', {
          class: 'meta',
          text: session ? `${agent} · ${session.host} · ${session.project}` : '',
          title: session ? `conversation ${session.conversation}` : '',
        }),
        h('span', {
          class: 'meta conversation',
          text: session ? shortConversation(session.conversation) : '',
        }),
        h('span', { class: 'chip state' }, h('span', { class: `dot ${state}` }), LINK_LABELS[state]),
      ),
    ];
    const banner = this.banner(session, state);
    if (banner) {
      parts.push(banner);
    }
    if (current || closed.length > 0) {
      parts.push(
        h(
          'div',
          { class: 'toolbar' },
          current
            ? h(
                'div',
                { class: 'documents', role: 'tablist', 'aria-label': 'Documents' },
                current.documents.map((document) => this.documentTab(key, document, document.id === current.document.id, sides[document.id] ?? null)),
              )
            : null,
          closed.length > 0
            ? h('button', {
                class: 'button quiet closed-documents',
                'aria-haspopup': 'menu',
                title: 'Documents closed for everyone; pick one to reopen it',
                text: `Closed (${closed.length})`,
                onclick: (event) => this.openClosedMenu(key, event.currentTarget),
              })
            : null,
          bookmarks > 0
            ? h(
                'button',
                {
                  class: 'button quiet bookmarks-control',
                  'aria-haspopup': 'dialog',
                  'aria-expanded': String(Boolean(this.menu?.bookmarks)),
                  title: 'The bookmarks of this conversation, in all its documents; pick one to go to it (Shift+B)',
                  onclick: () => this.openBookmarks(key),
                },
                bookmarkIcon(),
                `Bookmarks (${bookmarks})`,
              )
            : null,
        ),
      );
    }
    return parts;
  }

  /**
   * A document in the strip, with its close control (W closes the one shown). `selected`: shown
   * in the active pane; `side`: the pane showing it while split, marked L or R, or null. Its menu
   * shows it beside the other, or on a side.
   */
  documentTab(key, document, selected, side) {
    const openMenu = (x, y) => this.openMenu(key, x, y, document.id);
    const where = side ? `, shown ${side === 'left' ? 'on the left' : 'on the right'}` : '';
    return h(
      'div',
      {
        class: ['document', selected ? 'selected' : '', side ? 'shown' : ''].filter(Boolean).join(' '),
        role: 'presentation',
        oncontextmenu: (event) => {
          event.preventDefault();
          openMenu(event.clientX, event.clientY);
        },
      },
      h(
        'button',
        {
          role: 'tab',
          'aria-selected': String(selected),
          'aria-label': `${document.title}${where}`,
          title: `${document.title}${where}`,
          onclick: () => this.showDocument(this.activePane(key), document.id),
          onkeydown: (event) => {
            if (event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey)) {
              event.preventDefault();
              const rect = event.currentTarget.getBoundingClientRect();
              openMenu(rect.left, rect.bottom);
            }
          },
        },
        side ? h('span', { class: 'side-mark', 'aria-hidden': 'true', text: SIDE_LABELS[side][0] }) : null,
        document.title,
      ),
      h('button', {
        class: 'close',
        title: 'Close this document for everyone (W)',
        'aria-label': `Close ${document.title}`,
        text: '×',
        onclick: () => this.closeDocument(key, document),
      }),
    );
  }

  /** The view switch and export, the filter, then the counts with their actions. */
  paneControls(pane, current, view, mode) {
    const hidden = view.hidden.length;
    // Selected items hidden or filtered out stay selected but take no part in actions.
    const filteredOut = view.selected.length - current.selected.length;
    const switchable = hasDiagram(current.document);
    const viewRow = switchable
      ? h(
          'div',
          { class: 'pane-row' },
          h(
            'div',
            { class: 'segmented', role: 'group', 'aria-label': 'View' },
            VIEWS.map((name) =>
              h('button', {
                'aria-pressed': String(name === mode),
                'data-control': `view-${name}`,
                title: `Show as ${VIEW_LABELS[name].toLowerCase()} (V)`,
                text: VIEW_LABELS[name],
                onclick: () => this.setMode(pane, name),
              }),
            ),
          ),
          h('span', { class: 'spacer' }),
          mode === 'diagram'
            ? h('button', {
                class: 'button quiet',
                'data-control': 'export',
                title: 'Save the diagram as an SVG file (S)',
                text: 'Export SVG',
                onclick: () => this.exportDiagram(pane),
              })
            : null,
        )
      : null;
    return [
      viewRow,
      h('div', { class: 'pane-row' }, this.filterInput),
      h(
        'div',
        { class: 'pane-row counts' },
        this.info,
        hidden > 0
          ? h('button', {
              class: 'button quiet',
              'data-control': 'show-hidden',
              title: 'Show the items hidden in this tab (Shift+H)',
              text: `${hidden} hidden · show`,
              onclick: () => this.updateView(paneTab(pane), { hidden: [] }),
            })
          : null,
        view.selected.length > 0
          ? h('button', {
              class: 'button quiet selection',
              'data-control': 'clear-selection',
              title: filteredOut > 0 ? 'Actions leave out selected items that are hidden or filtered out. Clear the selection (Esc)' : 'Clear the selection (Esc)',
              text: `${current.selected.length} selected${filteredOut > 0 ? ` (${filteredOut} not shown)` : ''} · clear`,
              onclick: () => this.clearSelection(pane),
            })
          : null,
      ),
    ];
  }

  banner(session, state) {
    if (state === 'disconnected') {
      return h(
        'div',
        { class: 'banner' },
        h('span', { class: 'dot disconnected' }),
        session?.loaded
          ? 'Connection lost. The content shown is the last received; the bridge reconnects on its own.'
          : 'Connection lost before the content arrived; the bridge reconnects on its own.',
      );
    }
    // A tab restored ended has no content; its body says so.
    if (state === 'ended' && !session.restored) {
      return h(
        'div',
        { class: 'banner ended' },
        h('span', { class: 'dot ended' }),
        `This conversation ended (${session.link.reason}). The content stays readable until you close the tab.`,
      );
    }
    return null;
  }

  emptyTab(key, session, state, closed) {
    if (state === 'waiting') {
      // 0 or none: not known when, for example from an arrangement saved before it was kept.
      const seen = this.state.arrangement.seen?.[key];
      return empty(
        'Waiting for this conversation to reconnect',
        `${seen ? `Not seen since ${formatTime(seen)}. ` : ''}It shows its content again when the conversation’s bridge connects; if the conversation is over, close the tab.`,
        this.closeTabButton(key),
      );
    }
    if (state === 'ended' && session.restored) {
      const endedAt = this.state.arrangement.ended?.[key]?.ended_at_ms;
      return empty(
        'This conversation ended before Prism restarted',
        `It ended${endedAt ? ` on ${formatTime(endedAt)}` : ''} (${session.link.reason}). Its content is no longer available.`,
        this.closeTabButton(key),
      );
    }
    if (!session.loaded) {
      return empty(state === 'live' ? 'Loading content…' : 'No content received', null);
    }
    // Loaded in the host but not fetched into this window yet.
    if (!this.contents.has(key)) {
      return empty('Loading content…', null);
    }
    if (closed.length > 0) {
      return empty('All documents are closed', `They are closed for everyone. Closed (${closed.length}) above reopens one.`);
    }
    return empty('Nothing published yet', 'Content appears here when the agent publishes to Prism.');
  }

  closeTabButton(key) {
    return h('button', { class: 'button', text: 'Close tab', onclick: () => this.backend.closeTab(key) });
  }

  emptyWindow() {
    const status = this.state.status;
    if (!status.listening) {
      return empty('Prism cannot accept conversations', status.listen_error, this.statusFacts());
    }
    if (this.state.sessions.length === 0) {
      return empty(
        'No conversations connected yet',
        'Claude and Codex conversations with the Prism plugin connect on their own and open here as tabs.',
        this.statusFacts(),
      );
    }
    const problem = status.problem ? h('div', { class: 'facts' }, fact('bad', status.problem)) : null;
    return empty('No tabs in this window', 'Open a conversation here:', problem, this.sessionPicker());
  }

  statusFacts() {
    const status = this.state.status;
    const facts = [];
    if (status.listening) {
      facts.push(fact('live', 'Listening on ', h('code', { text: status.address })));
    }
    for (const pairing of status.pairing) {
      facts.push(
        pairing.error
          ? fact('bad', 'Pairing file failed: ', h('code', { text: `${pairing.path}: ${pairing.error}` }))
          : fact('live', 'Pairing file ', h('code', { text: pairing.path })),
      );
    }
    if (status.listening && status.pairing.length === 0) {
      facts.push(fact('bad', 'No pairing file written: neither ~/.claude nor ~/.codex exists'));
    }
    for (const warning of status.warnings) {
      facts.push(fact('bad', warning));
    }
    if (status.problem) {
      facts.push(fact('bad', status.problem));
    }
    facts.push(fact('waiting', 'Optional settings ', h('code', { text: status.settings_file })));
    facts.push(h('div', { class: 'about', text: `Prism ${status.version} · app protocol ${status.protocol}` }));
    return h('div', { class: 'facts' }, facts);
  }

  sessionPicker() {
    const groups = SESSION_GROUPS.map(([title, states]) => [
      title,
      this.state.sessions.filter((session) => states.includes(session.link.state)),
    ]).filter(([, sessions]) => sessions.length > 0);
    return h(
      'div',
      { class: 'session-picker' },
      groups.map(([title, sessions]) => [h('h3', { text: title }), sessions.map((session) => this.sessionRow(session))]),
    );
  }

  sessionRow(session) {
    const windows = this.state.arrangement.windows;
    const owner = windows.findIndex((window) => window.tabs.includes(session.key));
    const here = owner !== -1 && windows[owner].id === this.backend.windowId;
    const where = owner === -1 ? 'Open' : here ? 'this window' : `Show window ${owner + 1}`;
    return h(
      'button',
      {
        class: 'session-row',
        title: `${session.project}\nconversation ${session.conversation}`,
        onclick: () => {
          this.sessionsOpen = false;
          if (owner === -1) {
            this.backend.reopenTab(session.key);
          } else if (here) {
            this.activate(session.key);
          } else {
            // Shown in another window: bring that window forward instead of taking the tab.
            this.backend.showWindow(windows[owner].id);
          }
          this.render();
        },
      },
      h('span', { class: `dot ${session.link.state}` }),
      h(
        'span',
        {},
        h('div', { class: 'name', text: projectName(session.project) }),
        h('div', {
          class: 'sub',
          text: `${agentLabel(session.agent)} · ${session.host} · ${shortConversation(session.conversation)}`,
        }),
      ),
      h('span', { class: `where ${here ? '' : 'open'}`, text: where }),
    );
  }

  renderSessions() {
    this.popover.hidden = !this.sessionsOpen;
    if (!this.sessionsOpen) {
      this.signatures.delete('popover');
      return;
    }
    this.patch('popover', this.popover, [this.state], () => [
      this.state.sessions.length > 0 ? this.sessionPicker() : h('h3', { text: 'No sessions yet' }),
      h('div', { class: 'status' }, [...this.statusFacts().children]),
    ]);
  }

  renderStatus() {
    const key = this.activeKey();
    const pane = key ? this.activePane(key) : null;
    const mode = pane ? this.mode(pane) : 'list';
    const document = pane ? this.current(pane)?.document : null;
    // Without content the keys have nothing to act on.
    this.keys.hidden = !document;
    this.statusMessage.textContent = this.message?.text ?? '';
    this.statusMessage.title = this.message?.text ?? '';
    this.statusMessage.className = `message ${this.message?.tone ?? ''}`;
    this.fitKeys(mode, Boolean(document && hasDiagram(document)), Boolean(document && this.isSplit(key)));
  }

  /**
   * Shows the key hints of the active pane that fit beside the message, leaving out the
   * highest ranks first; V only when the document has a diagram to switch to.
   */
  fitKeys(mode, switchable, split) {
    const signature = JSON.stringify([mode, switchable, split, this.keys.hidden, this.message?.text ?? '', this.statusbar.clientWidth]);
    if (this.keys.hidden || signature === this.keysSignature) {
      return;
    }
    this.keysSignature = signature;
    let hints = [...(KEY_HINTS[mode] ?? KEY_HINTS.list), ...SPLIT_HINTS[split ? 'split' : 'single']].filter(([keys]) => switchable || keys[0] !== 'V');
    this.keys.replaceChildren(...keyHints(hints));
    const ranks = [...new Set(hints.map(([, , rank]) => rank))].sort((left, right) => right - left);
    for (const rank of ranks) {
      if (this.keys.scrollWidth <= this.keys.clientWidth) {
        break;
      }
      hints = hints.filter(([, , other]) => other < rank);
      this.keys.replaceChildren(...keyHints(hints));
    }
  }

  renderMenu() {
    this.menuElement.hidden = !this.menu;
    this.header.querySelector('.bookmarks-control')?.setAttribute('aria-expanded', String(Boolean(this.menu?.bookmarks)));
    if (!this.menu) {
      this.bookmarksSignature = null;
      return;
    }
    const { key, x, y, closed, bookmarks, document: documentId } = this.menu;
    this.menuElement.classList.toggle('closed-menu', Boolean(closed));
    this.menuElement.classList.toggle('bookmarks-menu', Boolean(bookmarks));
    // The bookmarks list holds entries with buttons of their own, not menu items.
    this.menuElement.setAttribute('role', bookmarks ? 'dialog' : 'menu');
    this.menuElement.removeAttribute('aria-label');
    if (bookmarks) {
      this.menuElement.setAttribute('aria-label', 'Bookmarks');
      this.renderBookmarks(key);
      this.placeMenu(x, y);
      return;
    }
    const items = closed ? this.closedMenuItems(key) : documentId ? this.documentMenuItems(key, documentId) : this.tabMenuItems(key);
    if (items.length === 0) {
      // The last closed document was reopened meanwhile, or the document of the menu closed.
      this.closeMenu();
      return;
    }
    for (const item of items) {
      item.addEventListener('click', () => this.closeMenu());
    }
    this.menuElement.replaceChildren(...items);
    this.placeMenu(x, y);
  }

  /** The menu at `x`, `y`, inside the window. */
  placeMenu(x, y) {
    const width = this.menuElement.offsetWidth;
    const height = this.menuElement.offsetHeight;
    this.menuElement.style.setProperty('left', `${Math.min(x, window.innerWidth - width - 4)}px`);
    this.menuElement.style.setProperty('top', `${Math.min(y, window.innerHeight - height - 4)}px`);
  }

  tabMenuItems(key) {
    const windows = this.state.arrangement.windows;
    return [
      menuItem('Move to new window', () => this.backend.moveTab(key, null)),
      ...windows
        .map((window, index) => [window, index])
        .filter(([window]) => window.id !== this.backend.windowId)
        .map(([window, index]) => menuItem(`Move to window ${index + 1}${this.windowProjects(window)}`, () => this.backend.moveTab(key, window.id))),
      h('hr'),
      menuItem('Close tab', () => this.backend.closeTab(key)),
    ];
  }

  /**
   * A document of the strip: beside the one shown, or, while split, on a side or alone; and
   * its close for everyone. The same document is never shown in both panes.
   */
  documentMenuItems(key, id) {
    const document = documentsOf(this.contents.get(key), false).find((candidate) => candidate.id === id);
    if (!document) {
      return [];
    }
    const showing = this.paneShowing(key, id);
    const items = [];
    if (!this.isSplit(key)) {
      items.push(
        showing
          ? menuItem('Split: show the next document beside it (\\)', () => this.split(key, null))
          : menuItem('Open on the right', () => this.split(key, id)),
      );
    } else if (showing) {
      items.push(menuItem('Show only this document', () => this.unsplit(key, paneSide(showing))));
    } else {
      items.push(
        ...SIDES.map((side) =>
          menuItem(`Show on the ${side}`, () => {
            this.updateView(key, { activePane: side });
            this.showDocument(paneId(key, side), id);
          }),
        ),
      );
    }
    return [...items, h('hr'), menuItem('Close for everyone', () => this.closeDocument(key, document))];
  }

  /** The closed documents of the tab, as the store has them; picking one reopens it for everyone. */
  closedMenuItems(key) {
    return documentsOf(this.contents.get(key), true).map((document) => {
      const item = menuItem(document.title, () => this.reopenDocument(key, document));
      item.title = `Reopen “${document.title}” for everyone`;
      return item;
    });
  }

  /**
   * The bookmarks list, rebuilt only when what it shows changed; the keyboard stays on the
   * entry it was on, or the one in its place when that entry went.
   */
  renderBookmarks(key) {
    const entries = sessionBookmarks(this.contents.get(key));
    const signature = JSON.stringify([
      key,
      this.isLive(key),
      entries.map(({ bookmark, document, closed, row }) => [bookmark, document.id, document.title, closed, row?.number, row?.title, row && quoteFound(row, bookmark)]),
    ]);
    if (signature === this.bookmarksSignature) {
      return;
    }
    this.bookmarksSignature = signature;
    const active = this.menuElement.contains(document.activeElement) ? document.activeElement : null;
    const index = [...this.menuElement.querySelectorAll('.bookmark-entry')].indexOf(active?.closest('.bookmark-entry'));
    this.menuElement.replaceChildren(...this.bookmarkItems(key, entries));
    if (!active) {
      return;
    }
    const jumps = [...this.menuElement.querySelectorAll('.bookmark-jump')];
    (this.menuElement.querySelector(`[data-control="${active.dataset.control}"]`) ?? jumps[Math.min(index, jumps.length - 1)])?.focus();
  }

  /** Every bookmark of the tab's conversation (`sessionBookmarks`), or how to make one. */
  bookmarkItems(key, entries) {
    const readOnly = this.isLive(key) ? null : (READ_ONLY_REASONS[this.session(key)?.link.state ?? 'waiting'] ?? 'Not connected.');
    const heading = h(
      'div',
      { class: 'bookmarks-heading' },
      h('h3', { text: `Bookmarks (${entries.length})` }),
      entries.length > 0 ? h('span', { class: 'hint', text: 'J K move · Enter go · Del remove' }) : null,
    );
    if (entries.length === 0) {
      return [heading, h('p', { class: 'hint', text: `No bookmarks in this conversation yet. ${BOOKMARK_NOTE}` })];
    }
    return [heading, ...entries.map((entry) => this.bookmarkEntry(key, entry, readOnly))];
  }

  /**
   * One bookmark in the list: its document, the item's number and title and the quote, as
   * written. Choosing it goes to the item; in a closed document, greyed, it first asks for its
   * Reopen button. Its × removes the bookmark; Reopen and × are unavailable while `readOnly`.
   */
  bookmarkEntry(key, { bookmark, document, closed, row }, readOnly) {
    const item = row ? `${row.number} “${row.title}”` : 'that is gone';
    const changed = row !== null && !quoteFound(row, bookmark);
    const jump = h(
      'button',
      {
        class: 'bookmark-jump',
        'data-control': `jump-${bookmark.id}`,
        title: closed ? `“${document.title}” is closed for everyone: Reopen opens it and goes to the item` : `Go to item ${item} in “${document.title}”`,
        'aria-label': `${document.title}${closed ? ', closed' : ''}: item ${item}${bookmark.quote ? `, quote “${bookmark.quote}”` : ''}${changed ? ', text changed' : ''}`,
        onclick: () => (closed ? this.askReopen(bookmark, document) : this.jumpTo(key, document.id, bookmark.item)),
      },
      h('span', { class: 'bookmark-where' }, h('span', { class: 'bookmark-document', text: document.title }), closed ? h('span', { class: 'chip', text: 'closed' }) : null),
      h('span', { class: 'bookmark-item' }, h('span', { class: 'number', text: row ? String(row.number) : '–' }), h('span', { class: 'title', text: row?.title ?? 'missing item' })),
      bookmark.quote ? h('span', { class: 'bookmark-quote', text: bookmark.quote }) : null,
      changed ? h('span', { class: 'chip text-changed', title: 'The item’s text no longer contains this quote', text: 'text changed' }) : null,
    );
    const reopen = closed
      ? h('button', {
          class: 'button reopen',
          'data-control': `reopen-${bookmark.id}`,
          disabled: Boolean(readOnly),
          title: readOnly ?? `Reopen “${document.title}” for everyone and go to the item`,
          text: 'Reopen',
          onclick: () => this.reopenToBookmark(key, document, bookmark.item),
        })
      : null;
    const remove = h('button', {
      class: 'close',
      'data-control': `remove-${bookmark.id}`,
      disabled: Boolean(readOnly),
      title: readOnly ?? 'Remove this bookmark for everyone; U undoes (Del)',
      'aria-label': 'Remove this bookmark',
      text: '×',
      onclick: () => this.removeBookmark(key, document, bookmark),
    });
    return h('div', { class: `bookmark-entry${closed ? ' closed' : ''}`, 'data-id': bookmark.id }, jump, reopen, remove);
  }

  /** A bookmark of a closed document chosen: its Reopen button takes the keyboard and confirms. */
  askReopen(bookmark, document) {
    this.menuElement.querySelector(`[data-control="reopen-${bookmark.id}"]`)?.focus();
    this.setMessage(`“${document.title}” is closed for everyone: Reopen opens it again and goes to the item.`);
  }

  windowProjects(window) {
    const names = [...new Set(window.tabs.map((key) => this.session(key)).filter(Boolean).map((session) => projectName(session.project)))];
    return names.length > 0 ? ` · ${names.join(', ')}` : '';
  }

  setMessage(text, tone = '') {
    this.message = { text, tone };
    this.renderStatus();
  }

  // Side pane width

  /**
   * The narrowest and widest pane the window has room for now; two documents side by side need
   * more room, one shown alone (`compact`) does not.
   */
  paneLimits() {
    const key = this.activeKey();
    const content = key && this.isSplit(key) && this.splitFits() ? 2 * CONTENT_MIN_WIDTH + DIVIDER_WIDTH : CONTENT_MIN_WIDTH;
    return [PANE_MIN_WIDTH, Math.max(PANE_MIN_WIDTH, this.body.clientWidth - content)];
  }

  clampPaneWidth(width) {
    const [min, max] = this.paneLimits();
    return Math.min(Math.max(width, min), max);
  }

  /** The width the pane shows: the one chosen (or the default) within the limits of the window now. */
  shownPaneWidth() {
    return this.clampPaneWidth(this.paneWidth ?? PANE_DEFAULT_WIDTH);
  }

  /** Shows the pane width; the narrow layout ignores it and keeps the one chosen for later. */
  showPaneWidth() {
    if (this.narrowLayout.matches || this.body.clientWidth === 0) {
      return;
    }
    const [min, max] = this.paneLimits();
    const width = this.shownPaneWidth();
    this.body.style.setProperty('--pane-width', `${width}px`);
    this.separator.setAttribute('aria-valuemin', String(min));
    this.separator.setAttribute('aria-valuemax', String(max));
    this.separator.setAttribute('aria-valuenow', String(width));
  }

  /** A width the user chose, or null for the default; saved shortly after the last change. The split is limited again. */
  setPaneWidth(width) {
    this.paneWidth = width;
    this.showPaneWidth();
    this.showSplit();
    clearTimeout(this.paneSaveTimer);
    this.paneSaveTimer = setTimeout(() => this.backend.setPaneWidth(this.paneWidth), VIEW_SAVE_DELAY_MS);
  }

  /** The pane grows as its left edge moves left. */
  onSeparatorDown(event) {
    this.startDrag(event, this.separator, this.shownPaneWidth(), (width, dx) => this.setPaneWidth(this.clampPaneWidth(width - dx)));
  }

  /** Left and Right move the edge a step, Home and End to the narrowest and widest pane. */
  onSeparatorKey(event) {
    const [min, max] = this.paneLimits();
    const width = this.shownPaneWidth();
    const widths = { ArrowLeft: width + PANE_KEY_STEP, ArrowRight: width - PANE_KEY_STEP, Home: min, End: max };
    this.onResizeKey(event, widths, (value) => this.setPaneWidth(this.clampPaneWidth(value)));
  }

  // Resize handles: the side pane's edge and the split divider

  /**
   * A press on a resize handle: as the pointer moves, `apply(from, dx)` sets the value `from`
   * it had at the press, moved by `dx` pixels.
   */
  startDrag(event, handle, from, apply) {
    if (event.button !== 0) {
      return;
    }
    this.drag = { id: event.pointerId, x: event.clientX, from, handle, apply };
    // No text selection and the resize cursor everywhere while dragging.
    this.body.classList.add('resizing');
    handle.classList.add('dragging');
    try {
      handle.setPointerCapture(event.pointerId);
    } catch {
      // The pointer is already gone; the drag ends with its pointerup anyway.
    }
  }

  onDragMove(event) {
    if (this.drag?.id !== event.pointerId) {
      return;
    }
    this.drag.apply(this.drag.from, event.clientX - this.drag.x);
  }

  endDrag(event) {
    if (this.drag?.id !== event.pointerId) {
      return;
    }
    this.drag.handle.classList.remove('dragging');
    this.drag = null;
    this.body.classList.remove('resizing');
  }

  /** A key on a resize handle sets the value `values` has for it; handled here, it neither moves the focus nor pans the diagram. */
  onResizeKey(event, values, set) {
    if (event.metaKey || event.ctrlKey || event.altKey || !(event.key in values)) {
      return;
    }
    event.preventDefault();
    set(values[event.key]);
  }

  // Split divider

  /** The width the two panes share. */
  splitWidth() {
    return this.content.clientWidth - DIVIDER_WIDTH;
  }

  /** The smallest and largest share of the left pane that leave both CONTENT_MIN_WIDTH. */
  splitLimits() {
    const width = this.splitWidth();
    if (width < 2 * CONTENT_MIN_WIDTH) {
      return [DEFAULT_SPLIT_RATIO, DEFAULT_SPLIT_RATIO];
    }
    return [CONTENT_MIN_WIDTH / width, 1 - CONTENT_MIN_WIDTH / width];
  }

  /**
   * Whether two panes fit beside the narrowest side pane. Decided by the body, not by the
   * content, whose width follows the side pane that this decides the limits of.
   */
  splitFits() {
    return !this.narrowLayout.matches && this.body.clientWidth - PANE_MIN_WIDTH >= 2 * CONTENT_MIN_WIDTH + DIVIDER_WIDTH;
  }

  clampSplitRatio(ratio) {
    const [min, max] = this.splitLimits();
    return Math.min(Math.max(ratio, min), max);
  }

  /** The left pane's share the tab shows: the one chosen (or half) within the limits of the window now. */
  shownSplitRatio(key) {
    return this.clampSplitRatio(this.view(key).ratio ?? DEFAULT_SPLIT_RATIO);
  }

  /** A share the user chose for the tab, or null for half; saved with its view state. */
  setSplitRatio(key, ratio) {
    this.storeView(key, { ratio });
    this.showSplit();
  }

  /**
   * Shows the active tab's split: where the divider is, or only the active pane where two do
   * not fit (the narrow layout, a small window).
   */
  showSplit() {
    const key = this.activeKey();
    if (!key || !this.isSplit(key) || this.content.clientWidth === 0) {
      return;
    }
    this.content.classList.toggle('compact', !this.splitFits());
    const [min, max] = this.splitLimits();
    const ratio = this.shownSplitRatio(key);
    const percent = (share) => String(Math.round(share * 100));
    this.content.style.setProperty('--split-ratio', String(ratio));
    this.divider.setAttribute('aria-valuemin', percent(min));
    this.divider.setAttribute('aria-valuemax', percent(max));
    this.divider.setAttribute('aria-valuenow', percent(ratio));
    this.divider.setAttribute('aria-valuetext', `Left document ${percent(ratio)}% of the width`);
  }

  /** The left pane grows as the divider moves right. */
  onDividerDown(event) {
    const key = this.activeKey();
    this.startDrag(event, this.divider, this.shownSplitRatio(key), (ratio, dx) => this.setSplitRatio(key, this.clampSplitRatio(ratio + dx / this.splitWidth())));
  }

  /** Left and Right move the divider a step, Home and End to the smallest and largest left pane. */
  onDividerKey(event) {
    const key = this.activeKey();
    const [min, max] = this.splitLimits();
    const ratio = this.shownSplitRatio(key);
    const step = PANE_KEY_STEP / this.splitWidth();
    const ratios = { ArrowLeft: ratio - step, ArrowRight: ratio + step, Home: min, End: max };
    this.onResizeKey(event, ratios, (value) => this.setSplitRatio(key, this.clampSplitRatio(value)));
  }

  // Tabs and windows

  activate(key) {
    // A jump waiting for a reopened document's content is over once the user goes elsewhere.
    this.pendingJump = null;
    this.backend.activateTab(key);
  }

  cycleTabs(step) {
    const window = this.myWindow();
    if (!window || window.tabs.length === 0) {
      return;
    }
    const index = window.tabs.indexOf(window.active);
    this.activate(window.tabs[(index + step + window.tabs.length) % window.tabs.length]);
  }

  onTabKey(event, key, tab) {
    if (event.key === 'ContextMenu' || (event.key === 'F10' && event.shiftKey)) {
      event.preventDefault();
      const rect = tab.getBoundingClientRect();
      this.openMenu(key, rect.left, rect.bottom);
    } else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      event.stopPropagation();
      this.activate(key);
    } else if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
      event.preventDefault();
      const sibling = event.key === 'ArrowRight' ? tab.nextElementSibling : tab.previousElementSibling;
      sibling?.focus();
    }
  }

  /** The tab's menu, or with `document` that document's menu in the strip. */
  openMenu(key, x, y, document = null) {
    this.menu = { key, x, y, document };
    this.renderMenu();
    this.menuElement.querySelector('button')?.focus();
  }

  /** Closed (N): the closed documents in a menu below the control. */
  openClosedMenu(key, control) {
    const rect = control.getBoundingClientRect();
    this.menu = { key, x: rect.left, y: rect.bottom + 4, closed: true };
    this.renderMenu();
    this.menuElement.querySelector('button')?.focus();
  }

  /**
   * Bookmarks (N) or Shift+B: the bookmarks list below the control, with the keyboard on its
   * first entry. The control is there while the conversation has bookmarks.
   */
  openBookmarks(key) {
    const control = this.header.querySelector('.bookmarks-control');
    if (!control) {
      this.setMessage(`No bookmarks in this conversation yet. ${BOOKMARK_NOTE}`);
      return;
    }
    const rect = control.getBoundingClientRect();
    this.menu = { key, x: rect.left, y: rect.bottom + 4, bookmarks: true };
    this.renderMenu();
    (this.menuElement.querySelector('.bookmark-jump') ?? this.menuElement).focus();
  }

  closeMenu() {
    this.menu = null;
    this.renderMenu();
  }

  /** Esc or Shift+B in the bookmarks list: it closes, and the keyboard goes back to its control. */
  closeBookmarks() {
    const inside = this.menuElement.contains(document.activeElement);
    this.closeMenu();
    if (inside || document.activeElement === document.body) {
      this.header.querySelector('.bookmarks-control')?.focus();
    }
  }

  toggleSessions() {
    this.sessionsOpen = !this.sessionsOpen;
    this.render();
  }

  dismiss(event) {
    if (this.menu && !this.menuElement.contains(event.target)) {
      this.closeMenu();
    }
    if (this.sessionsOpen && !this.popover.contains(event.target) && !this.sessionsButton.contains(event.target)) {
      this.sessionsOpen = false;
      this.render();
    }
  }

  onDragOver(event) {
    if (!event.dataTransfer.types.includes('text/plain')) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = 'move';
    this.tabstrip.classList.add('drop-target');
  }

  /** A tab dragged in from another window moves here. */
  onDrop(event) {
    this.tabstrip.classList.remove('drop-target');
    const data = event.dataTransfer.getData('text/plain');
    if (!data.startsWith(TAB_DRAG_PREFIX)) {
      return;
    }
    event.preventDefault();
    const key = data.slice(TAB_DRAG_PREFIX.length);
    if (!this.myWindow()?.tabs.includes(key)) {
      this.backend.moveTab(key, this.backend.windowId);
    }
  }

  /** A tab dropped outside every window opens in a new one, unless another window took it. */
  onDragEnd(event, key) {
    const outside =
      event.screenX < window.screenX ||
      event.screenX > window.screenX + window.outerWidth ||
      event.screenY < window.screenY ||
      event.screenY > window.screenY + window.outerHeight;
    if (event.dataTransfer.dropEffect !== 'none' || !outside) {
      return;
    }
    setTimeout(async () => {
      const state = await this.backend.state();
      const mine = state.arrangement.windows.find((candidate) => candidate.id === this.backend.windowId);
      if (mine?.tabs.includes(key)) {
        this.backend.moveTab(key, null);
      }
    }, DRAG_OUT_CHECK_DELAY_MS);
  }

  // Items

  focus(pane, id) {
    if (this.paneView(pane).focus !== id) {
      this.updatePane(pane, { focus: id });
    }
  }

  moveFocus(pane, step) {
    const current = this.current(pane);
    if (!current || current.rows.length === 0) {
      return;
    }
    const index = current.rows.findIndex((row) => row.id === current.focus?.id);
    const next = step === Infinity ? current.rows.length - 1 : step === -Infinity ? 0 : Math.min(Math.max(index + step, 0), current.rows.length - 1);
    this.focus(pane, current.rows[next].id);
    this.listView(pane).reveal(current.rows[next].id);
  }

  /** [ and ]: the pane's previous or next document; while split, the other pane's is left out. */
  cycleDocuments(pane, step) {
    const current = this.current(pane);
    const key = paneTab(pane);
    const beside = this.isSplit(key) ? this.current(this.otherPane(pane)).document.id : null;
    const documents = (current?.documents ?? []).filter((document) => document.id !== beside);
    if (documents.length < 2) {
      return;
    }
    const index = documents.findIndex((document) => document.id === current.document.id);
    const next = documents[(index + step + documents.length) % documents.length];
    this.showDocument(pane, next.id);
  }

  /**
   * Another document in a pane: the focus and selection of the document left are kept for
   * it, and those kept for the one shown come back. A document the other pane shows is not
   * shown twice: that pane becomes active instead.
   */
  showDocument(pane, id) {
    const key = paneTab(pane);
    if (this.isSplit(key) && this.current(this.otherPane(pane)).document.id === id) {
      this.updateView(key, { activePane: paneSide(this.otherPane(pane)) });
      return;
    }
    // U acts on the document shown from now on.
    this.lastClosed = null;
    const view = this.paneView(pane);
    const kept = keepFor(view.kept, this.current(pane)?.document.id, view);
    const back = keptFor(kept, id);
    this.updatePane(pane, { document: id, focus: back.focus, selected: back.selected, kept });
  }

  focusFilter() {
    this.filterInput.focus();
    this.filterInput.select();
  }

  /** Enter or Esc in the filter: the keyboard goes back to the items, the filter stays. */
  leaveFilter() {
    this.focusPane(this.activePane(this.activeKey()));
  }

  /** The keyboard goes to a pane's items: the focused item of its diagram, or its list; without items, the pane. */
  focusPane(pane) {
    const slot = this.slots[paneSide(pane)];
    if (!slot.shown) {
      slot.element.focus({ preventScroll: true });
    } else if (this.mode(pane) === 'diagram') {
      this.diagramView(pane).restoreKeyboard();
    } else {
      this.listView(pane).element.focus();
    }
  }

  /** What an action acts on: the selected items shown, else the focused one (in the diagram, only one the user picked). */
  targets(pane) {
    const current = this.current(pane);
    if (!current) {
      return [];
    }
    if (current.selected.length > 0) {
      return current.selected;
    }
    const focus = this.mode(pane) === 'diagram' ? this.diagramFocus(pane) : current.focus;
    return focus ? [focus] : [];
  }

  /** Space, or a click with ⌘/Ctrl: the item joins or leaves the selection and takes the focus. */
  toggleSelection(pane, id) {
    const selected = this.paneView(pane).selected;
    this.updatePane(pane, { focus: id, selected: selected.includes(id) ? selected.filter((other) => other !== id) : [...selected, id] });
  }

  /** Shift+A: every item shown is selected, as in the terminal list. */
  selectAll(pane) {
    const rows = this.current(pane)?.rows ?? [];
    this.updatePane(pane, { selected: rows.map((row) => row.id) });
  }

  clearSelection(pane) {
    if (this.paneView(pane).selected.length > 0) {
      this.updatePane(pane, { selected: [] });
    }
  }

  /** Removes items of a document from the selection of the pane showing it, for example once an action on them committed. */
  deselect(key, documentId, ids) {
    const pane = this.paneShowing(key, documentId);
    const selected = pane ? this.paneView(pane).selected : [];
    if (selected.some((id) => ids.includes(id))) {
      this.updatePane(pane, { selected: selected.filter((id) => !ids.includes(id)) });
    }
  }

  /** The item picked in the diagram, if it is still shown. */
  diagramFocus(pane) {
    return this.current(pane)?.rows.find((row) => row.id === this.paneView(pane).focus) ?? null;
  }

  /** J and K in the diagram follow the layout's order: boxes with their badges, then arrows in step order (lanes are left out). */
  moveDiagramFocus(pane, step) {
    const order = this.diagramView(pane).diagram?.order ?? [];
    if (order.length === 0) {
      return;
    }
    const index = order.indexOf(this.paneView(pane).focus);
    let next;
    if (step === Infinity || step === -Infinity) {
      next = step > 0 ? order.length - 1 : 0;
    } else if (index === -1) {
      next = step > 0 ? 0 : order.length - 1;
    } else {
      next = Math.min(Math.max(index + step, 0), order.length - 1);
    }
    this.focus(pane, order[next]);
  }

  clearDiagramFocus(pane) {
    this.focus(pane, null);
    this.diagramView(pane).focusCanvas();
  }

  async exportDiagram(pane) {
    const current = this.current(pane);
    const svg = current ? this.diagramView(pane).exportSvg() : null;
    if (!svg) {
      this.setMessage('The diagram is not ready to export yet.', 'error');
      return;
    }
    try {
      const path = await this.backend.exportSvg(current.document.title, svg);
      if (path) {
        this.setMessage(`Saved the diagram to ${path}.`, 'success');
      }
    } catch (error) {
      this.setMessage(String(error), 'error');
    }
  }

  isLive(key) {
    return this.session(key)?.link.state === 'live';
  }

  /** Whether shared commands can go out; if not, says why instead of sending. */
  requireLive(key) {
    if (this.isLive(key)) {
      return true;
    }
    this.setMessage(READ_ONLY_REASONS[this.session(key)?.link.state ?? 'waiting'] ?? 'Not connected.', 'error');
    return false;
  }

  /** The targets of an action, or none with a hint when nothing is focused or selected. */
  pickedTargets(pane) {
    const targets = this.targets(pane);
    if (targets.length === 0) {
      this.setMessage('Pick an item first: click it, or move to it with J and K.', 'error');
    }
    return targets;
  }

  /** The targets of Ask, Explain, Accept and Drop: dropped items take no part; with only dropped ones picked, none and why. */
  sharedTargets(pane) {
    const targets = this.pickedTargets(pane);
    const active = targets.filter((row) => !row.dropped);
    if (targets.length > 0 && active.length === 0) {
      this.setMessage(DROPPED_NOTE, 'error');
    }
    return active;
  }

  /**
   * Shares the focused or selected items of a pane with the CLI prompt box right away, where
   * the user types the question. Text selected inside the focused item's text in the panel
   * becomes its quote; every other item is quoted in full. With `intent` explain, the agent
   * is asked to explain them, and the side pane shows its explanation on each item.
   */
  async ask(pane, intent = null) {
    const key = paneTab(pane);
    if (!this.requireLive(key)) {
      return;
    }
    if (this.asking.has(key)) {
      return;
    }
    if (intent && this.explainBlocked(key)) {
      this.setMessage(this.explainBlocked(key), 'error');
      return;
    }
    const targets = this.sharedTargets(pane);
    if (targets.length === 0) {
      return;
    }
    const current = this.current(pane);
    const focus = this.mode(pane) === 'diagram' ? this.diagramFocus(pane) : current.focus;
    const quoted = focus ? this.detailPanel(pane).selectedText(focus.id) : '';
    // The item quoted by the selected part, if it is one of the targets.
    const quotedItem = quoted.trim() && targets.some((row) => row.id === focus.id) ? focus.id : null;
    const items = targets.map((row) => ({ number: row.number, title: row.title, quote: row.id === quotedItem ? quoted : row.details }));
    const numbered = `${items.length === 1 ? 'item' : 'items'} ${items.map((item) => item.number).join(', ')}`;
    const release = () => this.asking.delete(key);
    // Once delivered, an explain request is followed until its explanation arrives (`showArrivedExplanations`).
    const sentAt = Date.now();
    const delivered = (id) => {
      release();
      if (intent) {
        this.explaining.set(id, { key, documentId: current.document.id, itemId: targets[0].id, number: targets[0].number, sentAt });
      }
    };
    this.asking.add(key);
    const sent = await this.send(
      key,
      {
        type: 'ask',
        selection: { document_id: current.document.id, revision: current.document.revision, ids: targets.map((row) => row.id) },
        items,
        ...(intent ? { intent } : {}),
        ...(quotedItem ? { quoted: quotedItem } : {}),
      },
      {
        sending: intent ? 'Asking for an explanation…' : 'Sharing…',
        done: intent ? `Asked the conversation to explain ${numbered}.` : `Shared ${numbered} with the CLI prompt box.`,
        onSuccess: delivered,
        onError: release,
      },
    );
    if (!sent) {
      release();
    }
  }

  /** E or Explain: asks the conversation to explain what Ask would share; the answer shows in the side pane. */
  explain(pane) {
    return this.ask(pane, EXPLAIN_INTENT);
  }

  /**
   * B or Bookmark. With text selected in the focused item's text, bookmarks that text, or
   * removes the bookmark of the same text. Otherwise toggles the bookmark on the whole of each
   * target: removes them when every one has it, else adds it to those without. Dropped items
   * take part: a bookmark is only a mark.
   */
  async bookmark(pane) {
    const key = paneTab(pane);
    if (!this.requireLive(key)) {
      return;
    }
    const targets = this.pickedTargets(pane);
    if (targets.length === 0) {
      return;
    }
    const current = this.current(pane);
    const document = current.document;
    const focus = this.mode(pane) === 'diagram' ? this.diagramFocus(pane) : current.focus;
    const panel = this.detailPanel(pane);
    // A selection reaching beyond the item's text is no quote of it, and no reason to mark the whole item either.
    if (focus && panel.selectionBeyond(focus.id)) {
      this.setMessage('The selection reaches beyond the item’s text: select text inside it to bookmark that text.', 'error');
      return;
    }
    const quote = focus ? panel.selectedText(focus.id) : '';
    const title = this.splitTitle(pane);
    const where = title ? ` in ${title}` : '';
    if (quote.trim()) {
      const same = itemBookmarks(document, focus.id).find((bookmark) => collapsed(bookmark.quote ?? '') === collapsed(quote));
      const action = same ? { action: 'unbookmark', ids: [same.id] } : { action: 'bookmark', ids: [focus.id], quote };
      await this.sendBookmark(key, document, action, `${same ? 'Removed the bookmark of' : 'Bookmarked'} the text selected in “${focus.title}”${where}.`);
      return;
    }
    const unmarked = targets.filter((row) => !itemMark(document, row.id));
    if (unmarked.length > 0) {
      await this.sendBookmark(key, document, { action: 'bookmark', ids: unmarked.map((row) => row.id) }, `Bookmarked ${described(unmarked)}${where}.`);
    } else {
      await this.sendBookmark(key, document, { action: 'unbookmark', ids: targets.map((row) => row.id) }, `Removed the bookmark of ${described(targets)}${where}.`);
    }
  }

  /** The × of a bookmark in the side pane or the list: removes it for everyone. */
  async removeBookmark(key, document, bookmark) {
    if (!this.requireLive(key)) {
      return;
    }
    await this.sendBookmark(key, document, { action: 'unbookmark', ids: [bookmark.id] }, `Removed a bookmark in “${document.title}”.`);
  }

  /** Sends a bookmark change of `document`; once it committed and until the next command, U undoes it. */
  async sendBookmark(key, document, action, done) {
    // `send` counts this command first; another one after it ends the moment for U.
    const count = this.commandCount + 1;
    await this.send(
      key,
      { type: 'action', document_id: document.id, expected_revision: document.revision, action },
      {
        sending: action.action === 'bookmark' ? 'Bookmarking…' : 'Removing the bookmark…',
        done: `${done} U undoes.`,
        onSuccess: () => {
          if (this.commandCount === count) {
            this.lastBookmarked = { key, id: document.id, revision: document.revision + 1 };
          }
        },
      },
    );
  }

  /** The document of this window's last bookmark change, while nothing changed it and nothing was sent since. */
  justBookmarked(key) {
    const last = this.lastBookmarked;
    if (last?.key !== key) {
      return null;
    }
    return this.contents.get(key)?.documents.find((document) => document.id === last.id && document.revision === last.revision) ?? null;
  }

  /**
   * A bookmark chosen in the list: item `id` of the open document `documentId` in the active
   * pane, or in the other pane while that one shows the document; focused, in view and with
   * the keyboard, in the list or the diagram. A hidden item is shown again, and a filter that
   * leaves it out is cleared. A document reopened for it may not be here yet: the jump waits
   * for its content.
   */
  jumpTo(key, documentId, id) {
    this.closeMenu();
    const document = documentsOf(this.contents.get(key), false).find((candidate) => candidate.id === documentId);
    if (!document) {
      this.pendingJump = { key, documentId, id };
      return;
    }
    this.pendingJump = null;
    const pane = this.paneShowing(key, documentId) ?? this.activePane(key);
    if (this.isSplit(key)) {
      this.storeView(key, { activePane: paneSide(pane) });
    }
    if (this.current(pane)?.document.id !== documentId) {
      this.showDocument(pane, documentId);
    }
    const hidden = this.view(key).hidden;
    if (hidden.includes(id)) {
      this.storeView(key, { hidden: hidden.filter((other) => other !== id) });
    }
    const view = this.paneView(pane);
    const shown = buildRows(document, new Set(view.hidden), view.filter).some((row) => row.id === id);
    this.updatePane(pane, { focus: id, ...(shown ? {} : { filter: '' }) });
    this.revealFocus(pane, documentId, id);
  }

  /** The pane's focused item `id` of `documentId` comes into view with the keyboard, in the list or the diagram. */
  revealFocus(pane, documentId, id) {
    if (this.mode(pane) === 'diagram') {
      this.diagramView(pane).revealItem(documentId, id);
    } else {
      this.listView(pane).reveal(id);
      this.focusPane(pane);
    }
  }

  /**
   * Explain requests this window sent whose explanation arrived in the active tab: each shows
   * it (`showExplanation`); those that failed, were lost or timed out are forgotten, as are
   * those of another tab that timed out.
   */
  showArrivedExplanations() {
    const now = Date.now();
    for (const [id, request] of [...this.explaining].filter(([, request]) => now - request.sentAt > EXPLAIN_TIMEOUT_MS)) {
      this.explaining.delete(id);
    }
    const key = this.activeKey();
    const content = this.contents.get(key);
    if (!content) {
      return;
    }
    const ended = this.session(key)?.link.state === 'ended';
    for (const [id, request] of [...this.explaining].filter(([, request]) => request.key === key)) {
      const question = content.questions.find((candidate) => candidate.request_id === id);
      const status = question ? explanationOf([question], { ended, now }).status : null;
      if (status === 'done') {
        this.explaining.delete(id);
        this.showExplanation(request);
      } else if (status === 'failed' || status === 'lost') {
        this.explaining.delete(id);
      }
    }
  }

  /**
   * The explanation of an item this window asked about arrived: the pane that shows its
   * document becomes active and focuses the item, and the side pane scrolls to the explanation.
   * While the user types in a field or has text selected, only an item focused already is
   * scrolled to. Otherwise, or when the document or the item is not shown, the status bar says
   * where the explanation is.
   */
  showExplanation({ key, documentId, itemId, number }) {
    const pane = this.paneShowing(key, documentId);
    const shown = pane !== null && this.current(pane).rows.some((row) => row.id === itemId);
    const busy = Boolean(document.activeElement?.closest?.('input, textarea')) || window.getSelection()?.isCollapsed === false;
    if (shown && !busy) {
      if (this.isSplit(key)) {
        this.storeView(key, { activePane: paneSide(pane) });
      }
      this.updatePane(pane, { focus: itemId });
      this.revealFocus(pane, documentId, itemId);
      this.detailPanel(pane).revealExplanation();
      return;
    }
    const focus = shown && this.activePane(key) === pane ? (this.mode(pane) === 'diagram' ? this.diagramFocus(pane) : this.current(pane).focus) : null;
    if (focus?.id === itemId) {
      this.detailPanel(pane).revealExplanation();
      return;
    }
    const title = this.contents.get(key).documents.find((candidate) => candidate.id === documentId)?.title;
    this.setMessage(`Explanation for item ${number} arrived in “${title}”.`);
  }

  /** Reopen in the list: reopens the closed document for everyone, then goes to the bookmarked item. */
  async reopenToBookmark(key, document, id) {
    this.closeMenu();
    if (!this.requireLive(key)) {
      return;
    }
    await this.send(
      key,
      { type: 'action', document_id: document.id, expected_revision: document.revision, action: { action: 'reopen' } },
      { sending: 'Reopening…', done: `Reopened “${document.title}”.`, onSuccess: () => this.jumpTo(key, document.id, id) },
    );
  }

  hide(pane) {
    const targets = this.pickedTargets(pane);
    if (targets.length === 0) {
      return;
    }
    const ids = targets.map((row) => row.id);
    const view = this.paneView(pane);
    this.updatePane(pane, { hidden: [...view.hidden, ...ids], selected: view.selected.filter((id) => !ids.includes(id)) });
    this.setMessage(`Hid ${described(targets)} in this tab. Shift+H shows hidden items.`);
  }

  /** While split, the pane's document as a message names it, “Title”; with one pane, null. */
  splitTitle(pane) {
    return this.isSplit(paneTab(pane)) ? `“${this.current(pane).document.title}”` : null;
  }

  drop(pane) {
    const targets = this.isLive(paneTab(pane)) ? this.sharedTargets(pane) : [];
    const title = this.splitTitle(pane);
    return this.applyAction(pane, 'archive', targets, {
      sending: 'Dropping…',
      done: `Dropped ${described(targets)}${title ? ` in ${title}` : ''}. U undoes.`,
    });
  }

  accept(pane) {
    const targets = this.isLive(paneTab(pane)) ? this.sharedTargets(pane) : [];
    const title = this.splitTitle(pane);
    return this.applyAction(pane, 'accept', targets, {
      sending: 'Accepting…',
      done: `Accepted ${described(targets)}${title ? ` in ${title}` : ''}. ${ACCEPT_NOTE}`,
    });
  }

  /** One `action` for all targets at the document's revision; once it committed, they leave the selection. */
  async applyAction(pane, action, targets, { sending, done }) {
    const key = paneTab(pane);
    const current = this.current(pane);
    if (!this.requireLive(key) || !current || targets.length === 0) {
      return;
    }
    const ids = targets.map((row) => row.id);
    await this.send(
      key,
      {
        type: 'action',
        document_id: current.document.id,
        expected_revision: current.document.revision,
        action: { action, ids },
      },
      { sending, done, onSuccess: () => this.deselect(key, current.document.id, ids) },
    );
  }

  /** W or ×: closes the document in every window and for the agent; right after, U reopens it. */
  async closeDocument(key, document) {
    if (!this.requireLive(key) || !document) {
      return;
    }
    // `send` counts this command first; another one after it ends the moment for U.
    const count = this.commandCount + 1;
    const showing = this.isSplit(key) ? this.paneShowing(key, document.id) : null;
    const side = showing ? paneSide(showing) : null;
    await this.send(
      key,
      { type: 'action', document_id: document.id, expected_revision: document.revision, action: { action: 'close' } },
      {
        sending: 'Closing…',
        done: `Closed “${document.title}” for everyone. U reopens it.`,
        onSuccess: () => {
          if (this.commandCount === count) {
            this.lastClosed = { key, id: document.id, revision: document.revision + 1, side };
          }
        },
      },
    );
  }

  /** From Closed (N): reopens the document for everyone and shows it in the active pane. */
  async reopenDocument(key, document) {
    if (!this.requireLive(key)) {
      return;
    }
    await this.send(
      key,
      { type: 'action', document_id: document.id, expected_revision: document.revision, action: { action: 'reopen' } },
      { sending: 'Reopening…', done: `Reopened “${document.title}”.`, onSuccess: () => this.showDocument(this.activePane(key), document.id) },
    );
  }

  /** The document this window closed last, while nothing changed it and nothing was sent since. */
  justClosed(key) {
    const last = this.lastClosed;
    if (last?.key !== key) {
      return null;
    }
    return documentsOf(this.contents.get(key), true).find((document) => document.id === last.id && document.revision === last.revision) ?? null;
  }

  /**
   * Reverts the latest undo record of the pane's document, whatever made it (drop, accept,
   * close, a tool); right after a close, that of the closed document, which reopens it in the
   * active pane, or on the right beside the document left when it was closed there.
   */
  async undo(pane) {
    const key = paneTab(pane);
    const errors = {
      not_found: 'Nothing to undo in this document.',
      // The store discards a record that can never apply, so the next undo reaches the one before.
      undo_conflict: 'The latest action conflicts with later changes and was discarded; U undoes the one before it.',
    };
    const closed = this.justClosed(key);
    if (closed) {
      if (!this.requireLive(key)) {
        return;
      }
      // `send` forgets the close.
      const onRight = this.lastClosed.side === 'right';
      const reopen = () => (onRight && !this.isSplit(key) ? this.split(key, closed.id) : this.showDocument(this.activePane(key), closed.id));
      await this.send(
        key,
        { type: 'undo', document_id: closed.id, expected_revision: closed.revision },
        { sending: 'Reopening…', done: `Reopened “${closed.title}”.`, errors, onSuccess: reopen },
      );
      return;
    }
    const current = this.current(pane);
    const bookmarked = this.justBookmarked(key);
    if (bookmarked && bookmarked.id !== current?.document.id) {
      if (!this.requireLive(key)) {
        return;
      }
      await this.send(
        key,
        { type: 'undo', document_id: bookmarked.id, expected_revision: bookmarked.revision },
        { sending: 'Undoing…', done: `Undid the bookmark change in “${bookmarked.title}”.`, errors },
      );
      return;
    }
    if (!this.requireLive(key) || !current) {
      return;
    }
    await this.send(
      key,
      { type: 'undo', document_id: current.document.id, expected_revision: current.document.revision },
      { sending: 'Undoing…', done: `Undid the latest action on ${this.splitTitle(pane) ?? 'this document'}.`, errors },
    );
  }

  /**
   * Sends a command and shows `sending`; its reply later runs `onSuccess` with the command ID
   * and shows `done`, or runs `onError` and shows the error, in the words of `errors[code]` when given.
   */
  async send(key, command, { sending, done, errors = {}, onSuccess = null, onError = null }) {
    // Any command ends the moment right after a close or a bookmark change, and a jump still waiting.
    this.commandCount += 1;
    this.lastClosed = null;
    this.lastBookmarked = null;
    this.pendingJump = null;
    let id;
    try {
      id = await this.backend.send(key, command);
    } catch (error) {
      this.setMessage(String(error), 'error');
      return false;
    }
    this.pending.set(id, { key, done, errors, onSuccess, onError });
    this.setMessage(sending);
    const early = this.earlyReplies.get(id);
    if (early) {
      this.earlyReplies.delete(id);
      this.onReply(early);
    }
    return true;
  }

  onReply(reply) {
    const pending = this.pending.get(reply.id);
    if (!pending) {
      // Possibly faster than `send` returning its ID: kept briefly for it.
      this.earlyReplies.set(reply.id, reply);
      setTimeout(() => this.earlyReplies.delete(reply.id), EARLY_REPLY_KEEP_MS);
      return;
    }
    this.pending.delete(reply.id);
    if (reply.error) {
      pending.onError?.();
      this.setMessage(pending.errors[reply.error.code] ?? errorText(reply.error), 'error');
    } else {
      pending.onSuccess?.(reply.id);
      this.setMessage(pending.done, 'success');
    }
  }

  // Keyboard

  onKey(event) {
    if (this.menu) {
      this.onMenuKey(event);
      return;
    }
    if (event.key === 'Escape' && this.sessionsOpen) {
      this.sessionsOpen = false;
      this.render();
      return;
    }
    // Handled where it happened, for example arrow keys on the tab strip.
    if (event.defaultPrevented || event.target.closest?.('input')) {
      return;
    }
    if (event.ctrlKey && event.key === 'Tab') {
      event.preventDefault();
      this.cycleTabs(event.shiftKey ? -1 : 1);
      return;
    }
    const paneSwitch = event.key === PANE_SWITCH_KEY && !event.ctrlKey && !event.metaKey && !event.altKey;
    if ((event.ctrlKey && event.code === PANE_SWITCH_CODE) || paneSwitch) {
      const key = this.activeKey();
      if (key) {
        event.preventDefault();
        this.switchPane(key);
      }
      return;
    }
    const modifier = event.metaKey || event.ctrlKey;
    // ⌘/Ctrl+0: the diagram at 100%, as actual size elsewhere.
    if (modifier && event.key === '0') {
      const key = this.activeKey();
      const pane = key ? this.activePane(key) : null;
      if (pane && this.mode(pane) === 'diagram') {
        event.preventDefault();
        this.diagramView(pane).zoomActual();
      }
      return;
    }
    if (modifier && /^[1-9]$/.test(event.key)) {
      const key = this.myWindow()?.tabs[Number(event.key) - 1];
      if (key) {
        event.preventDefault();
        this.activate(key);
      }
      return;
    }
    if (modifier || event.altKey) {
      return;
    }
    // Enter and Space on a focused button act on that button.
    if ((event.key === 'Enter' || event.key === ' ') && event.target.closest?.('button, [role="tab"]')) {
      return;
    }
    // Arrow keys in the detail panel scroll it instead of moving or panning.
    if (event.key.startsWith('Arrow') && event.target.closest?.('.detail-panel')) {
      return;
    }

    const key = this.activeKey();
    if (!key) {
      return;
    }
    // Holding Enter or E must not ask again and again, nor B toggle the bookmark or Shift+B the list.
    if ((event.key === 'Enter' || event.key === 'e' || event.key === 'b' || event.key === 'B') && event.repeat) {
      event.preventDefault();
      return;
    }
    // The keys act on the active pane; Shift+H and \ on the whole tab.
    const pane = this.activePane(key);
    const focus = () => this.current(pane)?.focus;
    const actions = {
      j: () => this.moveFocus(pane, 1),
      ArrowDown: () => this.moveFocus(pane, 1),
      k: () => this.moveFocus(pane, -1),
      ArrowUp: () => this.moveFocus(pane, -1),
      Home: () => this.moveFocus(pane, -Infinity),
      End: () => this.moveFocus(pane, Infinity),
      ' ': () => focus() && this.toggleSelection(pane, focus().id),
      A: () => this.selectAll(pane),
      Escape: () => this.clearSelection(pane),
      Enter: () => this.ask(pane),
      e: () => this.explain(pane),
      b: () => this.bookmark(pane),
      B: () => this.openBookmarks(key),
      a: () => this.accept(pane),
      h: () => this.hide(pane),
      H: () => this.updateView(key, { hidden: [] }),
      x: () => this.drop(pane),
      u: () => this.undo(pane),
      '/': () => this.focusFilter(),
      '[': () => this.cycleDocuments(pane, -1),
      ']': () => this.cycleDocuments(pane, 1),
      w: () => this.closeDocument(key, this.current(pane)?.document),
      v: () => this.switchMode(pane),
      '\\': () => this.toggleSplit(key),
    };
    // Any other key stops a zoom on its way; zoom keys go on from where it was heading.
    if (this.mode(pane) === 'diagram' && !ZOOM_KEYS.has(event.key) && !MODIFIER_KEYS.has(event.key)) {
      this.diagramView(pane).stopAnimation();
    }
    const diagram = this.mode(pane) === 'diagram' ? this.diagramKeys(pane) : {};
    const action = event.key in diagram ? diagram[event.key] : actions[event.key];
    if (action) {
      event.preventDefault();
      action();
    }
  }

  /** Keys of a pane's diagram, over those of the list. */
  diagramKeys(pane) {
    const diagram = this.diagramView(pane);
    const focused = () => this.diagramFocus(pane);
    const pan = (dx, dy) => () => diagram.panStep(dx, dy);
    return {
      j: () => this.moveDiagramFocus(pane, 1),
      k: () => this.moveDiagramFocus(pane, -1),
      Home: () => this.moveDiagramFocus(pane, -Infinity),
      End: () => this.moveDiagramFocus(pane, Infinity),
      ArrowLeft: pan(-1, 0),
      ArrowRight: pan(1, 0),
      ArrowUp: pan(0, -1),
      ArrowDown: pan(0, 1),
      '+': () => diagram.zoomStep(1),
      '=': () => diagram.zoomStep(1),
      '-': () => diagram.zoomStep(-1),
      0: () => diagram.fit(),
      // 1 to 3: titles, summary, full text.
      ...Object.fromEntries(LEVELS.map((level, index) => [index + 1, () => diagram.setLevel(level)])),
      ' ': () => focused() && this.toggleSelection(pane, focused().id),
      // A selection clears first, then the focus.
      Escape: () => (this.paneView(pane).selected.length > 0 ? this.clearSelection(pane) : this.clearDiagramFocus(pane)),
      s: () => this.exportDiagram(pane),
    };
  }

  onMenuKey(event) {
    if (this.menu.bookmarks) {
      this.onBookmarksKey(event);
      return;
    }
    const items = [...this.menuElement.querySelectorAll('button')];
    const index = items.indexOf(document.activeElement);
    if (event.key === 'Escape') {
      event.preventDefault();
      this.closeMenu();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      items[(index + step + items.length) % items.length]?.focus();
    }
  }

  /**
   * Keys in the bookmarks list: J, K and the arrows move between entries, Enter and Space
   * press the focused button, Delete and Backspace remove the entry's bookmark; Esc or
   * Shift+B closes the list.
   */
  onBookmarksKey(event) {
    const jumps = [...this.menuElement.querySelectorAll('.bookmark-jump')];
    const entry = document.activeElement?.closest?.('.bookmark-entry') ?? null;
    const index = jumps.indexOf(entry?.querySelector('.bookmark-jump'));
    const step = { j: 1, ArrowDown: 1, k: -1, ArrowUp: -1 }[event.key];
    if (event.key === 'Escape' || event.key === 'B') {
      event.preventDefault();
      // Holding Shift+B must not open and close the list again and again.
      if (!event.repeat) {
        this.closeBookmarks();
      }
    } else if (step && jumps.length > 0) {
      event.preventDefault();
      const next = index === -1 ? (step > 0 ? 0 : jumps.length - 1) : Math.min(Math.max(index + step, 0), jumps.length - 1);
      jumps[next].focus();
    } else if ((event.key === 'Enter' || event.key === ' ') && this.menuElement.contains(document.activeElement)) {
      // Pressed here, so the button acts once whether or not the browser would click it.
      event.preventDefault();
      document.activeElement.click();
    } else if ((event.key === 'Delete' || event.key === 'Backspace') && entry) {
      event.preventDefault();
      entry.querySelector('.close')?.click();
    }
  }
}

/** The ID of a tab's pane on `side`. */
function paneId(key, side) {
  return side === 'right' ? `${key}${RIGHT_PANE_SUFFIX}` : key;
}

/** The tab key of a pane ID. */
function paneTab(pane) {
  return paneSide(pane) === 'right' ? pane.slice(0, -RIGHT_PANE_SUFFIX.length) : pane;
}

function paneSide(pane) {
  return pane.endsWith(RIGHT_PANE_SUFFIX) ? 'right' : 'left';
}

function keyHints(hints) {
  return hints.map(([keys, action], index) => [
    index > 0 ? ' · ' : '',
    keys.map((name, position) => [position > 0 ? ' ' : '', h('kbd', { text: name })]),
    ` ${action}`,
  ]).flat(Infinity);
}

/** `kept` with the focus and selection of `view` kept for document `id`; without one, `kept` as it is. */
function keepFor(kept, id, view) {
  return id ? { ...kept, [id]: { focus: view.focus, selected: view.selected } } : kept;
}

/** The focus and selection kept for document `id`, or none. */
function keptFor(kept, id) {
  return (id && kept[id]) || { focus: null, selected: [] };
}

/** “Title” for one item, “3 items” for several. */
function described(rows) {
  return rows.length === 1 ? `“${rows[0].title}”` : `${rows.length} items`;
}

function empty(title, text, ...extra) {
  return h('div', { class: 'empty' }, h('h2', { text: title }), text ? h('p', { text }) : null, ...extra);
}

function fact(tone, ...content) {
  return h('div', { class: `fact ${tone}` }, h('span', { class: `dot ${tone === 'bad' ? 'disconnected' : tone}` }), h('span', {}, ...content));
}

function menuItem(label, action) {
  return h('button', { role: 'menuitem', text: label, onclick: action });
}

