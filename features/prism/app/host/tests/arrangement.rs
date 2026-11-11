//! Windows, tabs, project memory, closed tabs, view state and their persistence.
mod common;

use common::*;
use prism_app_host::arrangement::{
    Arrangement, CORRUPT_ARRANGEMENT_FILE, CloseWindow, Ending, SessionLabel, project_name,
};
use prism_app_host::registry::Link;
use prism_core::model::AgentKind;
use prism_core::protocol::{AppMessage, BridgeMessage, Refusal};
use serde_json::json;
use tempfile::TempDir;

fn label(project: &str) -> SessionLabel {
    SessionLabel {
        host: "d_claude".into(),
        project: project.into(),
        agent: AgentKind::Claude,
        conversation: "c".into(),
    }
}

fn arrangement() -> Arrangement {
    let mut arrangement = Arrangement::default();
    arrangement.ensure_window();
    arrangement
}

#[test]
fn a_new_conversation_opens_in_its_projects_window() {
    let mut arrangement = arrangement();
    assert_eq!(arrangement.place("a1", label("/p/a")), Some("w1".into()));
    let window = arrangement.move_tab("a1", None).unwrap();
    assert_eq!(window, "w2");

    // Project a now lives in w2; project b goes to the window focused last.
    arrangement.focus("w1");
    assert_eq!(arrangement.place("a2", label("/p/a")), Some("w2".into()));
    assert_eq!(arrangement.place("b1", label("/p/b")), Some("w1".into()));
    assert_eq!(arrangement.window("w2").unwrap().tabs, vec!["a1", "a2"]);
    // A new tab does not take over the window the user is looking at.
    assert_eq!(
        arrangement.window("w2").unwrap().active.as_deref(),
        Some("a1")
    );
}

#[test]
fn placing_a_known_tab_again_changes_nothing() {
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    assert_eq!(arrangement.place("a1", label("/p/a")), None);
    assert_eq!(arrangement.windows[0].tabs, vec!["a1"]);
}

#[test]
fn a_closed_tab_stays_closed_until_reopened() {
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    arrangement.place("a2", label("/p/a"));
    arrangement.activate("w1", "a1");
    arrangement.close_tab("a1");
    assert_eq!(arrangement.windows[0].tabs, vec!["a2"]);
    assert_eq!(arrangement.windows[0].active.as_deref(), Some("a2"));
    assert_eq!(arrangement.closed, vec!["a1"]);

    // The bridge reconnecting does not reopen it.
    assert_eq!(arrangement.place("a1", label("/p/a")), None);
    assert!(arrangement.window_of("a1").is_none());

    arrangement.reopen("a1", label("/p/a"), "w1");
    assert!(arrangement.closed.is_empty());
    assert_eq!(arrangement.windows[0].tabs, vec!["a2", "a1"]);
    assert_eq!(arrangement.windows[0].active.as_deref(), Some("a1"));
}

#[test]
fn moving_a_tab_to_another_window_remembers_the_project_there() {
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    let second = arrangement.move_tab("a1", None).unwrap();
    arrangement.place("b1", label("/p/b"));
    arrangement.move_tab("b1", Some(&second));
    assert_eq!(arrangement.window(&second).unwrap().tabs, vec!["a1", "b1"]);
    assert_eq!(arrangement.project_windows["/p/b"], second);
    assert!(arrangement.window("w1").unwrap().tabs.is_empty());
    assert_eq!(arrangement.window_title(&second), "Prism · a, b");
    assert_eq!(arrangement.window_title("w1"), "Prism");
}

#[test]
fn moving_to_a_window_that_is_gone_opens_a_new_one() {
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    assert_eq!(arrangement.move_tab("a1", Some("w9")), Some("w2".into()));
    assert_eq!(arrangement.move_tab("unknown", None), None);
}

#[test]
fn closing_a_window_closes_its_tabs_and_forgets_its_projects() {
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    let second = arrangement.move_tab("a1", None).unwrap();
    arrangement.focus(&second);

    assert_eq!(arrangement.close_window(&second), CloseWindow::Closed);
    assert_eq!(arrangement.closed, vec!["a1"]);
    assert!(arrangement.project_windows.is_empty());
    assert_eq!(arrangement.focused, None);
    assert_eq!(arrangement.close_window("w1"), CloseWindow::Last);
    assert_eq!(arrangement.windows.len(), 1);
}

