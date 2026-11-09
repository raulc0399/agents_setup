// In-memory stand-in for the native shell with the same shape as ui/js/backend.js. It keeps
// the arrangement rules the UI relies on simple: tabs move, close and reopen; commands are
// recorded and answered by the test.

export function fakeBackend({ state, contents }, windowId = 'w1') {
  const listeners = [];
  const current = structuredClone(state);
  const emit = (event) => setTimeout(() => listeners.forEach((listener) => listener(event)), 0);
  const changed = () => emit({ type: 'state' });
  const windowOf = (key) => current.arrangement.windows.find((window) => window.tabs.includes(key));
  const removeTab = (key) => {
    const window = windowOf(key);
    if (!window) {
      return;
    }
    const index = window.tabs.indexOf(key);
    window.tabs.splice(index, 1);
    if (window.active === key) {
      window.active = window.tabs[index] ?? window.tabs.at(-1) ?? null;
    }
  };
  const addTab = (key, windowId) => {
    const window = current.arrangement.windows.find((candidate) => candidate.id === windowId);
    window.tabs.push(key);
    window.active = key;
  };

  return {
    windowId,
    sent: [],
    views: {},
    shown: [],
    state: async () => structuredClone(current),
    content: async (key) => (contents[key] ? structuredClone(contents[key]) : null),
    async send(key, command) {
      const session = current.sessions.find((candidate) => candidate.key === key);
      if (session?.link.state !== 'live') {
        throw 'the conversation is not connected; wait until its bridge reconnects';
      }
      const id = crypto.randomUUID();
      this.sent.push({ key, id, command });
      return id;
    },
    async activateTab(key) {
      windowOf(key).active = key;
      changed();
    },
    async closeTab(key) {
      removeTab(key);
      current.arrangement.closed.unshift(key);
      changed();
    },
    async reopenTab(key) {
      current.arrangement.closed = current.arrangement.closed.filter((closed) => closed !== key);
      removeTab(key);
      addTab(key, windowId);
      changed();
    },
    async moveTab(key, target) {
      removeTab(key);
      let id = target;
      if (!id) {
        current.arrangement.next_window += 1;
        id = `w${current.arrangement.next_window}`;
        current.arrangement.windows.push({ id, tabs: [], active: null });
      }
      addTab(key, id);
      changed();
    },
    async showWindow(id) {
      this.shown.push(id);
    },
    async setView(key, view) {
      this.views[key] = view;
    },
    onEvent(callback) {
      listeners.push(callback);
    },
    /** Test side: the bridge published; the host announces new content. */
    update(key, content) {
      contents[key] = content;
      emit({ type: 'content', key });
    },
    /** Test side: the bridge answers a command. */
    reply(key, id, error = null) {
      emit({ type: 'reply', key, id, error });
    },
  };
}
