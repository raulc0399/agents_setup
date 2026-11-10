//! The listener and the session registry against fake bridges speaking the real protocol.
mod common;

use common::*;
use prism_app_host::registry::{Command, HostEvent, Link};
use prism_core::ask::AskItem;
use prism_core::model::{
    ContentAction, DeliveryMode, DeliveryOutcome, Id, MAX_PAYLOAD_BYTES, MAX_TEXT_BYTES, Question,
    Selection, SelectionSnapshot,
};
use prism_core::protocol::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

fn link(app: &TestApp) -> Option<Link> {
    app.host
        .state()
        .sessions
        .into_iter()
        .find(|session| session.key == key())
        .map(|session| session.link)
}

fn titles(app: &TestApp) -> Vec<String> {
    app.host
        .content(&key())
        .unwrap()
        .documents
        .iter()
        .map(|document| document.title.clone())
        .collect()
}

fn selection() -> Selection {
    Selection {
        document_id: Id::from_u128(1),
        revision: 1,
        ids: vec![Id::from_u128(100)],
    }
}

fn shared_question(request: u128) -> Question {
    Question {
        request_id: Id::from_u128(request),
        workspace_id: Id::from_u128(9),
        generation: 1,
        snapshot: SelectionSnapshot {
            selection: selection(),
            document_title: "Document 1".into(),
            elements: BTreeMap::new(),
            relationships: BTreeMap::new(),
            groups: BTreeMap::new(),
        },
        text: "Item 1: First".into(),
        mode: DeliveryMode::Share,
        outcome: DeliveryOutcome::Pending,
        answer: None,
    }
}

fn archive() -> Command {
    Command::Action {
        document_id: Id::from_u128(1),
        expected_revision: 1,
        action: ContentAction::Archive {
            ids: vec![Id::from_u128(100)],
        },
    }
}

#[test]
fn handshake_proves_the_token_both_ways_and_opens_a_tab() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First"])], Vec::new());

    let state = app.wait_for("a loaded session", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });
    assert_eq!(state.sessions[0].link, Link::Live);
    assert_eq!(state.arrangement.windows[0].tabs, vec![key()]);
    assert_eq!(state.arrangement.windows[0].active, Some(key()));
    assert_eq!(titles(&app), vec!["Document 1"]);
}

#[test]
fn a_wrong_token_is_refused_before_the_session_is_trusted() {
    let app = start_app();
    let mut bridge = FakeBridge::open(app.port());
    let app_nonce = bridge.hello(&app.token());
    let proof = proof("another token", Prover::Bridge, &app_nonce).unwrap();
    bridge.send(&BridgeMessage::Auth {
        proof,
        session: session(1, 1),
    });

    assert_eq!(
        bridge.receive(),
        AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Token
        }
    );
    assert!(bridge.closed());
    let state = app.wait_for("the problem", |state| state.status.problem.is_some());
    assert!(state.sessions.is_empty());
    assert!(state.arrangement.windows[0].tabs.is_empty());
}

#[test]
fn another_protocol_number_or_a_bad_hello_is_refused() {
    let app = start_app();
    let hellos = [
        json!({"type": "hello", "protocol": APP_PROTOCOL + 1, "nonce": nonce()}),
        json!({"type": "hello", "nonce": nonce()}),
        json!({"type": "hello", "protocol": APP_PROTOCOL, "nonce": "short"}),
        json!({"type": "auth", "protocol": APP_PROTOCOL}),
    ];
    for hello in hellos {
        let mut bridge = FakeBridge::open(app.port());
        bridge.send_text(&hello.to_string());
        assert_eq!(
            bridge.receive(),
            AppMessage::Refused {
                protocol: APP_PROTOCOL,
                reason: Refusal::Protocol
            },
            "{hello}"
        );
        assert!(bridge.closed());
    }
    assert!(app.host.state().status.problem.is_some());
}

/// Waits for the problem and checks it stays after the refused bridge went away.
fn version_problem(app: &TestApp) -> String {
    let state = app.wait_for("the problem", |state| state.status.problem.is_some());
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(app.host.state().status.problem, state.status.problem);
    state.status.problem.unwrap()
}

