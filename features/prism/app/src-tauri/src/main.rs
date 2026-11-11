//! Native shell of the Prism app: Tauri windows around `prism-app-host`. Everything with
//! logic lives in the host; this file only maps commands, events, the menu and windows onto it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use prism_app_host::arrangement::CloseWindow;
use prism_app_host::diagram::{Diagram, SVG_EXTENSION, diagram, export_file_name};
use prism_app_host::registry::{Command, Content, HostEvent};
use prism_app_host::{AppState, EventSink, Host, HostConfig};
use prism_core::graph::Detail;
use prism_core::model::{Document, Id};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent, Wry,
};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

/// The one event name the UI listens to; the payload is a tagged `HostEvent`.
const HOST_EVENT: &str = "prism:event";
const WINDOW_PAGE: &str = "index.html";
const WINDOW_SIZE: (f64, f64) = (1100.0, 760.0);
const WINDOW_MIN_SIZE: (f64, f64) = (560.0, 400.0);
const QUIT_MENU_ID: &str = "quit";
const QUIT_ACCELERATOR: &str = "CmdOrCtrl+Q";
const SVG_FILTER_NAME: &str = "SVG image";

/// Set once the app quits: windows then close without turning their tabs into closed tabs,
/// so the next start restores every window.
#[derive(Default)]
struct Quitting(AtomicBool);

#[tauri::command]
fn app_state(host: State<'_, Host>) -> AppState {
    host.state()
}

/// Async, like `send_command`: serializing content stays off the main thread.
#[tauri::command]
async fn session_content(host: State<'_, Host>, key: String) -> Result<Option<Content>, String> {
    Ok(host.content(&key))
}

#[tauri::command]
async fn send_command(
    host: State<'_, Host>,
    key: String,
    command: Command,
) -> Result<Uuid, String> {
    host.send(&key, command)
}

#[tauri::command]
fn activate_tab(host: State<'_, Host>, window: WebviewWindow, key: String) {
    host.activate_tab(window.label(), &key);
}

#[tauri::command]
fn close_tab(host: State<'_, Host>, key: String) {
    host.close_tab(&key);
}

/// Opens a session's tab in the calling window.
#[tauri::command]
fn reopen_tab(host: State<'_, Host>, window: WebviewWindow, key: String) {
    host.reopen_tab(&key, window.label());
}

