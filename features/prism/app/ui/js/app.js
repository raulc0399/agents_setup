import { DiagramView } from './diagram.js';
import { h } from './dom.js';
import { agentLabel, firstLine, projectName, shortConversation } from './format.js';
import { ListView } from './list.js';
import { itemCount, rows as buildRows, sortedDocuments } from './rows.js';

/** View state is saved this long after the last change, not on every key press. */
const VIEW_SAVE_DELAY_MS = 300;
/** After a tab was dropped outside every window, wait this long for another window to claim it. */
const DRAG_OUT_CHECK_DELAY_MS = 250;
const TAB_DRAG_PREFIX = 'prism-tab:';
/** Per tab, saved by the host. `cameras`: the diagram's zoom and pan per document ID; none fits it. */
const DEFAULT_VIEW = { document: null, focus: null, hidden: [], mode: 'list', cameras: {} };
/** What a tab shows: `mode` in its view state; V switches. */
const VIEWS = ['list', 'diagram'];
const VIEW_LABELS = { list: 'List', diagram: 'Diagram' };

const LINK_LABELS = { live: 'Live', disconnected: 'Disconnected', ended: 'Ended', waiting: 'Waiting for bridge' };
const READ_ONLY_REASONS = {
  disconnected: 'Read-only until the bridge reconnects',
  waiting: 'Read-only until the bridge connects',
  ended: 'Read-only: this conversation ended',
};
const KEY_HINTS = {
  list: [
    [['J', 'K'], 'move'],
    [['Enter'], 'ask'],
    [['H'], 'hide'],
    [['X'], 'drop'],
    [['U'], 'undo'],
    [['[', ']'], 'documents'],
    [['V'], 'diagram'],
  ],
  diagram: [
    [['J', 'K'], 'move'],
    [['+', '-'], 'zoom'],
    [['0'], 'fit'],
    [['Esc'], 'clear'],
    [['H'], 'hide'],
    [['S'], 'export'],
    [['V'], 'list'],
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
    this.focusIndex = new Map();
    this.saveTimers = new Map();
    this.pending = new Map();
    this.ask = null;
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
    this.body = h('div', { class: 'body' });
    this.popover = h('div', { class: 'popover', role: 'dialog', 'aria-label': 'Sessions', hidden: true });
    this.main = h('main', { class: 'main' }, this.header, this.body, this.popover);
    this.statusMessage = h('div', { class: 'message', role: 'status' });
    this.keys = h('div', { class: 'keys' });
    this.statusbar = h('footer', { class: 'statusbar' }, this.statusMessage, this.keys);
    this.menuElement = h('div', { class: 'menu', role: 'menu', hidden: true });
    this.root.replaceChildren(this.tabstrip, this.main, this.statusbar, this.menuElement);
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

  /** The list and diagram of a tab that left this window are rebuilt if it comes back. */
  forgetViews() {
    const tabs = new Set(this.myWindow()?.tabs ?? []);
    for (const key of [...this.listViews.keys()].filter((key) => !tabs.has(key))) {
      this.listViews.delete(key);
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
        this.contents.set(key, content);
      }
    } finally {
      this.loading.delete(key);
    }
    this.render();
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
      this.views.set(key, { ...DEFAULT_VIEW, ...(this.state.arrangement.views[key] ?? {}) });
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
          onAsk: (row) => this.openAsk(key, row),
          onHide: (row) => this.hide(key, row),
          onDrop: (row) => this.drop(key, row),
          onSend: (row, question) => this.sendAsk(key, row, question),
          onCancelAsk: () => this.cancelAsk(),
          onDraft: (text) => {
            if (this.ask) {
              this.ask.text = text;
            }
          },
        }),
      );
    }
    return this.listViews.get(key);
  }

  diagramView(key) {
    if (!this.diagramViews.has(key)) {
      this.diagramViews.set(
        key,
        new DiagramView({
          layout: (document, visible) => this.backend.diagram(document, visible),
          onFocus: (id) => this.focus(key, id),
          onClear: () => this.focus(key, null),
          onCamera: (documentId, camera) => {
            const cameras = { ...this.view(key).cameras };
            if (camera) {
              cameras[documentId] = camera;
            } else {
              delete cameras[documentId];
            }
            this.storeView(key, { cameras });
          },
        }),
      );
    }
    return this.diagramViews.get(key);
  }

  /** The document, its visible rows and the focused row of a tab; null before content. */
  current(key) {
    const content = this.contents.get(key);
    if (!content || content.documents.length === 0) {
      return null;
    }
    const view = this.view(key);
    const documents = sortedDocuments(content.documents);
    const document = documents.find((candidate) => candidate.id === view.document) ?? documents[0];
    const rows = buildRows(document, new Set(view.hidden));
    let focusIndex = rows.findIndex((row) => row.id === view.focus);
    if (focusIndex === -1 && rows.length > 0) {
      // The focused item left (dropped, hidden): its neighbour takes over.
      focusIndex = Math.min(this.focusIndex.get(key) ?? 0, rows.length - 1);
    }
    this.focusIndex.set(key, focusIndex);
    return { content, documents, document, rows, focus: rows[focusIndex] ?? null };
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
    this.patch('sessions-button', this.sessionsButton, [live, this.state.sessions.length], () => [
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
      this.showBody(null);
      this.patch('body', this.body, ['empty', this.state.status, this.state.sessions, this.myWindow()?.id], () => [
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
    const hidden = view.hidden.length;
    this.patch(
      'header',
      this.header,
      [key, session, current?.documents.map((document) => [document.id, document.title]), current?.document.id, current?.rows.length, hidden, view.mode],
      () => this.tabHeader(key, session, state, current, hidden, view.mode),
    );

    if (!current) {
      this.showBody(null);
      this.patch('body', this.body, ['no-content', key, state, session?.loaded, this.contents.has(key)], () => [
        this.emptyTab(key, session, state),
      ]);
      return;
    }
    if (current.rows.length === 0) {
      this.showBody(null);
      const total = itemCount(current.document);
      this.patch('body', this.body, ['no-rows', key, current.document.id, total, hidden], () => [
        total === 0
          ? empty('This document has no items', 'Dropped items come back with U (undo).')
          : empty(
              `All ${total} items are hidden in this tab`,
              null,
              h('button', { class: 'button', text: 'Show hidden', onclick: () => this.updateView(key, { hidden: [] }) }),
            ),
      ]);
      return;
    }

    if (view.mode === 'diagram') {
      const diagramView = this.diagramView(key);
      this.showBody(diagramView, key);
      diagramView.render({
        document: current.document,
        rows: current.rows,
        // Only an item the user picked: without one the panel shows the overview.
        focus: view.focus,
        questions: current.content.questions,
        camera: view.cameras?.[current.document.id] ?? null,
      });
      return;
    }
    const listView = this.listView(key);
    this.showBody(listView, key);
    listView.render({
      document: current.document,
      rows: current.rows,
      focus: current.focus?.id ?? null,
      questions: current.content.questions,
      ask: this.ask?.key === key ? this.ask : null,
      readOnly: READ_ONLY_REASONS[state] ?? null,
    });
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
      this.body.replaceChildren(view.element);
    }
    if (view instanceof ListView) {
      view.element.scrollTop = this.scrolls.get(key) ?? 0;
    }
  }

  tabHeader(key, session, state, current, hidden, mode) {
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
    if (current) {
      parts.push(
        h(
          'div',
          { class: 'toolbar' },
          h(
            'div',
            { class: 'documents', role: 'tablist', 'aria-label': 'Documents' },
            current.documents.map((document) =>
              h('button', {
                role: 'tab',
                'aria-selected': String(document.id === current.document.id),
                title: document.title,
                text: document.title,
                onclick: () => this.updateView(key, { document: document.id, focus: null }),
              }),
            ),
          ),
          h(
            'div',
            { class: 'segmented', role: 'group', 'aria-label': 'View' },
            VIEWS.map((name) =>
              h('button', {
                'aria-pressed': String(name === mode),
                title: `Show as ${VIEW_LABELS[name].toLowerCase()} (V)`,
                text: VIEW_LABELS[name],
                onclick: () => this.updateView(key, { mode: name }),
              }),
            ),
          ),
          h('span', { class: 'spacer' }),
          h('span', {
            class: 'info',
            text: `${current.rows.length} ${current.rows.length === 1 ? 'item' : 'items'} · revision ${current.document.revision}`,
          }),
          hidden > 0
            ? h('button', {
                class: 'button quiet',
                title: 'Show the items hidden in this tab (Shift+H)',
                text: `${hidden} hidden · show`,
                onclick: () => this.updateView(key, { hidden: [] }),
              })
            : null,
          mode === 'diagram'
            ? h('button', {
                class: 'button quiet',
                title: 'Save the diagram as an SVG file (S)',
                text: 'Export SVG',
                onclick: () => this.exportDiagram(key),
              })
            : null,
        ),
      );
    }
    return parts;
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

  emptyTab(key, session, state) {
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
    return empty('No tabs in this window', 'Open a conversation here:', this.sessionPicker());
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
    const mode = key ? this.view(key).mode : 'list';
    this.patch('keys', this.keys, [mode], () => keyHints(KEY_HINTS[mode] ?? KEY_HINTS.list));
    this.statusMessage.textContent = this.message?.text ?? '';
    this.statusMessage.className = `message ${this.message?.tone ?? ''}`;
  }

  renderMenu() {
    this.menuElement.hidden = !this.menu;
    if (!this.menu) {
      return;
    }
    const { key, x, y } = this.menu;
    const windows = this.state.arrangement.windows;
    const items = [
      menuItem('Move to new window', () => this.backend.moveTab(key, null)),
      ...windows
        .map((window, index) => [window, index])
        .filter(([window]) => window.id !== this.backend.windowId)
        .map(([window, index]) => menuItem(`Move to window ${index + 1}${this.windowProjects(window)}`, () => this.backend.moveTab(key, window.id))),
      h('hr'),
      menuItem('Close tab', () => this.backend.closeTab(key)),
    ];
    for (const item of items) {
      item.addEventListener('click', () => this.closeMenu());
    }
    this.menuElement.replaceChildren(...items);
    const width = this.menuElement.offsetWidth;
    const height = this.menuElement.offsetHeight;
    this.menuElement.style.setProperty('left', `${Math.min(x, window.innerWidth - width - 4)}px`);
    this.menuElement.style.setProperty('top', `${Math.min(y, window.innerHeight - height - 4)}px`);
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
    this.ask = this.ask?.key === key ? this.ask : null;
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
    this.updateView(key, { document: next.id, focus: null });
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

  openAsk(key, row) {
    if (!this.isLive(key)) {
      this.setMessage('Not connected: asking needs a live conversation.', 'error');
      return;
    }
    const selected = this.listView(key).selectedText(row.id);
    const quote = selected.trim() ? selected : row.details;
    this.ask = {
      key,
      id: row.id,
      text: this.ask?.id === row.id ? this.ask.text : '',
      quote,
      quoteLabel: selected.trim()
        ? `Quoting your selection: “${firstLine(selected)}”`
        : `Quoting item ${row.number}, ${row.title}, in full`,
    };
    this.render();
    this.listView(key).focusEditor();
  }

  cancelAsk() {
    const key = this.ask?.key;
    this.ask = null;
    this.render();
    if (key) {
      this.listView(key).element.focus();
    }
  }

  async sendAsk(key, row, question) {
    const current = this.current(key);
    if (!current || !this.ask) {
      return;
    }
    const sent = await this.send(
      key,
      {
        type: 'ask',
        selection: { document_id: current.document.id, revision: current.document.revision, ids: [row.id] },
        number: row.number,
        title: row.title,
        quote: this.ask.quote,
        question,
      },
      `Shared item ${row.number} with the CLI prompt box.`,
    );
    if (sent) {
      this.setMessage('Sharing…');
      this.cancelAsk();
    }
  }

  hide(key, row) {
    this.updateView(key, { hidden: [...this.view(key).hidden, row.id] });
    this.setMessage(`Hid “${row.title}” in this tab. Shift+H shows hidden items.`);
  }

  async drop(key, row) {
    const current = this.current(key);
    if (!current) {
      return;
    }
    const sent = await this.send(
      key,
      {
        type: 'action',
        document_id: current.document.id,
        expected_revision: current.document.revision,
        action: { action: 'archive', ids: [row.id] },
      },
      `Dropped “${row.title}”. U undoes.`,
    );
    if (sent) {
      this.setMessage('Dropping…');
    }
  }

  async undo(key) {
    const current = this.current(key);
    if (!current) {
      return;
    }
    const sent = await this.send(
      key,
      { type: 'undo', document_id: current.document.id, expected_revision: current.document.revision },
      'Undone.',
    );
    if (sent) {
      this.setMessage('Undoing…');
    }
  }

  /** Sends a command; its reply later shows `done` or the error. */
  async send(key, command, done) {
    try {
      const id = await this.backend.send(key, command);
      this.pending.set(id, { key, done });
      return true;
    } catch (error) {
      this.setMessage(String(error), 'error');
      return false;
    }
  }

  onReply({ id, error }) {
    const pending = this.pending.get(id);
    if (!pending) {
      return;
    }
    this.pending.delete(id);
    if (error) {
      this.setMessage(errorText(error), 'error');
    } else {
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
    if (event.defaultPrevented || event.target.closest?.('textarea, input')) {
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

    const key = this.activeKey();
    if (!key) {
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
      Enter: () => focus() && this.openAsk(key, focus()),
      h: () => focus() && this.hide(key, focus()),
      H: () => this.updateView(key, { hidden: [] }),
      x: () => focus() && this.drop(key, focus()),
      u: () => this.undo(key),
      '[': () => this.cycleDocuments(key, -1),
      ']': () => this.cycleDocuments(key, 1),
      v: () => this.updateView(key, { mode: VIEWS[(VIEWS.indexOf(this.view(key).mode) + 1) % VIEWS.length] }),
    };
    const diagram = this.view(key).mode === 'diagram' ? this.diagramKeys(key, event.target) : {};
    const action = event.key in diagram ? diagram[event.key] : actions[event.key];
    if (action) {
      event.preventDefault();
      action();
    }
  }

  /**
   * Keys of the diagram, over those of the list; `null` turns a list key off. Arrow keys in
   * the side panel scroll it instead of panning.
   */
  diagramKeys(key, target) {
    const diagram = this.diagramView(key);
    const focused = () => this.diagramFocus(key);
    const inPanel = Boolean(target.closest?.('.diagram-panel'));
    const pan = (dx, dy) => (inPanel ? null : () => diagram.panStep(dx, dy));
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
      Escape: () => this.clearDiagramFocus(key),
      h: () => focused() && this.hide(key, focused()),
      x: () => focused() && this.drop(key, focused()),
      s: () => this.exportDiagram(key),
      // Enter acts on the focused shape; the list's ask editor is not shown here.
      Enter: null,
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

function empty(title, text, ...extra) {
  return h('div', { class: 'empty' }, h('h2', { text: title }), text ? h('p', { text }) : null, ...extra);
}

function fact(tone, ...content) {
  return h('div', { class: `fact ${tone}` }, h('span', { class: `dot ${tone === 'bad' ? 'disconnected' : tone}` }), h('span', {}, ...content));
}

function menuItem(label, action) {
  return h('button', { role: 'menuitem', text: label, onclick: action });
}