#[test]
fn a_bridge_with_another_protocol_is_told_apart_by_which_side_is_older() {
    let older = APP_PROTOCOL - 1;
    let newer = APP_PROTOCOL + 1;
    for (bridge_protocol, expected) in [
        (older, "The bridge is older: install the newer prism binary"),
        (
            newer,
            "This app is older: build and install the newer Prism app",
        ),
    ] {
        let app = start_app();
        let mut bridge = FakeBridge::open(app.port());
        bridge.send_text(
            &json!({"type": "hello", "protocol": bridge_protocol, "nonce": nonce()}).to_string(),
        );
        assert!(matches!(
            bridge.receive(),
            AppMessage::Refused {
                reason: Refusal::Protocol,
                ..
            }
        ));
        assert!(bridge.closed());
        let problem = version_problem(&app);
        assert!(
            problem.starts_with(&format!(
                "A bridge speaks Prism protocol {bridge_protocol}, this app protocol {APP_PROTOCOL}."
            )),
            "{problem}"
        );
        assert!(problem.contains(expected), "{problem}");

        let _bridge = app.connect(session(1, 1));
        app.wait_for("the problem cleared", |state| {
            state.status.problem.is_none()
        });
    }

    // A bridge that refuses the welcome for its own protocol number.
    let app = start_app();
    let mut bridge = FakeBridge::open(app.port());
    bridge.hello(&app.token());
    bridge.send(&BridgeMessage::Refused {
        protocol: newer,
        reason: Refusal::Protocol,
    });
    assert!(bridge.closed());
    let problem = version_problem(&app);
    assert!(
        problem.contains(&format!(
            "protocol {newer}, this app protocol {APP_PROTOCOL}"
        )) && problem.contains("This app is older"),
        "{problem}"
    );
}

#[test]
fn the_status_names_the_app_version_and_protocol() {
    let status = start_app().host.state().status;
    assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(status.protocol, APP_PROTOCOL);
}

/// `tauri.conf.json` repeats the workspace version, which the bundles and the installer
/// carry; this keeps the two in step.
#[test]
fn the_tauri_config_carries_the_workspace_version() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src-tauri/tauri.conf.json");
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(config["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_bridge_refusing_the_welcome_is_reported() {
    let app = start_app();
    let mut bridge = FakeBridge::open(app.port());
    bridge.hello(&app.token());
    bridge.send(&BridgeMessage::Refused {
        protocol: APP_PROTOCOL,
        reason: Refusal::Token,
    });
    assert!(bridge.closed());
    app.wait_for("the problem", |state| state.status.problem.is_some());
}

#[test]
fn a_silent_connection_times_out() {
    let app = start(
        home_with(&[".claude"]),
        tempfile::TempDir::new().unwrap(),
        |config| {
            config.timing.handshake = Duration::from_millis(200);
        },
    );
    let mut bridge = FakeBridge::open(app.port());
    assert!(bridge.closed());
}

#[test]
fn an_oversized_message_drops_the_connection() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(Vec::new(), Vec::new());
    app.wait_for("live", |_| link(&app) == Some(Link::Live));

    let oversized = "x".repeat(MAX_MESSAGE_BYTES + 1);
    let _ = bridge.socket.send(tungstenite::Message::text(oversized));
    assert!(bridge.closed());
    app.wait_for("disconnected", |_| link(&app) == Some(Link::Disconnected));
}

#[test]
fn a_complete_snapshot_replaces_content_and_updates_replace_by_id() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.send(&BridgeMessage::Snapshot {
        documents: vec![document(1, 1, &["First"])],
        questions: Vec::new(),
        closed: Vec::new(),
        more: true,
    });
    bridge.send(&BridgeMessage::Snapshot {
        documents: vec![document(2, 1, &["Second"])],
        questions: vec![shared_question(50)],
        closed: Vec::new(),
        more: false,
    });
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });
    assert_eq!(titles(&app), vec!["Document 1", "Document 2"]);
    assert_eq!(app.host.content(&key()).unwrap().questions.len(), 1);

    let mut revised = document(1, 2, &["First", "Added"]);
    revised.title = "Revised".into();
    bridge.send(&BridgeMessage::Update {
        documents: vec![revised, document(3, 1, &["Third"])],
        questions: Vec::new(),
        closed: Vec::new(),
    });
    app.wait_for("the update", |_| titles(&app).len() == 3);
    assert_eq!(titles(&app), vec!["Revised", "Document 2", "Document 3"]);

    // A later snapshot without document 2 and the question drops both.
    bridge.send(&BridgeMessage::Snapshot {
        documents: vec![document(1, 2, &["First"]), document(3, 1, &["Third"])],
        questions: Vec::new(),
        closed: Vec::new(),
        more: false,
    });
    app.wait_for("the second snapshot", |_| titles(&app).len() == 2);
    assert!(app.host.content(&key()).unwrap().questions.is_empty());
}

