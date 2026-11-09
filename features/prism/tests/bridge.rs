use prism::bridge::{
    BridgeCall, InspectParams, Operation, PROTOCOL_VERSION, authorize, connection_info, socket_path,
};
use prism::model::AgentKind;
use prism::session::{LiveBinding, RuntimeDirectory, TmuxClient};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use uuid::Uuid;

struct Fixture {
    project: TempDir,
    runtime: RuntimeDirectory,
    name: String,
    binding: LiveBinding,
}
impl Fixture {
    fn new() -> Self {
        Self::with_agent(AgentKind::Codex)
    }

    fn with_agent(agent: AgentKind) -> Self {
        let project = TempDir::new().unwrap();
        let runtime = RuntimeDirectory::create(project.path()).unwrap();
        let name = format!("prism-bridge-test-{}", Uuid::new_v4());
        let tmux = TmuxClient::named(&name);
        let pane = tmux
            .launch_cli(project.path(), Path::new("/usr/bin/cat"), &[], None)
            .unwrap();
        // Claude records the CLI pane process itself; Codex records its private server.
        let (endpoint, server_pid) = match agent {
            AgentKind::Codex => ("unix:///unused".to_owned(), std::process::id()),
            AgentKind::Claude => (format!("claude-cli://{}", pane.pane_id), pane.pid),
        };
        let binding = LiveBinding::new(
            project.path(),
            agent,
            endpoint,
            server_pid,
            tmux.identity().unwrap(),
            pane,
        )
        .unwrap();
        tmux.tag_cli(&binding).unwrap();
        runtime.write_binding(&binding).unwrap();
        Self {
            project,
            runtime,
            name,
            binding,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.name, "kill-server"])
            .output();
    }
}

#[test]
fn stale_socket_is_not_a_connected_bridge() {
    let fixture = Fixture::new();
    let listener =
        UnixListener::bind(socket_path(&fixture.runtime.binding_path()).unwrap()).unwrap();
    drop(listener);
    assert!(!connection_info(&fixture.runtime.binding_path()).connected);
}

#[test]
fn codex_mcp_refuses_foreign_thread_before_contacting_bridge() {
    let mut fixture = Fixture::new();
    fixture
        .binding
        .bind_conversation(&Uuid::new_v4().to_string())
        .unwrap();
    fixture.runtime.write_binding(&fixture.binding).unwrap();
    let response = prism::mcp::handle(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"prism_inspect","arguments":{},"_meta":{"threadId":Uuid::new_v4().to_string()}}}), Some(&fixture.runtime.binding_path())).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("bound Codex thread")
    );
    assert!(
        !prism::session::read_binding(&fixture.runtime.binding_path())
            .unwrap()
            .retired
    );
}

#[test]
fn authentication_precedes_binding_and_retirement() {
    let mut fixture = Fixture::new();
    let thread = Uuid::new_v4().to_string();
    let call = BridgeCall {
        version: PROTOCOL_VERSION,
        generation: 1,
        thread_id: thread.clone(),
        operation: Operation::Inspect(InspectParams {
            document_id: None,
            cursor: None,
        }),
    };
    assert!(authorize(&mut fixture.binding, &"x".repeat(64), &call).is_err());
    assert!(fixture.binding.conversation.is_none());
    let token = fixture.binding.token.clone();
    authorize(&mut fixture.binding, &token, &call).unwrap();
    assert_eq!(
        fixture.binding.conversation.as_deref(),
        Some(thread.as_str())
    );
    let changed = BridgeCall {
        thread_id: Uuid::new_v4().to_string(),
        ..call
    };
    assert!(authorize(&mut fixture.binding, &"x".repeat(64), &changed).is_err());
    assert!(!fixture.binding.retired);
    assert!(authorize(&mut fixture.binding, &token, &changed).is_err());
    assert!(fixture.binding.retired);
    assert!(fixture.binding.token.is_empty());
    assert!(fixture.project.path().exists());
}

