/**
 * The only module that talks to the native shell. The UI receives a backend object, so a
 * test harness can drive it in a plain browser with a fake one of the same shape:
 *
 *   windowId                      label of this native window
 *   state()                       -> AppState {status, sessions, arrangement}
 *   content(key)                  -> {documents, questions, closed} or null; `closed`: IDs
 *                                    of the documents closed for everyone
 *   send(key, command)            -> command ID; rejects with a message
 *   activateTab(key), closeTab(key), reopenTab(key), moveTab(key, target | null),
 *   showWindow(windowId), setView(key, view)
 *   diagram(document, visibleIds, detail | null)
 *                                 -> {sizes: {<detail>: {width, height}}, steps: {<link
 *                                 ID>: step}, layout: {detail, svg, order} | null}: from
 *                                 prism-core, the size of the whole diagram at every detail
 *                                 level, the number each arrow's pill shows and the layout
 *                                 of the one asked for
 *   exportSvg(title, svg)         -> the path saved to, or null when the user cancelled
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
    diagram: (document, visible, detail) => invoke('diagram_layout', { document, visible, detail }),
    exportSvg: (title, svg) => invoke('export_svg', { title, svg }),
    onEvent: (callback) => event.listen(HOST_EVENT, (message) => callback(message.payload)),
  };
}