#[test]
fn the_closed_set_comes_from_the_snapshot_frames_and_each_update_replaces_it() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    let closed = |app: &TestApp| app.host.content(&key()).unwrap().closed;
    bridge.send(&BridgeMessage::Snapshot {
        documents: vec![document(1, 1, &["First"])],
        questions: Vec::new(),
        closed: vec![Id::from_u128(1)],
        more: true,
    });
    bridge.send(&BridgeMessage::Snapshot {
        documents: vec![document(2, 1, &["Second"])],
        questions: Vec::new(),
        closed: vec![Id::from_u128(2)],
        more: false,
    });
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });
    // Closed documents stay in the content; the set names them.
    assert_eq!(titles(&app), vec!["Document 1", "Document 2"]);
    assert_eq!(closed(&app), vec![Id::from_u128(1), Id::from_u128(2)]);

    bridge.send(&BridgeMessage::Update {
        documents: vec![document(1, 2, &["First"])],
        questions: Vec::new(),
        closed: vec![Id::from_u128(2)],
    });
    app.wait_for("the reopen", |_| closed(&app) == vec![Id::from_u128(2)]);

    bridge.snapshot(
        vec![document(1, 2, &["First"]), document(2, 1, &["Second"])],
        Vec::new(),
    );
    app.wait_for("the snapshot without closed", |_| closed(&app).is_empty());
}

#[test]
fn a_snapshot_shows_only_when_its_last_frame_arrived() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });

    bridge.send(&BridgeMessage::Snapshot {
        documents: vec![document(2, 1, &["Second"])],
        questions: Vec::new(),
        closed: Vec::new(),
        more: true,
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(titles(&app), vec!["Document 1"]);
}

#[test]
fn closed_ends_the_session_and_a_lost_connection_only_disconnects() {
    let app = start_app();
    let bridge = app.connect(session(1, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
    drop(bridge);
    app.wait_for("disconnected", |_| link(&app) == Some(Link::Disconnected));

    let mut bridge = app.connect(session(1, 1));
    app.wait_for("live again", |_| link(&app) == Some(Link::Live));
    bridge.send(&BridgeMessage::Closed {
        reason: "conversation cleared".into(),
    });
    let ended = Link::Ended {
        reason: "conversation cleared".into(),
    };
    app.wait_for("ended", |_| link(&app) == Some(ended.clone()));
    drop(bridge);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(link(&app), Some(ended));
    assert_eq!(
        app.host.send(&key(), Command::Fetch),
        Err("the conversation is not connected; wait until its bridge reconnects".into())
    );
}

#[test]
fn commands_get_fresh_ids_and_replies_are_matched() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });

    let ask = Command::Ask {
        selection: selection(),
        items: vec![AskItem {
            number: 1,
            title: "First".into(),
            quote: "First body".into(),
        }],
    };
    let first = app.host.send(&key(), ask).unwrap();
    let second = app.host.send(&key(), archive()).unwrap();
    assert_ne!(first, second);
    // Time-ordered, so the UI can show the latest question first by its request ID.
    assert_eq!(first.get_version_num(), 7);
    assert!(second > first);
    assert_eq!(
        bridge.receive(),
        AppMessage::Ask {
            id: first,
            selection: selection(),
            text: "Item 1: First\n> First body".into(),
        }
    );
    assert!(matches!(bridge.receive(), AppMessage::Action { id, .. } if id == second));

    bridge.send(&BridgeMessage::Reply {
        id: second,
        error: Some(CommandError::NotFound {
            message: "item".into(),
        }),
    });
    bridge.send(&BridgeMessage::Reply {
        id: first,
        error: None,
    });
    assert_eq!(
        app.next_reply(),
        HostEvent::Reply {
            key: key(),
            id: second,
            error: Some(CommandError::NotFound {
                message: "item".into()
            }),
        }
    );
    assert_eq!(
        app.next_reply(),
        HostEvent::Reply {
            key: key(),
            id: first,
            error: None
        }
    );
}

