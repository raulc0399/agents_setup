/**
 * The only module that talks to the native shell. The UI receives a backend object, so a
 * test harness can drive it in a plain browser with a fake one of the same shape:
 *
 *   windowId                      label of this native window
 *   state()                       -> AppState {status, sessions, arrangement}
 *   content(key)                  -> {documents, questions} or null
 *   send(key, command)            -> command ID; rejects with a message
 *   activateTab(key), closeTab(key), reopenTab(key), moveTab(key, target | null),
 *   showWindow(windowId), setView(key, view)
 *   onEvent(callback)             host events: {type: "state"}, {type: "content", key},
 *                                 {type: "reply", key, id, error}
 */
const HOST_EVENT = 'prism:event';

export function tauriBackend() {
  const { core, event, webviewWindow } = window.__TAURI__;
  const invoke = core.invoke;
  return {
    windowId: webviewWindow.getCurrentWebviewWindow().label,
    state: () => invoke('app_state'),
    content: (key) => invoke('session_content', { key }),
    send: (key, command) => invoke('send_command', { key, command }),
    activateTab: (key) => invoke('activate_tab', { key }),
    closeTab: (key) => invoke('close_tab', { key }),
    reopenTab: (key) => invoke('reopen_tab', { key }),
    moveTab: (key, target) => invoke('move_tab', { key, target }),
    showWindow: (window) => invoke('show_window', { window }),
    setView: (key, view) => invoke('set_view', { key, view }),
    onEvent: (callback) => event.listen(HOST_EVENT, (message) => callback(message.payload)),
  };
}
