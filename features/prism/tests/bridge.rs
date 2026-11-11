use prism::bridge::{
    BridgeCall, InspectParams, Operation, PROTOCOL_VERSION, authorize, is_ready, socket_path,
};
use prism::model::AgentKind;
use prism::session::{LiveBinding, RuntimeDirectory};
use std::os::unix::net::UnixListener;
use std::process::{Child, Command};
use tempfile::TempDir;
use uuid::Uuid;

struct Fixture {
    project: TempDir,
    runtime: RuntimeDirectory,
    /// The attached CLI: a child process, so a test can check that Prism leaves it running.
    cli: Child,
    binding: LiveBinding,
}
impl Fixture {
    fn new() -> Self {
        Self::with_agent(AgentKind::Codex)
    }

    fn with_agent(agent: AgentKind) -> Self {
        let project = TempDir::new().unwrap();
        let runtime = RuntimeDirectory::create(project.path()).unwrap();
        let cli = Command::new("sleep").arg("60").spawn().unwrap();
        let binding = LiveBinding::new(
            project.path(),
            agent.clone(),
            format!("{}-cli://{}", agent.cli_name(), cli.id()),
            cli.id(),
        )
        .unwrap();
        runtime.write_binding(&binding).unwrap();
        Self {
            project,
            runtime,
            cli,
            binding,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.cli.kill();
        let _ = self.cli.wait();
    }
}

#[test]
fn stale_socket_is_not_a_connected_bridge() {
    let fixture = Fixture::new();
    let listener =
        UnixListener::bind(socket_path(&fixture.runtime.binding_path()).unwrap()).unwrap();
    drop(listener);
    assert!(!is_ready(&fixture.runtime.binding_path()));
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
    use prism::bridge::{AnswerParams, PendingParams, QuestionParams, apply_operation};
    use prism::model::{DeliveryMode, ElementKind, Id, Selection};
    use prism::store::Store;
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
        lanes: None,
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
        assert!(!result.contains("native-only-question"));
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
    // The mod's poll sees only shared questions still waiting for the prompt box.
    let pending = apply_operation(
        &mut store,
        &fixture.binding,
        Operation::Pending(PendingParams {}),
    )
    .unwrap();
    assert_eq!(pending["questions"].as_array().unwrap().len(), 1);
    assert_eq!(pending["questions"][0]["text"], "explicitly-shared");
    assert_eq!(
        pending["questions"][0]["request_id"],
        shared.request_id.to_string()
    );
    // Only explicitly shared questions take an answer; it is stored beside the question.
    assert!(
        apply_operation(
            &mut store,
            &fixture.binding,
            Operation::Answer(AnswerParams {
                request_id: private.request_id,
                text: "never".into(),
            })
        )
        .is_err()
    );
    let answered = apply_operation(
        &mut store,
        &fixture.binding,
        Operation::Answer(AnswerParams {
            request_id: shared.request_id,
            text: "Because ancestry is stable.".into(),
        }),
    )
    .unwrap();
    assert_eq!(answered["answer"], "Because ancestry is stable.");
    assert_eq!(
        store
            .question(workspace.id, shared.request_id)
            .unwrap()
            .answer
            .as_deref(),
        Some("Because ancestry is stable.")
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
    for wrong_generation in [false, true] {
        let socket = socket_path(&fixture.runtime.binding_path()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let token = fixture.binding.token.clone();
        let id = fixture.binding.binding_id;
        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(1) {
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
                let body = serde_json::json!({"version":PROTOCOL_VERSION,"binding_id":id,"generation":if wrong_generation { 2 } else { 1 }}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        assert_eq!(is_ready(&fixture.runtime.binding_path()), !wrong_generation);
        server.join().unwrap();
        std::fs::remove_file(socket).unwrap();
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
                    lanes: None,
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
    // No pairing file: the bridge never dials a desktop app that happens to run on the host.
    let link = prism::applink::LinkConfig {
        pairing_file: Some(fixture.project.path().join("no-app.json")),
        ..prism::applink::LinkConfig::from_env()
    };
    let bridge = std::thread::spawn(move || prism::bridge::run_with_link(&bridge_path, link));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !is_ready(&binding_path) {
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
    // The CLI belongs to the user: retirement must leave it running.
    assert!(
        fixture.cli.try_wait().unwrap().is_none(),
        "Claude CLI must survive retirement"
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
                    lanes: None,
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
    // The fifth consecutive failure is treated as a real loss of the bound CLI.
    assert!(!prism::bridge::binding_still_live(false, &mut failures));
}

#[test]
fn tools_close_and_reopen_documents_with_the_revision_check() {
    use prism::actions::{ContentAction, PublishRequest};
    use prism::bridge::{ActionParams, apply_operation};
    use prism::model::Id;
    use prism::store::Store;
    use serde_json::json;
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
        title: "Plan".into(),
        elements: vec![],
        relationships: vec![],
        groups: vec![],
        lanes: None,
    };
    let published = apply_operation(
        &mut store,
        &fixture.binding,
        Operation::Publish(request.clone()),
    )
    .unwrap();
    let document = Id::parse_str(published["document_id"].as_str().unwrap()).unwrap();

    // The Codex tool arguments parse to the shared action; close takes no IDs.
    let close = |revision: u64| {
        prism::mcp::operation(
            "prism_action",
            json!({"document_id": document, "expected_revision": revision, "action": {"action": "close"}}),
        )
        .unwrap()
    };
    let with_ids = prism::mcp::operation(
        "prism_action",
        json!({"document_id": document, "expected_revision": 1, "action": {"action": "close", "ids": [document]}}),
    );
    assert!(with_ids.is_err(), "close takes no IDs");
    let stale = apply_operation(&mut store, &fixture.binding, close(0)).unwrap_err();
    assert!(
        stale.to_string().contains("current revision is 1"),
        "{stale}"
    );
    let closed = apply_operation(&mut store, &fixture.binding, close(1)).unwrap();
    assert_eq!(closed["revision"], 2);

    // Listing marks it; inspecting it says so too.
    let inspect = |document_id| {
        Operation::Inspect(InspectParams {
            document_id,
            cursor: None,
        })
    };
    let listed = apply_operation(&mut store, &fixture.binding, inspect(None)).unwrap();
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["documents"][0]["closed"], true);
    let shown = apply_operation(&mut store, &fixture.binding, inspect(Some(document))).unwrap();
    assert_eq!(
        (shown["closed"].clone(), shown["revision"].clone()),
        (json!(true), json!(2))
    );

    // Reopen through the shared action, close again, then a publish reopens it.
    let reopen = Operation::Action(ActionParams {
        document_id: document,
        expected_revision: 2,
        action: ContentAction::Reopen {},
    });
    apply_operation(&mut store, &fixture.binding, reopen).unwrap();
    apply_operation(&mut store, &fixture.binding, close(3)).unwrap();
    let revision = PublishRequest {
        request_id: Id::new_v4(),
        document_id: Some(document),
        expected_revision: Some(4),
        ..request
    };
    apply_operation(&mut store, &fixture.binding, Operation::Publish(revision)).unwrap();
    let listed = apply_operation(&mut store, &fixture.binding, inspect(None)).unwrap();
    assert_eq!(listed["documents"][0]["closed"], false);
    assert_eq!(listed["documents"][0]["revision"], 5);
}

#[test]
fn the_publish_tool_sets_or_keeps_the_lane_orientation_and_inspect_returns_it() {
    use prism::bridge::apply_operation;
    use serde_json::json;
    let mut fixture = Fixture::new();
    fixture
        .binding
        .bind_conversation(&Uuid::new_v4().to_string())
        .unwrap();
    let mut store = prism::store::Store::open(":memory:").unwrap();
    let publish = |document_id: Option<Uuid>, revision: Option<u64>, lanes: Option<&str>| {
        let mut arguments = json!({"request_id": Uuid::new_v4(), "document_id": document_id, "expected_revision": revision, "title": "Flow", "elements": [], "relationships": [], "groups": []});
        if let Some(lanes) = lanes {
            arguments["lanes"] = lanes.into();
        }
        prism::mcp::operation("prism_publish", arguments).unwrap()
    };
    assert!(
        prism::mcp::operation(
            "prism_publish",
            json!({"request_id": Uuid::new_v4(), "document_id": null, "expected_revision": null, "title": "Flow", "elements": [], "relationships": [], "groups": [], "lanes": "diagonal"}),
        )
        .is_err(),
        "only rows or columns"
    );
    let published = apply_operation(
        &mut store,
        &fixture.binding,
        publish(None, None, Some("columns")),
    )
    .unwrap();
    let document = Uuid::parse_str(published["document_id"].as_str().unwrap()).unwrap();
    let inspect = || {
        Operation::Inspect(InspectParams {
            document_id: Some(document),
            cursor: None,
        })
    };
    let shown = apply_operation(&mut store, &fixture.binding, inspect()).unwrap();
    assert_eq!(shown["lanes"], "columns", "{shown}");
    // Omitted or null keeps it; `auto` chooses again (one lane: nothing to choose).
    apply_operation(
        &mut store,
        &fixture.binding,
        publish(Some(document), Some(1), None),
    )
    .unwrap();
    let shown = apply_operation(&mut store, &fixture.binding, inspect()).unwrap();
    assert_eq!(shown["lanes"], "columns", "{shown}");
    let with_null = prism::mcp::operation(
        "prism_publish",
        json!({"request_id": Uuid::new_v4(), "document_id": document, "expected_revision": 2, "title": "Flow", "elements": [], "relationships": [], "groups": [], "lanes": null}),
    )
    .unwrap();
    apply_operation(&mut store, &fixture.binding, with_null).unwrap();
    let shown = apply_operation(&mut store, &fixture.binding, inspect()).unwrap();
    assert_eq!(shown["lanes"], "columns", "{shown}");
    apply_operation(
        &mut store,
        &fixture.binding,
        publish(Some(document), Some(3), Some("auto")),
    )
    .unwrap();
    let shown = apply_operation(&mut store, &fixture.binding, inspect()).unwrap();
    assert!(shown.get("lanes").is_none(), "{shown}");
}