#[test]
fn window_labels_are_never_reused() {
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    let second = arrangement.move_tab("a1", None).unwrap();
    arrangement.close_window(&second);
    arrangement.reopen("a1", label("/p/a"), "w1");
    assert_eq!(arrangement.move_tab("a1", None), Some("w3".into()));
}

#[test]
fn views_belong_to_known_tabs_and_are_forgotten_with_them() {
    let mut arrangement = arrangement();
    arrangement.set_view("a1", json!({"focus": null}));
    assert!(arrangement.views.is_empty());

    arrangement.place("a1", label("/p/a"));
    arrangement.set_view("a1", json!({"hidden": ["x"]}));
    arrangement.close_tab("a1");
    assert_eq!(arrangement.views["a1"], json!({"hidden": ["x"]}));
    for index in 0..60 {
        let key = format!("k{index}");
        arrangement.place(&key, label("/p/k"));
        arrangement.close_tab(&key);
    }
    assert_eq!(arrangement.closed.len(), 50);
    assert!(!arrangement.views.contains_key("a1"));
    assert!(!arrangement.labels.contains_key("a1"));
}

fn ending(reason: &str) -> Ending {
    Ending {
        reason: reason.into(),
        ended_at_ms: 1_700_000_000_000,
    }
}

#[test]
fn ended_marks_belong_to_known_tabs_and_are_forgotten_with_them() {
    let mut arrangement = arrangement();
    arrangement.end("a1", ending("exit"));
    assert!(arrangement.ended.is_empty());

    arrangement.place("a1", label("/p/a"));
    arrangement.end("a1", ending("exit"));
    arrangement.close_tab("a1");
    assert_eq!(arrangement.ended["a1"], ending("exit"));
    arrangement.resume("a1");
    assert!(arrangement.ended.is_empty());

    arrangement.end("a1", ending("exit"));
    for index in 0..60 {
        let key = format!("k{index}");
        arrangement.place(&key, label("/p/k"));
        arrangement.close_tab(&key);
    }
    assert!(!arrangement.ended.contains_key("a1"));
}

#[test]
fn last_seen_times_belong_to_known_tabs_and_are_forgotten_with_them() {
    let mut arrangement = arrangement();
    arrangement.see("a1", 5);
    assert!(arrangement.seen.is_empty());

    arrangement.place("a1", label("/p/a"));
    arrangement.see("a1", 5);
    arrangement.close_tab("a1");
    assert_eq!(arrangement.seen["a1"], 5);
    arrangement.resume("a1");
    assert!(arrangement.seen.is_empty());

    arrangement.see("a1", 5);
    for index in 0..60 {
        let key = format!("k{index}");
        arrangement.place(&key, label("/p/k"));
        arrangement.close_tab(&key);
    }
    assert!(!arrangement.seen.contains_key("a1"));
}

#[test]
fn an_arrangement_saved_without_ended_marks_still_loads() {
    let data = TempDir::new().unwrap();
    let path = Arrangement::path(data.path());
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    let mut older = serde_json::to_value(&arrangement).unwrap();
    let fields = older.as_object_mut().unwrap();
    fields.remove("ended").unwrap();
    fields.remove("seen").unwrap();
    std::fs::write(&path, serde_json::to_vec(&older).unwrap()).unwrap();
    assert_eq!(Arrangement::load(&path), (arrangement, None));
}

#[test]
fn saves_and_restores_and_starts_empty_from_a_broken_file() {
    let data = TempDir::new().unwrap();
    let path = Arrangement::path(data.path());
    let mut arrangement = arrangement();
    arrangement.place("a1", label("/p/a"));
    arrangement.set_view("a1", json!({"document": "d"}));
    arrangement.end("a1", ending("exit"));
    arrangement.save(&path).unwrap();
    assert_eq!(Arrangement::load(&path), (arrangement, None));

    std::fs::write(&path, "{").unwrap();
    let (loaded, error) = Arrangement::load(&path);
    assert_eq!(loaded, Arrangement::default());
    assert!(error.unwrap().contains(CORRUPT_ARRANGEMENT_FILE));
    let copy = path.with_file_name(CORRUPT_ARRANGEMENT_FILE);
    assert_eq!(std::fs::read_to_string(copy).unwrap(), "{");
}