#[test]
fn an_ask_about_several_items_names_each_of_them() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First", "Second"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });

    // As the UI sends it.
    let ask: Command = serde_json::from_value(json!({
        "type": "ask",
        "selection": {"document_id": Id::from_u128(1), "revision": 1, "ids": [Id::from_u128(101), Id::from_u128(100)]},
        "items": [
            {"number": 2, "title": "Second", "quote": "Second body"},
            {"number": 1, "title": "First", "quote": "First"},
        ],
    }))
    .unwrap();
    let id = app.host.send(&key(), ask).unwrap();
    assert_eq!(
        bridge.receive(),
        AppMessage::Ask {
            id,
            selection: Selection {
                document_id: Id::from_u128(1),
                revision: 1,
                ids: vec![Id::from_u128(101), Id::from_u128(100)],
            },
            text: "Item 2: Second\n> Second body\nItem 1: First\n> First".into(),
        }
    );
}

#[test]
fn a_close_from_the_ui_reaches_the_bridge_as_an_action() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });

    // As the UI sends it.
    let close: Command = serde_json::from_value(json!({
        "type": "action",
        "document_id": Id::from_u128(1),
        "expected_revision": 1,
        "action": {"action": "close"},
    }))
    .unwrap();
    let id = app.host.send(&key(), close).unwrap();
    assert_eq!(
        bridge.receive(),
        AppMessage::Action {
            id,
            document_id: Id::from_u128(1),
            expected_revision: 1,
            action: ContentAction::Close {},
        }
    );
}

#[test]
fn an_ask_over_the_text_limit_is_refused_before_it_is_sent() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });

    let ask = Command::Ask {
        selection: selection(),
        items: vec![AskItem {
            number: 1,
            title: "First".into(),
            quote: "x".repeat(MAX_TEXT_BYTES),
        }],
    };
    let error = app.host.send(&key(), ask).unwrap_err();
    assert!(
        error.contains("more than the 65536 the CLI accepts"),
        "{error}"
    );

    // Nothing was queued: the next message the bridge sees is the next command.
    let fetch = app.host.send(&key(), Command::Fetch).unwrap();
    assert_eq!(bridge.receive(), AppMessage::Fetch { id: fetch });
}

#[test]
fn a_stale_revision_fetches_the_current_content() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });

    let id = app.host.send(&key(), archive()).unwrap();
    bridge.receive();
    bridge.send(&BridgeMessage::Reply {
        id,
        error: Some(CommandError::StaleRevision { current: 2 }),
    });
    assert!(matches!(bridge.receive(), AppMessage::Fetch { .. }));
    assert!(matches!(
        app.next_reply(),
        HostEvent::Reply {
            error: Some(CommandError::StaleRevision { current: 2 }),
            ..
        }
    ));
}

#[test]
fn the_same_binding_reconnecting_replaces_the_connection_and_gets_unanswered_commands() {
    let app = start_app();
    let mut first = app.connect(session(1, 1));
    first.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });
    let id = app.host.send(&key(), archive()).unwrap();
    assert!(matches!(first.receive(), AppMessage::Action { .. }));

    // The reply never came; the same binding dials again (for example after a network drop
    // the app did not notice yet).
    let mut second = app.connect(session(1, 1));
    assert!(first.closed());
    assert!(matches!(second.receive(), AppMessage::Action { id: resent, .. } if resent == id));
    assert_eq!(link(&app), Some(Link::Live));
    assert_eq!(
        titles(&app),
        vec!["Document 1"],
        "content stays until the new snapshot"
    );
}