#[test]
fn operations_preserve_replay_and_expose_only_explicit_shares() {
    use prism::actions::{ElementInput, PrepareQuestion, PublishRequest};
    use prism::bridge::{QuestionParams, apply_operation};
    use prism::model::{DeliveryMode, ElementKind, Id, Selection, ViewKind, ViewState};
    use prism::store::Store;
    use std::collections::{BTreeMap, BTreeSet};
    let mut fixture = Fixture::new();
    fixture
        .binding
        .bind_conversation(&Uuid::new_v4().to_string())
        .unwrap();
    let mut store = Store::open(":memory:").unwrap();
    let request = PublishRequest {
        request_id: Id::new_v4(),
        document_id: None,
        expected_revision: None,
        title: "Public diagram".into(),
        elements: vec![ElementInput {
            id: Id::new_v4(),
            title: "Node".into(),
            body: "Published".into(),
            kind: ElementKind::Text,
            source: None,
        }],
        relationships: vec![],
        groups: vec![],
    };
    let first = apply_operation(
        &mut store,
        &fixture.binding,
        Operation::Publish(request.clone()),
    )
    .unwrap();
    assert_eq!(
        first,
        apply_operation(
            &mut store,
            &fixture.binding,
            Operation::Publish(request.clone())
        )
        .unwrap()
    );
    let workspace = store
        .workspace(&fixture.binding.workspace_key().unwrap())
        .unwrap();
    let document = Id::parse_str(first["document_id"].as_str().unwrap()).unwrap();
    let selection = Selection {
        document_id: document,
        revision: 1,
        ids: vec![request.elements[0].id],
    };
    store
        .save_view(
            workspace.id,
            &ViewState {
                id: Id::new_v4(),
                document_id: document,
                kind: ViewKind::List,
                selection: Some(selection.clone()),
                positions: BTreeMap::new(),
                hidden: BTreeSet::new(),
                filter: "private-filter".into(),
                draft: "private-draft".into(),
            },
        )
        .unwrap();
    store
        .save_ui_state(
            workspace.id,
            &serde_json::json!({"private":"private-layout"}),
        )
        .unwrap();
    let private = PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 1,
        selection: selection.clone(),
        text: "native-only-question".into(),
        mode: DeliveryMode::Start,
    };
    store.prepare_question(workspace.id, &private).unwrap();
    let shared = PrepareQuestion {
        request_id: Id::new_v4(),
        text: "explicitly-shared".into(),
        mode: DeliveryMode::Share,
        ..private.clone()
    };
    store.prepare_question(workspace.id, &shared).unwrap();
    for operation in [
        Operation::Inspect(InspectParams {
            document_id: None,
            cursor: None,
        }),
        Operation::Inspect(InspectParams {
            document_id: Some(document),
            cursor: None,
        }),
        Operation::Questions(QuestionParams {
            request_id: None,
            cursor: None,
        }),
    ] {
        let result = apply_operation(&mut store, &fixture.binding, operation)
            .unwrap()
            .to_string();
        for secret in [
            "private-filter",
            "private-draft",
            "private-layout",
            "native-only-question",
        ] {
            assert!(!result.contains(secret));
        }
    }
    assert!(
        apply_operation(
            &mut store,
            &fixture.binding,
            Operation::Questions(QuestionParams {
                request_id: Some(private.request_id),
                cursor: None
            })
        )
        .is_err()
    );
    let result = apply_operation(
        &mut store,
        &fixture.binding,
        Operation::Questions(QuestionParams {
            request_id: Some(shared.request_id),
            cursor: None,
        }),
    )
    .unwrap();
    assert_eq!(result["text"], "explicitly-shared");
    assert!(
        result["snapshot"]["elements"]
            .get(request.elements[0].id.to_string())
            .is_some()
    );
    let mut other = fixture.binding.clone();
    other.conversation = Some(Uuid::new_v4().to_string());
    assert!(
        apply_operation(
            &mut store,
            &other,
            Operation::Inspect(InspectParams {
                document_id: Some(document),
                cursor: None
            })
        )
        .is_err()
    );
}

#[test]
fn protocol_and_generation_rejections_do_not_bind_a_conversation() {
    let mut fixture = Fixture::new();
    let token = fixture.binding.token.clone();
    for (version, generation, thread_id) in [
        (PROTOCOL_VERSION + 1, 1, Uuid::new_v4().to_string()),
        (PROTOCOL_VERSION, 2, Uuid::new_v4().to_string()),
        (PROTOCOL_VERSION, 1, Uuid::nil().to_string()),
    ] {
        let call = BridgeCall {
            version,
            generation,
            thread_id,
            operation: Operation::Inspect(InspectParams {
                document_id: None,
                cursor: None,
            }),
        };
        assert!(authorize(&mut fixture.binding, &token, &call).is_err());
        assert!(fixture.binding.conversation.is_none());
        assert!(!fixture.binding.retired);
    }
}