#[test]
fn a_missing_file_starts_empty_without_a_warning() {
    let data = TempDir::new().unwrap();
    let path = Arrangement::path(data.path());
    assert_eq!(Arrangement::load(&path), (Arrangement::default(), None));
}

#[test]
fn the_host_reports_a_corrupt_arrangement_and_starts_with_one_window() {
    let data = TempDir::new().unwrap();
    std::fs::write(Arrangement::path(data.path()), "not json").unwrap();
    let app = start(home_with(&[".claude"]), data, |_| {});
    let state = app.host.state();
    assert_eq!(state.arrangement.windows.len(), 1);
    assert_eq!(state.status.warnings.len(), 1);
    assert!(state.status.warnings[0].contains("could not be read"));
}

#[test]
fn unchanged_arrangements_are_neither_saved_nor_broadcast() {
    let app = start_app();
    let _bridge = app.connect(session(1, 1));
    app.wait_for("the tab", |state| {
        !state.arrangement.windows[0].tabs.is_empty()
    });
    let path = Arrangement::path(app.data.path());
    std::thread::sleep(std::time::Duration::from_millis(50));
    while app.events.try_recv().is_ok() {}
    let saved = std::fs::metadata(&path).unwrap().modified().unwrap();

    // Activating the active tab changes nothing.
    app.host.activate_tab("w1", &key());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(app.events.try_recv().is_err(), "no event");
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), saved);
}

#[test]
fn view_and_focus_changes_are_saved_later_without_a_broadcast() {
    let app = start_app();
    let _bridge = app.connect(session(1, 1));
    app.wait_for("the tab", |state| {
        !state.arrangement.windows[0].tabs.is_empty()
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    while app.events.try_recv().is_ok() {}

    app.host.set_view(&key(), json!({"focus": "a"}));
    app.host.set_view(&key(), json!({"focus": "b"}));
    app.host.focus_window("w1");
    assert!(app.events.try_recv().is_err(), "no broadcast");
    let path = Arrangement::path(app.data.path());
    let saved = || Arrangement::load(&path).0.views.get(&key()).cloned();
    assert_eq!(saved(), None, "not saved at once");
    std::thread::sleep(std::time::Duration::from_millis(800));
    assert_eq!(saved(), Some(json!({"focus": "b"})));
}

#[test]
fn project_names_are_the_last_path_component() {
    assert_eq!(project_name("/workspace/agents_setup/"), "agents_setup");
    assert_eq!(project_name(r"C:\work\prism"), "prism");
    assert_eq!(project_name("/"), "/");
}

#[test]
fn the_host_restores_tabs_whose_bridges_have_not_connected_yet() {
    let app = start_app();
    let _bridge = app.connect(session(1, 1));
    app.wait_for("the tab", |state| {
        !state.arrangement.windows[0].tabs.is_empty()
    });
    app.host.set_view(&key(), json!({"focus": "x"}));
    let second = app.host.move_tab(&key(), None).unwrap();
    app.host.focus_window(&second);
    let TestApp { home, data, .. } = app;

    let restarted = start(home, data, |_| {});
    let state = restarted.host.state();
    assert_eq!(state.arrangement.window(&second).unwrap().tabs, vec![key()]);
    assert_eq!(state.arrangement.views[&key()], json!({"focus": "x"}));
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].link, Link::Waiting);
    assert!(state.sessions[0].restored);
    assert_eq!(state.sessions[0].project, PROJECT);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn saved_seen(app: &TestApp) -> Option<u64> {
    let saved = Arrangement::load(&Arrangement::path(app.data.path())).0;
    saved.seen.get(&key()).copied()
}

#[test]
fn a_disconnect_and_quitting_remember_when_the_bridge_was_last_seen() {
    let app = start_app();
    let bridge = app.connect(session(1, 1));
    app.wait_for("the tab", |state| {
        !state.arrangement.windows[0].tabs.is_empty()
    });
    assert_eq!(saved_seen(&app), None);
    let before = now_ms();
    drop(bridge);
    app.wait_for("seen", |state| state.arrangement.seen.contains_key(&key()));
    assert!(saved_seen(&app).unwrap() >= before, "saved at once");

    // Connected again: not waiting, so no time; quitting writes it.
    let _bridge = app.connect(session(1, 1));
    app.wait_for("live", |state| {
        state.sessions[0].link == Link::Live && state.arrangement.seen.is_empty()
    });
    assert_eq!(saved_seen(&app), None);
    let before = now_ms();
    app.host.shutdown();
    assert!(saved_seen(&app).unwrap() >= before);
    let TestApp { home, data, .. } = app;

    let restarted = start(home, data, |_| {});
    let state = restarted.host.state();
    assert_eq!(state.sessions[0].link, Link::Waiting);
    assert!(state.arrangement.seen[&key()] >= before);
}

fn ended_mark(app: &TestApp) -> Option<Ending> {
    let saved = Arrangement::load(&Arrangement::path(app.data.path())).0;
    assert_eq!(
        saved.ended,
        app.host.state().arrangement.ended,
        "saved at once"
    );
    saved.ended.get(&key()).cloned()
}

#[test]
fn a_conversation_that_ended_shows_as_ended_after_a_restart_until_it_resumes() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    app.wait_for("the tab", |state| {
        !state.arrangement.windows[0].tabs.is_empty()
    });
    assert_eq!(ended_mark(&app), None);
    let before = now_ms();
    bridge.send(&BridgeMessage::Closed {
        reason: "conversation cleared".into(),
    });
    app.wait_for("the mark", |state| {
        state.arrangement.ended.contains_key(&key())
    });
    let mark = ended_mark(&app).unwrap();
    assert_eq!(mark.reason, "conversation cleared");
    assert!(mark.ended_at_ms >= before);
    drop(bridge);
    let TestApp { home, data, .. } = app;

    let restarted = start(home, data, |_| {});
    let state = restarted.host.state();
    assert_eq!(state.arrangement.windows[0].tabs, vec![key()]);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(
        state.sessions[0].link,
        Link::Ended {
            reason: "conversation cleared".into()
        }
    );
    assert!(state.sessions[0].restored);
    assert!(!state.sessions[0].loaded);
    assert_eq!(state.sessions[0].project, PROJECT);

    // Resumed: live again, the mark is gone.
    let _resumed = restarted.connect(session(2, 1));
    let state = restarted.wait_for("live", |state| state.sessions[0].link == Link::Live);
    assert!(!state.sessions[0].restored);
    assert_eq!(ended_mark(&restarted), None);
}