/// `target: null` moves the tab to a new window, the only window the app opens after start.
/// Async: creating a window from the main thread inside a command deadlocks on Windows.
#[tauri::command]
async fn move_tab(
    app: AppHandle,
    host: State<'_, Host>,
    key: String,
    target: Option<String>,
) -> Result<(), String> {
    let Some(window) = host.move_tab(&key, target.as_deref()) else {
        return Ok(());
    };
    if app.get_webview_window(&window).is_none() {
        open_window(&app, &host, &window).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Brings another window to the front, for a session shown there.
#[tauri::command]
fn show_window(app: AppHandle, window: String) -> Result<(), String> {
    let Some(native) = app.get_webview_window(&window) else {
        return Ok(());
    };
    native
        .unminimize()
        .and_then(|()| native.set_focus())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_view(host: State<'_, Host>, key: String, view: serde_json::Value) {
    host.set_view(&key, view);
}

/// The calling window's side pane width; `null` for the default.
#[tauri::command]
fn set_pane_width(host: State<'_, Host>, window: WebviewWindow, width: Option<u32>) {
    host.set_pane_width(window.label(), width);
}

/// Sizes the document as the tab lists it at every detail and lays it out at the detail of
/// its zoom, if one is given, on a blocking thread: a large diagram neither holds up the main
/// thread nor the async runtime.
#[tauri::command]
async fn diagram_layout(
    document: Document,
    visible: BTreeSet<Id>,
    detail: Option<Detail>,
) -> Result<Diagram, String> {
    tauri::async_runtime::spawn_blocking(move || diagram(&document, &visible, detail))
        .await
        .map_err(|error| error.to_string())
}

/// Asks where to save the SVG the UI prepared and writes it there. Returns the path, or
/// nothing when the user cancelled. Async: the blocking dialog must not run on the main thread.
#[tauri::command]
async fn export_svg(app: AppHandle, title: String, svg: String) -> Result<Option<String>, String> {
    let Some(chosen) = app
        .dialog()
        .file()
        .set_file_name(export_file_name(&title))
        .add_filter(SVG_FILTER_NAME, &[SVG_EXTENSION])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = chosen.into_path().map_err(|error| error.to_string())?;
    std::fs::write(&path, svg)
        .map_err(|error| format!("Cannot write {}: {error}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

fn open_window(app: &AppHandle, host: &Host, id: &str) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, id, WebviewUrl::App(WINDOW_PAGE.into()))
        .title(host.window_title(id))
        .inner_size(WINDOW_SIZE.0, WINDOW_SIZE.1)
        .min_inner_size(WINDOW_MIN_SIZE.0, WINDOW_MIN_SIZE.1)
        // HTML drag and drop of tabs needs the native file-drop handler off (Windows).
        .disable_drag_drop_handler()
        .build()
}

/// Window titles follow the projects of their tabs. Only existing windows are touched, and
/// none while quitting.
fn update_titles(app: &AppHandle) {
    let (Some(host), Some(quitting)) = (app.try_state::<Host>(), app.try_state::<Quitting>())
    else {
        return;
    };
    if quitting.0.load(Ordering::SeqCst) {
        return;
    }
    for (id, native) in app.webview_windows() {
        let _ = native.set_title(&host.window_title(&id));
    }
}

fn event_sink(app: AppHandle) -> EventSink {
    Box::new(move |event: HostEvent| {
        if event == HostEvent::State {
            update_titles(&app);
        }
        let _ = app.emit(HOST_EVENT, &event);
    })
}

/// A Quit item with Cmd+Q / Ctrl+Q on every platform, so a layout of several windows
/// survives quitting on Windows too. macOS also needs the Edit items for copy and paste.
fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let quit = MenuItemBuilder::with_id(QUIT_MENU_ID, "Quit Prism")
        .accelerator(QUIT_ACCELERATOR)
        .build(app)?;
    if cfg!(target_os = "macos") {
        let application = SubmenuBuilder::new(app, "Prism")
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .item(&quit)
            .build()?;
        let edit = SubmenuBuilder::new(app, "Edit")
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .build()?;
        let window = SubmenuBuilder::new(app, "Window")
            .minimize()
            .close_window()
            .build()?;
        return MenuBuilder::new(app)
            .items(&[&application, &edit, &window])
            .build();
    }
    let file = SubmenuBuilder::new(app, "File").item(&quit).build()?;
    MenuBuilder::new(app).item(&file).build()
}

fn quit(app: &AppHandle) {
    if let Some(quitting) = app.try_state::<Quitting>() {
        quitting.0.store(true, Ordering::SeqCst);
    }
    app.exit(0);
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(Quitting::default())
        .menu(menu)
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ID {
                quit(app);
            }
        })
        .setup(|app| {
            let home = app.path().home_dir()?;
            let data = app.path().app_data_dir()?;
            let config = HostConfig::new(&home, data);
            let host = tauri::async_runtime::block_on(Host::start(
                config,
                event_sink(app.handle().clone()),
            ));
            // The native windows of the saved arrangement; later only `move_tab` opens one.
            for window in host.state().arrangement.windows {
                open_window(app.handle(), &host, &window.id)?;
            }
            app.manage(host);
            Ok(())
        })
        .on_window_event(|window, event| {
            let (Some(host), Some(quitting)) =
                (window.try_state::<Host>(), window.try_state::<Quitting>())
            else {
                return;
            };
            if quitting.0.load(Ordering::SeqCst) {
                return;
            }
            match event {
                WindowEvent::Focused(true) => host.focus_window(window.label()),
                WindowEvent::CloseRequested { .. } => match host.close_window(window.label()) {
                    // Its tabs became closed tabs, reopenable from the sessions list.
                    CloseWindow::Closed => {}
                    // The last window keeps its tabs: closing it quits the app.
                    CloseWindow::Last => quitting.0.store(true, Ordering::SeqCst),
                },
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_state,
            session_content,
            send_command,
            activate_tab,
            close_tab,
            reopen_tab,
            move_tab,
            show_window,
            set_view,
            set_pane_width,
            diagram_layout,
            export_svg
        ])
        .build(tauri::generate_context!())
        .expect("the Prism app starts")
        .run(|app, event| match event {
            // Quitting from the Dock or the system: windows close without losing their tabs.
            RunEvent::ExitRequested { .. } => {
                if let Some(quitting) = app.try_state::<Quitting>() {
                    quitting.0.store(true, Ordering::SeqCst);
                }
            }
            RunEvent::Exit => {
                if let Some(host) = app.try_state::<Host>() {
                    host.shutdown();
                }
            }
            _ => {}
        });
}