#[test]
fn health_requires_an_authenticated_matching_live_service() {
    use std::io::{Read, Write};
    use std::time::Duration;
    let fixture = Fixture::new();
    for (wrong_generation, native_connected) in [(false, true), (true, true), (false, false)] {
        let socket = socket_path(&fixture.runtime.binding_path()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let token = fixture.binding.token.clone();
        let id = fixture.binding.binding_id;
        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).unwrap().to_lowercase();
                assert!(header.starts_with("get /health http/1.1"));
                assert!(header.contains(&format!("authorization: bearer {token}")));
                let body = serde_json::json!({"version":PROTOCOL_VERSION,"binding_id":id,"generation":if wrong_generation { 2 } else { 1 },"native_connected":native_connected}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let info = connection_info(&fixture.runtime.binding_path());
        assert_eq!(info.connected, !wrong_generation && native_connected);
        if info.connected {
            assert_eq!(info.send_modes, vec![prism::model::DeliveryMode::Share]);
        }
        assert_eq!(
            prism::bridge::is_ready(&fixture.runtime.binding_path()),
            !wrong_generation
        );
        server.join().unwrap();
        std::fs::remove_file(socket).unwrap();
    }
}

#[test]
fn health_message_names_the_bound_agent() {
    use std::io::{Read, Write};
    use std::time::Duration;
    for (agent, label) in [(AgentKind::Codex, "Codex"), (AgentKind::Claude, "Claude")] {
        let fixture = Fixture::with_agent(agent);
        let socket = socket_path(&fixture.runtime.binding_path()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let id = fixture.binding.binding_id;
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let body =
                serde_json::json!({"version":PROTOCOL_VERSION,"binding_id":id,"generation":1})
                    .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let info = connection_info(&fixture.runtime.binding_path());
        server.join().unwrap();
        assert!(info.connected);
        assert_eq!(
            info.message,
            format!("Share explicitly, then ask {label} to retrieve the Prism question")
        );
    }
}

#[test]
fn claude_session_end_retires_the_binding_and_preserves_shared_questions() {
    use prism::actions::{ElementInput, PrepareQuestion, PublishRequest};
    use prism::bridge::{BridgeClient, LifecycleEvent};
    use prism::model::{DeliveryMode, DeliveryOutcome, ElementKind, Id, Selection};
    use prism::store::Store;
    use std::time::{Duration, Instant};
    let mut fixture = Fixture::with_agent(AgentKind::Claude);
    let session = Uuid::new_v4().to_string();
    fixture.binding.bind_conversation(&session).unwrap();
    fixture.runtime.write_binding(&fixture.binding).unwrap();
    let binding_path = fixture.runtime.binding_path();
    let store_path = prism::bridge::store_path(&fixture.binding);
    let (workspace, shared) = {
        let mut store = Store::open(&store_path).unwrap();
        let workspace = store
            .workspace(&fixture.binding.workspace_key().unwrap())
            .unwrap();
        let element = Id::new_v4();
        let document = store
            .publish(
                workspace.id,
                &PublishRequest {
                    request_id: Id::new_v4(),
                    document_id: None,
                    expected_revision: None,
                    title: "Doc".into(),
                    elements: vec![ElementInput {
                        id: element,
                        title: "Node".into(),
                        body: String::new(),
                        kind: ElementKind::Text,
                        source: None,
                    }],
                    relationships: vec![],
                    groups: vec![],
                },
            )
            .unwrap();
        let shared = store
            .prepare_question(
                workspace.id,
                &PrepareQuestion {
                    request_id: Id::new_v4(),
                    generation: 1,
                    selection: Selection {
                        document_id: document.document_id,
                        revision: 1,
                        ids: vec![element],
                    },
                    text: "Shared before /clear".into(),
                    mode: DeliveryMode::Share,
                },
            )
            .unwrap();
        (workspace.id, shared)
    };
    let bridge_path = binding_path.clone();
    let bridge = std::thread::spawn(move || prism::bridge::run(&bridge_path));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !connection_info(&binding_path).connected {
        assert!(Instant::now() < deadline, "bridge did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = BridgeClient::new(&binding_path)
        .unwrap()
        .call(
            &session,
            Operation::Lifecycle(LifecycleEvent::SessionEnd {
                reason: "clear".into(),
            }),
        )
        .unwrap();
    assert_eq!(result["data"]["retiring"], true);
    assert!(result.get("pane").is_none());
    let error = bridge.join().unwrap().unwrap_err().to_string();
    assert!(error.contains("clear"), "{error}");
    // The Claude pane is the user's CLI: retirement must leave it running.
    let panes = Command::new("tmux")
        .args(["-L", &fixture.name, "list-panes", "-a", "-F", "#{pane_id}"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(panes.stdout)
            .unwrap()
            .lines()
            .any(|id| id == fixture.binding.cli.pane_id),
        "Claude CLI pane must survive retirement"
    );
    let retired = prism::session::read_binding(&binding_path).unwrap();
    assert!(retired.retired);
    assert_eq!(retired.generation, 2);
    let store = Store::open(&store_path).unwrap();
    let question = store.question(workspace, shared.request_id).unwrap();
    assert_eq!(question.mode, DeliveryMode::Share);
    assert_eq!(question.outcome, shared.outcome);
    assert!(!matches!(question.outcome, DeliveryOutcome::Failed { .. }));
    assert!(fixture.project.path().join(".prism/state.db").exists());
}

#[test]
fn listing_all_documents_and_shared_questions_supports_pagination() {
    use prism::actions::{ElementInput, PrepareQuestion, PublishRequest};
    use prism::bridge::apply_operation;
    use prism::model::{DeliveryMode, ElementKind, Id, Selection};
    use prism::store::Store;
    let mut fixture = Fixture::new();
    fixture
        .binding
        .bind_conversation(&Uuid::new_v4().to_string())
        .unwrap();
    let mut store = Store::open(":memory:").unwrap();
    let workspace = store
        .workspace(&fixture.binding.workspace_key().unwrap())
        .unwrap();
    for index in 0..105 {
        let element = Id::new_v4();
        let document = store
            .publish(
                workspace.id,
                &PublishRequest {
                    request_id: Id::new_v4(),
                    document_id: None,
                    expected_revision: None,
                    title: format!("Document {index}"),
                    elements: vec![ElementInput {
                        id: element,
                        title: "Item".into(),
                        body: String::new(),
                        kind: ElementKind::Text,
                        source: None,
                    }],
                    relationships: vec![],
                    groups: vec![],
                },
            )
            .unwrap();
        store
            .prepare_question(
                workspace.id,
                &PrepareQuestion {
                    request_id: Id::new_v4(),
                    generation: 1,
                    selection: Selection {
                        document_id: document.document_id,
                        revision: 1,
                        ids: vec![element],
                    },
                    text: "\u{0001}".repeat(prism::model::MAX_TEXT_BYTES),
                    mode: DeliveryMode::Share,
                },
            )
            .unwrap();
    }
    store
        .invalidate_generation(
            workspace.id,
            1,
            &"\u{0001}".repeat(prism::model::MAX_TEXT_BYTES),
        )
        .unwrap();
    for (method, collection, id_field) in [
        ("inspect", "documents", "id"),
        ("questions", "questions", "request_id"),
    ] {
        let mut cursor = serde_json::Value::Null;
        let mut ids = std::collections::BTreeSet::new();
        loop {
            let operation = serde_json::from_value(
                serde_json::json!({"method":method,"params":{"cursor":cursor}}),
            )
            .unwrap();
            let page = apply_operation(&mut store, &fixture.binding, operation).unwrap();
            assert_eq!(page["total"], 105);
            assert!(
                serde_json::to_vec(&page).unwrap().len() <= 4 * prism::model::MAX_PAYLOAD_BYTES
            );
            let items = page[collection].as_array().unwrap();
            assert!(items.len() <= 100);
            for item in items {
                assert!(ids.insert(item[id_field].as_str().unwrap().to_owned()));
                if method == "questions" {
                    assert_eq!(item["text_truncated"], true);
                }
            }
            cursor = page["next_cursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        assert_eq!(ids.len(), 105);
        if method == "questions" {
            let operation = serde_json::from_value(serde_json::json!({"method":"questions","params":{"request_id":ids.first().unwrap()}})).unwrap();
            let question = apply_operation(&mut store, &fixture.binding, operation).unwrap();
            assert_eq!(
                question["text"].as_str().unwrap().len(),
                prism::model::MAX_TEXT_BYTES
            );
        }
    }
}

#[test]
fn transient_binding_verification_failures_do_not_retire_the_bridge() {
    let mut failures = 0;
    for _ in 0..4 {
        assert!(prism::bridge::binding_still_live(false, &mut failures));
    }
    assert!(prism::bridge::binding_still_live(true, &mut failures));
    assert_eq!(failures, 0);
    for _ in 0..4 {
        assert!(prism::bridge::binding_still_live(false, &mut failures));
    }
    // The fifth consecutive failure is treated as a real loss of the tmux binding.
    assert!(!prism::bridge::binding_still_live(false, &mut failures));
}