#[test]
fn superseding_ends_no_tab_and_an_older_binding_leaves_the_mark() {
    let app = start_app();
    let mut older = app.connect(session(1, 1));
    app.wait_for("live", |state| {
        state
            .sessions
            .first()
            .is_some_and(|session| session.link == Link::Live)
    });
    let mut newer = app.connect(session(2, 1));
    assert!(matches!(
        older.receive(),
        AppMessage::Refused {
            reason: Refusal::Superseded,
            ..
        }
    ));
    // The newer binding holds the tab: nothing ended.
    assert_eq!(app.host.state().sessions[0].link, Link::Live);
    assert_eq!(ended_mark(&app), None);

    newer.send(&BridgeMessage::Closed {
        reason: "exit".into(),
    });
    app.wait_for("the mark", |state| {
        state.arrangement.ended.contains_key(&key())
    });
    let mark = ended_mark(&app);
    let mut again = app.connect(session(1, 1));
    assert!(matches!(
        again.receive(),
        AppMessage::Refused {
            reason: Refusal::Superseded,
            ..
        }
    ));
    assert!(again.closed());
    assert_eq!(ended_mark(&app), mark);
}

#[test]
fn the_host_reopens_a_live_session_from_the_session_list() {
    let app = start_app();
    let _first = app.connect(session_for("one", "/p/a"));
    let _second = app.connect(session_for("two", "/p/b"));
    app.wait_for("two tabs", |state| {
        state.arrangement.windows[0].tabs.len() == 2
    });

    app.host.close_tab("claude:one");
    let state = app.host.state();
    assert_eq!(state.arrangement.closed, vec!["claude:one"]);
    assert_eq!(
        state.sessions.len(),
        2,
        "a closed tab's session stays listed"
    );

    app.host.reopen_tab("claude:one", "w1");
    let window = &app.host.state().arrangement.windows[0];
    assert_eq!(window.tabs, vec!["claude:two", "claude:one"]);
    assert_eq!(window.active.as_deref(), Some("claude:one"));
}
