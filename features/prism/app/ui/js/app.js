import { DiagramView } from './diagram.js';
import { h } from './dom.js';
import { agentLabel, projectName, shortConversation } from './format.js';
import { ACCEPT_NOTE, DROPPED_NOTE, ListView } from './list.js';
import { DetailPanel } from './panel.js';
import { documentsOf, hasDiagram, itemCount, itemIds, rows as buildRows } from './rows.js';

/** View state is saved this long after the last change, not on every key press. */
const VIEW_SAVE_DELAY_MS = 300;
/** After a tab was dropped outside every window, wait this long for another window to claim it. */
const DRAG_OUT_CHECK_DELAY_MS = 250;
const TAB_DRAG_PREFIX = 'prism-tab:';
/** A reply can arrive before `send_command` returned its ID; it waits this long to be matched. */
const EARLY_REPLY_KEEP_MS = 10000;
/**
 * Per tab, saved by the host. `modes`: list or diagram per document ID, chosen with V; none
 * shows the default for its content. `cameras`: the diagram's zoom and pan per document ID;
 * none fits it. `filter`: text the items must contain; `selected`: item IDs selected for one action.
 */
const DEFAULT_VIEW = { document: null, focus: null, hidden: [], filter: '', selected: [], modes: {}, cameras: {}, kept: {} };
/** What a document shows: its entry in `modes`, else the diagram when it has something to draw; V switches. */
const VIEWS = ['list', 'diagram'];
const VIEW_LABELS = { list: 'List', diagram: 'Diagram' };

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
    [['A'], 'accept', 2],
    [['X'], 'drop', 2],
    [['U'], 'undo', 2],
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
    [['A'], 'accept', 2],
    [['X'], 'drop', 2],
    [['U'], 'undo', 2],
    [['+', '-'], 'zoom', 3],
    [['0'], 'fit', 4],
    [['Esc'], 'clear', 4],
    [['H'], 'hide', 3],
    [['/'], 'filter', 3],
    [['S'], 'export', 4],
    [['W'], 'close', 4],
    [['V'], 'list', 3],
  ],
};
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
    this.panels = new Map();
    this.shownPanel = null;
    this.focusIndex = new Map();
    this.saveTimers = new Map();
    this.pending = new Map();
    this.earlyReplies = new Map();
    /** Tabs with an ask on its way: a new one waits for its reply, so a question is not shared twice. */
    this.asking = new Set();
    /**
     * The document this window closed last, `{key, id, revision}` with the revision its close
     * produced, until the next command: U reopens it.
     */
    this.lastClosed = null;
    /** Commands sent so far; a close only becomes the last one if nothing else went out after it. */
    this.commandCount = 0;
    this.message = null;
    this.sessionsOpen = false;
    this.menu = null;
    this.signatures = new Map();
    this.shownView = null;
    this.scrolls = new Map();

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
    // One input for the active tab's filter, kept across pane rebuilds so typing goes on.
    this.filterInput = h('input', {
      class: 'filter',
      type: 'search',
      placeholder: 'Filter',
      spellcheck: 'false',
      'aria-label': 'Filter the items of this tab',
      title: 'Show only items whose kind, title, text or ID contains this; only this tab (/)',
      oninput: () => this.updateView(this.activeKey(), { filter: this.filterInput.value }),
      onkeydown: (event) => {
        if (event.key === 'Escape' || event.key === 'Enter') {
          event.preventDefault();
          this.leaveFilter();
        }
      },
    });
    // Updated in place, so a new revision does not rebuild the controls around the keyboard.
    this.info = h('span', { class: 'info' });
    // The body: the tab's list or diagram (or why there is none), and its side pane.
    this.content = h('div', { class: 'content' });
    this.controls = h('div', { class: 'pane-controls' });
    this.pane = h('aside', { class: 'side-pane', 'aria-label': 'View and item details', hidden: true }, this.controls);
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
    this.forgetViews();
    this.render();
  }

  /** The list, diagram and detail panel of a tab that left this window are rebuilt if it comes back. */
  forgetViews() {
    const tabs = new Set(this.myWindow()?.tabs ?? []);
    for (const views of [this.listViews, this.panels]) {
      for (const key of [...views.keys()].filter((key) => !tabs.has(key))) {
        views.delete(key);
      }
    }
    for (const [key, view] of [...this.diagramViews].filter(([key]) => !tabs.has(key))) {
      view.dispose();
      this.diagramViews.delete(key);
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
        this.contents.set(key, content);
        this.leaveClosedDocument(key, shown);
        this.pruneSelection(key);
      }
    } finally {
      this.loading.delete(key);
    }
    this.render();
  }

  /**
   * The document shown is no longer open (closed here, by the agent or in another window): the
   * first open one takes over as if picked, keeping the focus and selection of the one left.
   */
  leaveClosedDocument(key, shown) {
    const view = this.view(key);
    const open = documentsOf(this.contents.get(key), false);
    if (!shown || open.some((document) => document.id === shown)) {
      return;
    }
    const next = open[0]?.id ?? null;
    const kept = { ...view.kept, [shown]: { focus: view.focus, selected: view.selected } };
    const back = (next && kept[next]) || { focus: null, selected: [] };
    this.storeView(key, { document: next, focus: back.focus, selected: back.selected, kept });
  }

  /** Selected items that left with their document leave the selection; dropped ones stay in the list and stay selected. */
  pruneSelection(key) {
    const current = this.current(key);
    const items = current ? itemIds(current.document) : new Set();
    const selected = this.view(key).selected;
    if (selected.some((id) => !items.has(id))) {
      this.storeView(key, { selected: selected.filter((id) => items.has(id)) });
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

  listView(key) {
    if (!this.listViews.has(key)) {
      this.listViews.set(
        key,
        new ListView({
          onFocus: (id) => this.focus(key, id),
          onToggle: (id) => this.toggleSelection(key, id),
        }),
      );
    }
    return this.listViews.get(key);
  }

  /** The detail area of the tab's side pane, in both views. */
  detailPanel(key) {
    if (!this.panels.has(key)) {
      this.panels.set(key, new DetailPanel(this.itemActions(key)));
    }
    return this.panels.get(key);
  }

  /** What the detail panel does with the focused or selected items. */
  itemActions(key) {
    return {
      onAsk: () => this.ask(key),
      onAccept: () => this.accept(key),
      onDrop: () => this.drop(key),
    };
  }

  diagramView(key) {
    if (!this.diagramViews.has(key)) {
      this.diagramViews.set(
        key,
        new DiagramView(
          {
            layout: (document, visible) => this.backend.diagram(document, visible),
            onFocus: (id) => this.focus(key, id),
            onToggle: (id) => this.toggleSelection(key, id),
            onClear: () => this.focus(key, null),
            // The overview in the detail panel counts the shapes of the new layout.
            onLayout: () => this.render(),
            onCamera: (documentId, camera) => {
              const cameras = { ...this.view(key).cameras };
              if (camera) {
                cameras[documentId] = camera;
              } else {
                delete cameras[documentId];
              }
              this.storeView(key, { cameras });
            },
          },
          this.detailPanel(key).element,
        ),
      );
    }
    return this.diagramViews.get(key);
  }

  /** The document, its visible rows, the focused and the selected rows of a tab; null before content. */
  current(key) {
    const content = this.contents.get(key);
    const documents = documentsOf(content, false);
    if (documents.length === 0) {
      return null;
    }
    const view = this.view(key);
    const document = documents.find((candidate) => candidate.id === view.document) ?? documents[0];
    const rows = buildRows(document, new Set(view.hidden), view.filter);
    let focusIndex = rows.findIndex((row) => row.id === view.focus);
    if (focusIndex === -1 && rows.length > 0) {
      // The focused item left (hidden, filtered out): its neighbour takes over.
      focusIndex = Math.min(this.focusIndex.get(key) ?? 0, rows.length - 1);
    }
    this.focusIndex.set(key, focusIndex);
    // Only what is shown counts: a selected item hidden or filtered out takes no part.
    const selected = new Set(view.selected);
    return { content, documents, document, rows, focus: rows[focusIndex] ?? null, selected: rows.filter((row) => selected.has(row.id)) };
  }

  /** What the tab's document shows: list, or diagram when it has something to draw. */
  mode(key) {
    const document = this.current(key)?.document;
    if (!document || !hasDiagram(document)) {
      return 'list';
    }
    return this.view(key).modes[document.id] ?? 'diagram';
  }

  /** V: the other view of the tab's document; a document with nothing to draw stays a list. */
  switchMode(key) {
    const current = this.current(key);
    if (!current) {
      return;
    }
    if (!hasDiagram(current.document)) {
      this.setMessage('Nothing to draw: this document has no arrows or groups.');
      return;
    }
    this.setMode(key, VIEWS[(VIEWS.indexOf(this.mode(key)) + 1) % VIEWS.length]);
  }

  setMode(key, mode) {
    const document = this.current(key).document;
    this.updateView(key, { modes: { ...this.view(key).modes, [document.id]: mode } });
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
      this.showBody(null);
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
    const current = this.current(key);
    const view = this.view(key);
    const mode = this.mode(key);
    const hidden = view.hidden.length;
    const closed = documentsOf(this.contents.get(key), true);
    this.patch(
      'header',
      this.header,
      [key, session, current?.documents.map((document) => [document.id, document.title]), current?.document.id, closed.length],
      () => this.tabHeader(key, session, state, current, closed),
    );

    this.pane.hidden = !current;
    if (!current) {
      this.showBody(null);
      this.patch('body', this.content, ['no-content', key, state, session?.loaded, this.contents.has(key), closed.length], () => [
        this.emptyTab(key, session, state, closed),
      ]);
      return;
    }
    this.renderControls(key, current, view, mode);
    if (current.rows.length === 0) {
      this.showPanel(null);
      this.showBody(null);
      const total = itemCount(current.document);
      const filter = view.filter.trim();
      this.patch('body', this.content, ['no-rows', key, current.document.id, total, hidden, filter], () => [
        total === 0
          ? empty('This document has no items', null)
          : filter
            ? empty(
                `No items match “${filter}”`,
                hidden > 0 ? `Items hidden in this tab are not searched; Shift+H shows them.` : null,
                h('button', { class: 'button', text: 'Clear filter', onclick: () => this.updateView(key, { filter: '' }) }),
              )
            : empty(
                `All ${total} items are hidden in this tab`,
                null,
                h('button', { class: 'button', text: 'Show hidden', onclick: () => this.updateView(key, { hidden: [] }) }),
              ),
      ]);
      return;
    }
    const selected = new Set(current.selected.map((row) => row.id));
    const panel = this.detailPanel(key);
    this.showPanel(panel);
    const details = {
      document: current.document,
      rows: current.rows,
      selected,
      questions: current.content.questions,
      readOnly: READ_ONLY_REASONS[state] ?? null,
    };

    if (mode === 'diagram') {
      const diagramView = this.diagramView(key);
      this.showBody(diagramView, key);
      // Only an item the user picked: without one the panel shows the overview.
      diagramView.render({ ...details, focus: view.focus, camera: view.cameras?.[current.document.id] ?? null });
      panel.render({ ...details, focus: view.focus, counts: diagramView.counts() });
      return;
    }
    const listView = this.listView(key);
    this.showBody(listView, key);
    const focus = current.focus?.id ?? null;
    listView.render({ ...details, focus });
    panel.render({ ...details, focus, counts: null });
  }

  /**
   * The controls block at the top of the side pane: the view switch when the document has
   * something to draw, the filter, the counts with their actions, and the export.
   */
  renderControls(key, current, view, mode) {
    const filtering = document.activeElement === this.filterInput;
    const caret = filtering ? [this.filterInput.selectionStart, this.filterInput.selectionEnd] : null;
    // A rebuild must not take the keyboard away from a focused button: its new copy takes it.
    const control = this.controls.contains(document.activeElement) ? document.activeElement.dataset.control : null;
    this.patch(
      'controls',
      this.controls,
      [key, current.document.id, view.hidden.length, current.selected.length, view.selected.length, mode, hasDiagram(current.document)],
      () => this.paneControls(key, current, view, mode),
    );
    if (filtering && document.activeElement !== this.filterInput) {
      this.filterInput.focus();
      this.filterInput.setSelectionRange(...caret);
    }
    if (control && !this.controls.contains(document.activeElement)) {
      this.controls.querySelector(`[data-control="${control}"]`)?.focus();
    }
    this.info.textContent = `${current.rows.length} ${current.rows.length === 1 ? 'item' : 'items'} · revision ${current.document.revision}`;
    // While typing the two agree; they differ after a tab switch or Clear filter.
    if (this.filterInput.value !== view.filter) {
      this.filterInput.value = view.filter;
    }
    this.filterInput.classList.toggle('active', view.filter.trim() !== '');
  }

  /** Below the controls, the side pane shows the tab's detail panel while there are items. */
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

  /** Shows a tab's list or diagram in the body, keeping each tab's list scroll position across switches. */
  showBody(view, key) {
    if (this.shownView?.view === view) {
      return;
    }
    if (this.shownView?.view instanceof ListView) {
      this.scrolls.set(this.shownView.key, this.shownView.view.element.scrollTop);
    }
    this.signatures.delete('body');
    this.shownView = view ? { view, key } : null;
    if (view) {
      this.content.replaceChildren(view.element);
    }
    if (view instanceof ListView) {
      view.element.scrollTop = this.scrolls.get(key) ?? 0;
    }
  }

  tabHeader(key, session, state, current, closed) {
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
                current.documents.map((document) => this.documentTab(key, document, document.id === current.document.id)),
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
        ),
      );
    }
    return parts;
  }

  /** A document in the strip, with its close control (W closes the one shown). */
  documentTab(key, document, selected) {
    return h(
      'div',
      { class: selected ? 'document selected' : 'document', role: 'presentation' },
      h('button', {
        role: 'tab',
        'aria-selected': String(selected),
        title: document.title,
        text: document.title,
        onclick: () => this.showDocument(key, document.id),
      }),
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
  paneControls(key, current, view, mode) {
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
                onclick: () => this.setMode(key, name),
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
                onclick: () => this.exportDiagram(key),
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
              onclick: () => this.updateView(key, { hidden: [] }),
            })
          : null,
        view.selected.length > 0
          ? h('button', {
              class: 'button quiet selection',
              'data-control': 'clear-selection',
              title: filteredOut > 0 ? 'Actions leave out selected items that are hidden or filtered out. Clear the selection (Esc)' : 'Clear the selection (Esc)',
              text: `${current.selected.length} selected${filteredOut > 0 ? ` (${filteredOut} not shown)` : ''} · clear`,
              onclick: () => this.clearSelection(key),
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
    if (state === 'ended') {
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
      return empty(
        'Waiting for this conversation',
        'The tab was open when Prism last quit. Its content appears when the conversation’s bridge connects.',
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
    this.keys.hidden = !key;
    const mode = key ? this.mode(key) : 'list';
    const document = key ? this.current(key)?.document : null;
    this.statusMessage.textContent = this.message?.text ?? '';
    this.statusMessage.title = this.message?.text ?? '';
    this.statusMessage.className = `message ${this.message?.tone ?? ''}`;
    this.fitKeys(mode, Boolean(document && hasDiagram(document)));
  }

  /**
   * Shows the key hints that fit beside the message, leaving out the highest ranks first; V
   * only when the document has a diagram to switch to.
   */
  fitKeys(mode, switchable) {
    const signature = JSON.stringify([mode, switchable, this.keys.hidden, this.message?.text ?? '', this.statusbar.clientWidth]);
    if (this.keys.hidden || signature === this.keysSignature) {
      return;
    }
    this.keysSignature = signature;
    let hints = (KEY_HINTS[mode] ?? KEY_HINTS.list).filter(([keys]) => switchable || keys[0] !== 'V');
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
    if (!this.menu) {
      return;
    }
    const { key, x, y, closed } = this.menu;
    const items = closed ? this.closedMenuItems(key) : this.tabMenuItems(key);
    if (items.length === 0) {
      // The last closed document was reopened meanwhile.
      this.closeMenu();
      return;
    }
    this.menuElement.classList.toggle('closed-menu', Boolean(closed));
    for (const item of items) {
      item.addEventListener('click', () => this.closeMenu());
    }
    this.menuElement.replaceChildren(...items);
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

  /** The closed documents of the tab, as the store has them; picking one reopens it for everyone. */
  closedMenuItems(key) {
    return documentsOf(this.contents.get(key), true).map((document) => {
      const item = menuItem(document.title, () => this.reopenDocument(key, document));
      item.title = `Reopen “${document.title}” for everyone`;
      return item;
    });
  }

  windowProjects(window) {
    const names = [...new Set(window.tabs.map((key) => this.session(key)).filter(Boolean).map((session) => projectName(session.project)))];
    return names.length > 0 ? ` · ${names.join(', ')}` : '';
  }

  setMessage(text, tone = '') {
    this.message = { text, tone };
    this.renderStatus();
  }

  // Tabs and windows

  activate(key) {
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

  openMenu(key, x, y) {
    this.menu = { key, x, y };
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

  closeMenu() {
    this.menu = null;
    this.renderMenu();
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

  focus(key, id) {
    if (this.view(key).focus !== id) {
      this.updateView(key, { focus: id });
    }
  }

  moveFocus(key, step) {
    const current = this.current(key);
    if (!current || current.rows.length === 0) {
      return;
    }
    const index = current.rows.findIndex((row) => row.id === current.focus?.id);
    const next = step === Infinity ? current.rows.length - 1 : step === -Infinity ? 0 : Math.min(Math.max(index + step, 0), current.rows.length - 1);
    this.focus(key, current.rows[next].id);
    this.listView(key).reveal(current.rows[next].id);
  }

  cycleDocuments(key, step) {
    const current = this.current(key);
    if (!current || current.documents.length < 2) {
      return;
    }
    const index = current.documents.findIndex((document) => document.id === current.document.id);
    const next = current.documents[(index + step + current.documents.length) % current.documents.length];
    this.showDocument(key, next.id);
  }

  /**
   * Another document of the tab: the focus and selection of the document left are kept for
   * it, and those kept for the one shown come back.
   */
  showDocument(key, id) {
    // U acts on the document shown from now on.
    this.lastClosed = null;
    const view = this.view(key);
    const left = this.current(key)?.document.id;
    const kept = left ? { ...view.kept, [left]: { focus: view.focus, selected: view.selected } } : view.kept;
    const back = kept[id] ?? { focus: null, selected: [] };
    this.updateView(key, { document: id, focus: back.focus, selected: back.selected, kept });
  }

  focusFilter() {
    this.filterInput.focus();
    this.filterInput.select();
  }

  /** Enter or Esc in the filter: the keyboard goes back to the items, the filter stays. */
  leaveFilter() {
    const key = this.activeKey();
    if (this.mode(key) === 'diagram') {
      this.diagramView(key).restoreKeyboard();
    } else {
      this.listView(key).element.focus();
    }
  }

  /** What an action acts on: the selected items shown, else the focused one (in the diagram, only one the user picked). */
  targets(key) {
    const current = this.current(key);
    if (!current) {
      return [];
    }
    if (current.selected.length > 0) {
      return current.selected;
    }
    const focus = this.mode(key) === 'diagram' ? this.diagramFocus(key) : current.focus;
    return focus ? [focus] : [];
  }

  /** Space, or a click with ⌘/Ctrl: the item joins or leaves the selection and takes the focus. */
  toggleSelection(key, id) {
    const selected = this.view(key).selected;
    this.updateView(key, { focus: id, selected: selected.includes(id) ? selected.filter((other) => other !== id) : [...selected, id] });
  }

  /** Shift+A: every item shown is selected, as in the terminal list. */
  selectAll(key) {
    const rows = this.current(key)?.rows ?? [];
    this.updateView(key, { selected: rows.map((row) => row.id) });
  }

  clearSelection(key) {
    if (this.view(key).selected.length > 0) {
      this.updateView(key, { selected: [] });
    }
  }

  /** Removes items from the selection, for example once an action on them committed. */
  deselect(key, ids) {
    const selected = this.view(key).selected;
    if (selected.some((id) => ids.includes(id))) {
      this.updateView(key, { selected: selected.filter((id) => !ids.includes(id)) });
    }
  }

  /** The item picked in the diagram, if it is still shown. */
  diagramFocus(key) {
    return this.current(key)?.rows.find((row) => row.id === this.view(key).focus) ?? null;
  }

  /** J and K in the diagram follow the layout's order: boxes with their badges, then arrows (lanes are left out). */
  moveDiagramFocus(key, step) {
    const order = this.diagramView(key).diagram?.order ?? [];
    if (order.length === 0) {
      return;
    }
    const index = order.indexOf(this.view(key).focus);
    let next;
    if (step === Infinity || step === -Infinity) {
      next = step > 0 ? order.length - 1 : 0;
    } else if (index === -1) {
      next = step > 0 ? 0 : order.length - 1;
    } else {
      next = Math.min(Math.max(index + step, 0), order.length - 1);
    }
    this.focus(key, order[next]);
  }

  clearDiagramFocus(key) {
    this.focus(key, null);
    this.diagramView(key).focusCanvas();
  }

  async exportDiagram(key) {
    const current = this.current(key);
    const svg = current ? this.diagramView(key).exportSvg() : null;
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
  pickedTargets(key) {
    const targets = this.targets(key);
    if (targets.length === 0) {
      this.setMessage('Pick an item first: click it, or move to it with J and K.', 'error');
    }
    return targets;
  }

  /** The targets of Ask, Accept and Drop: dropped items take no part; with only dropped ones picked, none and why. */
  sharedTargets(key) {
    const targets = this.pickedTargets(key);
    const active = targets.filter((row) => !row.dropped);
    if (targets.length > 0 && active.length === 0) {
      this.setMessage(DROPPED_NOTE, 'error');
    }
    return active;
  }

  /**
   * Shares the focused or selected items with the CLI prompt box right away, where the user
   * types the question. Text selected inside the focused item's text in the panel becomes its
   * quote; every other item is quoted in full.
   */
  async ask(key) {
    if (!this.isLive(key)) {
      this.setMessage('Not connected: asking needs a live conversation.', 'error');
      return;
    }
    if (this.asking.has(key)) {
      return;
    }
    const targets = this.sharedTargets(key);
    if (targets.length === 0) {
      return;
    }
    const current = this.current(key);
    const focus = this.mode(key) === 'diagram' ? this.diagramFocus(key) : current.focus;
    const quoted = focus ? this.detailPanel(key).selectedText(focus.id) : '';
    const items = targets.map((row) => ({ number: row.number, title: row.title, quote: row.id === focus?.id && quoted.trim() ? quoted : row.details }));
    const release = () => this.asking.delete(key);
    this.asking.add(key);
    const sent = await this.send(
      key,
      {
        type: 'ask',
        selection: { document_id: current.document.id, revision: current.document.revision, ids: targets.map((row) => row.id) },
        items,
      },
      {
        sending: 'Sharing…',
        done: `Shared ${items.length === 1 ? 'item' : 'items'} ${items.map((item) => item.number).join(', ')} with the CLI prompt box.`,
        onSuccess: release,
        onError: release,
      },
    );
    if (!sent) {
      release();
    }
  }

  hide(key) {
    const targets = this.pickedTargets(key);
    if (targets.length === 0) {
      return;
    }
    const ids = targets.map((row) => row.id);
    const view = this.view(key);
    this.updateView(key, { hidden: [...view.hidden, ...ids], selected: view.selected.filter((id) => !ids.includes(id)) });
    this.setMessage(`Hid ${described(targets)} in this tab. Shift+H shows hidden items.`);
  }

  drop(key) {
    const targets = this.isLive(key) ? this.sharedTargets(key) : [];
    return this.applyAction(key, 'archive', targets, {
      sending: 'Dropping…',
      done: `Dropped ${described(targets)}. U undoes.`,
    });
  }

  accept(key) {
    const targets = this.isLive(key) ? this.sharedTargets(key) : [];
    return this.applyAction(key, 'accept', targets, {
      sending: 'Accepting…',
      done: `Accepted ${described(targets)}. ${ACCEPT_NOTE}`,
    });
  }

  /** One `action` for all targets at the document's revision; once it committed, they leave the selection. */
  async applyAction(key, action, targets, { sending, done }) {
    const current = this.current(key);
    if (!current || !this.requireLive(key) || targets.length === 0) {
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
      { sending, done, onSuccess: () => this.deselect(key, ids) },
    );
  }

  /** W or ×: closes the document in every window and for the agent; right after, U reopens it. */
  async closeDocument(key, document) {
    if (!document || !this.requireLive(key)) {
      return;
    }
    // `send` counts this command first; another one after it ends the moment for U.
    const count = this.commandCount + 1;
    await this.send(
      key,
      { type: 'action', document_id: document.id, expected_revision: document.revision, action: { action: 'close' } },
      {
        sending: 'Closing…',
        done: `Closed “${document.title}” for everyone. U reopens it.`,
        onSuccess: () => {
          if (this.commandCount === count) {
            this.lastClosed = { key, id: document.id, revision: document.revision + 1 };
          }
        },
      },
    );
  }

  /** From Closed (N): reopens the document for everyone and shows it. */
  async reopenDocument(key, document) {
    if (!this.requireLive(key)) {
      return;
    }
    await this.send(
      key,
      { type: 'action', document_id: document.id, expected_revision: document.revision, action: { action: 'reopen' } },
      { sending: 'Reopening…', done: `Reopened “${document.title}”.`, onSuccess: () => this.showDocument(key, document.id) },
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
   * Reverts the latest undo record of the document, whatever made it (drop, accept, close, a
   * tool); right after a close, that of the closed document, which reopens it.
   */
  async undo(key) {
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
      await this.send(
        key,
        { type: 'undo', document_id: closed.id, expected_revision: closed.revision },
        { sending: 'Reopening…', done: `Reopened “${closed.title}”.`, errors, onSuccess: () => this.showDocument(key, closed.id) },
      );
      return;
    }
    const current = this.current(key);
    if (!current || !this.requireLive(key)) {
      return;
    }
    await this.send(
      key,
      { type: 'undo', document_id: current.document.id, expected_revision: current.document.revision },
      { sending: 'Undoing…', done: 'Undid the latest action on this document.', errors },
    );
  }

  /**
   * Sends a command and shows `sending`; its reply later runs `onSuccess` and shows `done`, or
   * runs `onError` and shows the error, in the words of `errors[code]` when given.
   */
  async send(key, command, { sending, done, errors = {}, onSuccess = null, onError = null }) {
    // Any command ends the moment right after a close.
    this.commandCount += 1;
    this.lastClosed = null;
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
      pending.onSuccess?.();
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
    const modifier = event.metaKey || event.ctrlKey;
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
    // Holding Enter must not ask again and again.
    if (event.key === 'Enter' && event.repeat) {
      event.preventDefault();
      return;
    }
    const focus = () => this.current(key)?.focus;
    const actions = {
      j: () => this.moveFocus(key, 1),
      ArrowDown: () => this.moveFocus(key, 1),
      k: () => this.moveFocus(key, -1),
      ArrowUp: () => this.moveFocus(key, -1),
      Home: () => this.moveFocus(key, -Infinity),
      End: () => this.moveFocus(key, Infinity),
      ' ': () => focus() && this.toggleSelection(key, focus().id),
      A: () => this.selectAll(key),
      Escape: () => this.clearSelection(key),
      Enter: () => this.ask(key),
      a: () => this.accept(key),
      h: () => this.hide(key),
      H: () => this.updateView(key, { hidden: [] }),
      x: () => this.drop(key),
      u: () => this.undo(key),
      '/': () => this.focusFilter(),
      '[': () => this.cycleDocuments(key, -1),
      ']': () => this.cycleDocuments(key, 1),
      w: () => this.closeDocument(key, this.current(key)?.document),
      v: () => this.switchMode(key),
    };
    const diagram = this.mode(key) === 'diagram' ? this.diagramKeys(key) : {};
    const action = event.key in diagram ? diagram[event.key] : actions[event.key];
    if (action) {
      event.preventDefault();
      action();
    }
  }

  /** Keys of the diagram, over those of the list. */
  diagramKeys(key) {
    const diagram = this.diagramView(key);
    const focused = () => this.diagramFocus(key);
    const pan = (dx, dy) => () => diagram.panStep(dx, dy);
    return {
      j: () => this.moveDiagramFocus(key, 1),
      k: () => this.moveDiagramFocus(key, -1),
      Home: () => this.moveDiagramFocus(key, -Infinity),
      End: () => this.moveDiagramFocus(key, Infinity),
      ArrowLeft: pan(-1, 0),
      ArrowRight: pan(1, 0),
      ArrowUp: pan(0, -1),
      ArrowDown: pan(0, 1),
      '+': () => diagram.zoomStep(1),
      '=': () => diagram.zoomStep(1),
      '-': () => diagram.zoomStep(-1),
      0: () => diagram.fit(),
      ' ': () => focused() && this.toggleSelection(key, focused().id),
      // A selection clears first, then the focus.
      Escape: () => (this.view(key).selected.length > 0 ? this.clearSelection(key) : this.clearDiagramFocus(key)),
      s: () => this.exportDiagram(key),
    };
  }

  onMenuKey(event) {
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
}

function keyHints(hints) {
  return hints.map(([keys, action], index) => [
    index > 0 ? ' · ' : '',
    keys.map((name, position) => [position > 0 ? ' ' : '', h('kbd', { text: name })]),
    ` ${action}`,
  ]).flat(Infinity);
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