#[test]
fn a_newer_binding_supersedes_and_an_older_one_is_refused() {
    let app = start_app();
    let mut older = app.connect(session(1, 1));
    older.snapshot(vec![document(1, 1, &["Old"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });
    let id = app.host.send(&key(), archive()).unwrap();
    older.receive();

    let mut newer = app.connect(session(2, 1));
    assert_eq!(
        older.receive(),
        AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Superseded
        }
    );
    assert!(older.closed());
    assert!(matches!(
        app.next_reply(),
        HostEvent::Reply { id: failed, error: Some(CommandError::Failed { .. }), .. } if failed == id
    ));
    newer.snapshot(vec![document(2, 1, &["New"])], Vec::new());
    app.wait_for("new content", |_| titles(&app) == vec!["Document 2"]);

    // The older binding dials again: refused, the newer one keeps the tab.
    let mut again = app.connect(session(1, 1));
    assert_eq!(
        again.receive(),
        AppMessage::Refused {
            protocol: APP_PROTOCOL,
            reason: Refusal::Superseded
        }
    );
    assert!(again.closed());
    assert_eq!(link(&app), Some(Link::Live));
    let state = app.host.state();
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.arrangement.windows[0].tabs, vec![key()]);
}

#[test]
fn the_binding_id_breaks_a_tie_of_bound_at() {
    let app = start_app();
    let mut lower = app.connect(session(5, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
    let _higher = app.connect(session(5, 2));
    assert!(matches!(
        lower.receive(),
        AppMessage::Refused {
            reason: Refusal::Superseded,
            ..
        }
    ));
}

#[test]
fn closed_fails_unanswered_commands() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
    let id = app.host.send(&key(), archive()).unwrap();
    bridge.send(&BridgeMessage::Closed {
        reason: "exit".into(),
    });
    assert!(matches!(
        app.next_reply(),
        HostEvent::Reply { id: failed, error: Some(CommandError::Failed { .. }), .. } if failed == id
    ));
}

#[test]
fn snapshot_frames_from_the_core_are_applied_whole() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    // Two documents that do not fit one frame.
    let mut large = document(1, 1, &["Large"]);
    large.title = "L".repeat(MAX_PAYLOAD_BYTES * 2 / 3);
    let mut second = document(2, 1, &["Second"]);
    second.title = "S".repeat(MAX_PAYLOAD_BYTES * 2 / 3);
    let frames = snapshot_frames(vec![large, second], Vec::new(), Vec::new()).unwrap();
    assert_eq!(frames.len(), 2);
    for frame in &frames {
        bridge.send(frame);
    }
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });
    assert_eq!(app.host.content(&key()).unwrap().documents.len(), 2);
}

#[test]
fn a_newer_binding_with_a_wrong_proof_leaves_the_live_older_bridge_alone() {
    let app = start_app();
    let mut older = app.connect(session(1, 1));
    older.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });

    let impostor = FakeBridge::with_wrong_proof(app.port(), &app.token(), session(9, 9));
    drop(impostor);
    app.wait_for("the problem", |state| state.status.problem.is_some());

    assert_eq!(link(&app), Some(Link::Live));
    let id = app.host.send(&key(), Command::Fetch).unwrap();
    assert_eq!(older.receive(), AppMessage::Fetch { id });
}

#[test]
fn a_partial_snapshot_is_dropped_with_its_connection() {
    let app = start_app();
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(1, 1, &["First"])], Vec::new());
    app.wait_for("loaded", |state| {
        state.sessions.iter().any(|session| session.loaded)
    });
    bridge.send(&BridgeMessage::Snapshot {
        documents: vec![document(2, 1, &["Partial"])],
        questions: Vec::new(),
        closed: Vec::new(),
        more: true,
    });
    drop(bridge);
    app.wait_for("disconnected", |_| link(&app) == Some(Link::Disconnected));

    // The reconnected bridge's snapshot starts afresh: the frame of the lost one is gone.
    let mut bridge = app.connect(session(1, 1));
    bridge.snapshot(vec![document(3, 1, &["Third"])], Vec::new());
    app.wait_for("the new snapshot", |_| titles(&app) == vec!["Document 3"]);
}

#[test]
fn a_command_to_a_waiting_session_is_rejected() {
    let app = start_app();
    let bridge = app.connect(session(1, 1));
    app.wait_for("the tab", |state| {
        !state.arrangement.windows[0].tabs.is_empty()
    });
    drop(bridge);
    let TestApp { home, data, .. } = app;

    let restarted = start(home, data, |_| {});
    assert_eq!(restarted.host.state().sessions[0].link, Link::Waiting);
    assert_eq!(
        restarted.host.send(&key(), archive()),
        Err("the conversation has not connected since Prism started".into())
    );
}

#[test]
fn an_unanswered_command_fails_after_its_timeout() {
    let app = start(
        home_with(&[".claude"]),
        tempfile::TempDir::new().unwrap(),
        |config| {
            config.timing.command = Duration::from_millis(300);
        },
    );
    let mut bridge = app.connect(session(1, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
    let id = app.host.send(&key(), archive()).unwrap();
    bridge.receive();
    assert!(matches!(
        app.next_reply(),
        HostEvent::Reply { id: failed, error: Some(CommandError::Failed { .. }), .. } if failed == id
    ));
}

#[test]
fn commands_fail_when_the_session_stays_disconnected() {
    let app = start(
        home_with(&[".claude"]),
        tempfile::TempDir::new().unwrap(),
        |config| {
            config.timing.disconnect_grace = Duration::from_millis(200);
        },
    );
    let mut bridge = app.connect(session(1, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
    let id = app.host.send(&key(), archive()).unwrap();
    bridge.receive();
    drop(bridge);
    assert!(matches!(
        app.next_reply(),
        HostEvent::Reply { id: failed, error: Some(CommandError::Failed { .. }), .. } if failed == id
    ));
    assert_eq!(link(&app), Some(Link::Disconnected));
}

fn with_fast_pings() -> TestApp {
    start(
        home_with(&[".claude"]),
        tempfile::TempDir::new().unwrap(),
        |config| {
            config.timing.ping_interval = Duration::from_millis(50);
        },
    )
}

#[test]
fn a_bridge_that_answers_pings_stays_live() {
    let app = with_fast_pings();
    let mut bridge = app.connect(session(1, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
    bridge
        .socket
        .get_mut()
        .set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    // tungstenite answers each ping with a pong during the next read.
    let until = std::time::Instant::now() + Duration::from_millis(500);
    let mut pings = 0;
    while std::time::Instant::now() < until {
        if let Ok(tungstenite::Message::Ping(_)) = bridge.socket.read() {
            pings += 1;
        }
    }
    assert!(pings >= 3, "{pings} pings");
    assert_eq!(link(&app), Some(Link::Live));
}

#[test]
fn a_bridge_that_misses_pongs_is_disconnected() {
    let app = with_fast_pings();
    let _silent = app.connect(session(1, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
    app.wait_for("disconnected", |_| link(&app) == Some(Link::Disconnected));
}

#[test]
fn handshakes_in_progress_are_capped() {
    let app = start_app();
    let silent: Vec<_> = (0..16)
        .map(|_| std::net::TcpStream::connect(("127.0.0.1", app.port())).unwrap())
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    let stream = std::net::TcpStream::connect(("127.0.0.1", app.port())).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    assert!(tungstenite::client(format!("ws://127.0.0.1:{}/", app.port()), stream).is_err());

    drop(silent);
    std::thread::sleep(Duration::from_millis(100));
    let _bridge = app.connect(session(1, 1));
    app.wait_for("live", |_| link(&app) == Some(Link::Live));
}

#[test]
fn problems_are_not_repeated_and_clear_when_a_bridge_connects() {
    let app = start_app();
    for _ in 0..3 {
        let mut bridge = FakeBridge::with_wrong_proof(app.port(), &app.token(), session(1, 1));
        assert!(matches!(
            bridge.receive(),
            AppMessage::Refused {
                reason: Refusal::Token,
                ..
            }
        ));
    }
    std::thread::sleep(Duration::from_millis(50));
    let problem_events = app
        .events
        .try_iter()
        .filter(|event| *event == HostEvent::State)
        .count();
    assert_eq!(problem_events, 1);

    let _bridge = app.connect(session(1, 1));
    app.wait_for("the problem cleared", |state| {
        state.status.problem.is_none()
    });
}
