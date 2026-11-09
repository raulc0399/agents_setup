use prism::actions::*;
use prism::model::*;
use prism::store::Store;
use std::collections::{BTreeMap, BTreeSet};

fn key(conversation: &str) -> WorkspaceKey {
    WorkspaceKey {
        project: "/project".into(),
        agent: AgentKind::Codex,
        conversation: conversation.into(),
    }
}

fn publication() -> PublishRequest {
    let a = Id::new_v4();
    let b = Id::new_v4();
    PublishRequest {
        request_id: Id::new_v4(),
        document_id: None,
        expected_revision: None,
        title: "Flow".into(),
        elements: vec![
            ElementInput {
                id: a,
                title: "Source".into(),
                body: "Original".into(),
                kind: ElementKind::Step,
                source: None,
            },
            ElementInput {
                id: b,
                title: "Sink".into(),
                body: String::new(),
                kind: ElementKind::Text,
                source: None,
            },
        ],
        relationships: vec![RelationshipInput {
            id: Id::new_v4(),
            from: a,
            to: b,
            label: "flows".into(),
            direction: Direction::Directed,
        }],
        groups: vec![GroupInput {
            id: Id::new_v4(),
            title: "Both".into(),
            members: [a, b].into_iter().collect(),
        }],
    }
}

fn setup() -> (Store, Id, PublishRequest, PublishResult) {
    let mut store = Store::open(":memory:").unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let request = publication();
    let result = store.publish(workspace, &request).unwrap();
    (store, workspace, request, result)
}

#[test]
fn private_ui_state_is_bounded_workspace_scoped_and_persistent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let other = store.workspace(&key("thread-2")).unwrap().id;
    let state = serde_json::json!({"active": "private-tab", "split": true});
    store.save_ui_state(workspace, &state).unwrap();
    assert_eq!(store.ui_state(other).unwrap(), None);
    assert!(store.documents(workspace).unwrap().is_empty());
    assert!(store.save_ui_state(Id::new_v4(), &state).is_err());
    assert!(store.ui_state(Id::new_v4()).is_err());
    assert!(
        store
            .save_ui_state(
                workspace,
                &serde_json::json!("x".repeat(MAX_PAYLOAD_BYTES + 1))
            )
            .is_err()
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.ui_state(workspace).unwrap(), Some(state));
}

#[test]
fn diagram_positions_support_relationships_beyond_element_limit() {
    let (mut store, workspace, request, result) = setup();
    let mut view = ViewState {
        id: Id::new_v4(),
        document_id: result.document_id,
        kind: ViewKind::Diagram,
        filter: String::new(),
        hidden: BTreeSet::new(),
        positions: (0..MAX_ELEMENTS)
            .map(|_| (Id::new_v4(), Position { x: 0, y: 0 }))
            .collect(),
        selection: None,
        draft: String::new(),
    };
    view.positions
        .insert(request.relationships[0].id, Position { x: 1, y: 1 });
    store.save_view(workspace, &view).unwrap();
    assert_eq!(
        store.views(workspace).unwrap()[0].positions.len(),
        MAX_ELEMENTS + 1
    );
}

#[test]
fn schema_one_migration_preserves_existing_content() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let published = store.publish(workspace, &publication()).unwrap();
    let before = store.document(workspace, published.document_id).unwrap();
    drop(store);
    // Removing the sole schema-2 table reconstructs the schema-1 database layout.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE private_ui; PRAGMA user_version=1;")
        .unwrap();
    drop(connection);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.document(workspace, published.document_id).unwrap(),
        before
    );
    assert_eq!(store.ui_state(workspace).unwrap(), None);
    store
        .save_ui_state(workspace, &serde_json::json!({"tabs": []}))
        .unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    connection.pragma_update(None, "user_version", 3).unwrap();
    assert!(matches!(Store::open(&path), Err(Error::SchemaVersion(3))));
}

fn edit(request: &PublishRequest, result: &PublishResult, revision: u64) -> PublishRequest {
    let mut update = request.clone();
    update.request_id = Id::new_v4();
    update.document_id = Some(result.document_id);
    update.expected_revision = Some(revision);
    update
}

#[test]
fn publication_replays_after_later_edits_and_rejects_payload_reuse() {
    let (mut store, workspace, request, result) = setup();
    let mut update = edit(&request, &result, 1);
    update.elements[0].body = "Changed".into();
    store.publish(workspace, &update).unwrap();
    assert_eq!(store.publish(workspace, &request).unwrap(), result);
    let mut changed = request.clone();
    changed.title = "Changed payload".into();
    assert!(matches!(
        store.publish(workspace, &changed),
        Err(Error::RequestConflict)
    ));
    assert_eq!(
        store
            .document(workspace, result.document_id)
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn stale_revision_and_invalid_refs_roll_back_without_consuming_request() {
    let (mut store, workspace, request, result) = setup();
    let mut update = edit(&request, &result, 0);
    assert!(matches!(
        store.publish(workspace, &update),
        Err(Error::Conflict { current: 1 })
    ));
    update.expected_revision = Some(1);
    update.relationships[0].to = Id::new_v4();
    update.elements[0].body = "Must roll back".into();
    assert!(matches!(
        store.publish(workspace, &update),
        Err(Error::Invalid(_))
    ));
    let current = store.document(workspace, result.document_id).unwrap();
    assert_eq!(current.elements[&request.elements[0].id].body, "Original");
    assert_eq!(current.revision, 1);
    update.relationships[0].to = request.elements[1].id;
    assert_eq!(store.publish(workspace, &update).unwrap().revision, 2);
}

#[test]
fn acceptance_tracks_semantics_and_undo_preserves_unrelated_edits() {
    let (mut store, workspace, request, result) = setup();
    let id = request.elements[0].id;
    let accepted = store
        .apply(
            workspace,
            result.document_id,
            1,
            &ContentAction::Accept { ids: vec![id] },
        )
        .unwrap();
    let mut update = edit(&request, &result, 2);
    update.elements = vec![request.elements[1].clone()];
    update.elements[0].body = "Unrelated edit".into();
    update.relationships.clear();
    update.groups.clear();
    store.publish(workspace, &update).unwrap();
    let restored = store
        .undo(workspace, result.document_id, 3, accepted.undo_id)
        .unwrap();
    assert_eq!(restored.elements[&id].accepted_revision, None);
    assert_eq!(
        restored.elements[&request.elements[1].id].body,
        "Unrelated edit"
    );
    store
        .apply(
            workspace,
            result.document_id,
            4,
            &ContentAction::Accept { ids: vec![id] },
        )
        .unwrap();
    let mut update = edit(&request, &result, 5);
    update.elements[0].body = "New semantics".into();
    store.publish(workspace, &update).unwrap();
    let current = store.document(workspace, result.document_id).unwrap();
    assert_eq!(current.elements[&id].accepted_revision, Some(1));
    assert_eq!(current.elements[&id].semantic_revision, 6);
}

#[test]
fn conflicting_undo_does_not_overwrite_agent_edit() {
    let (mut store, workspace, request, result) = setup();
    let id = request.elements[0].id;
    let accepted = store
        .apply(
            workspace,
            result.document_id,
            1,
            &ContentAction::Accept { ids: vec![id] },
        )
        .unwrap();
    let mut update = edit(&request, &result, 2);
    update.elements[0].title = "Changed".into();
    store.publish(workspace, &update).unwrap();
    assert!(matches!(
        store.undo(workspace, result.document_id, 3, accepted.undo_id),
        Err(Error::UndoConflict)
    ));
    assert_eq!(
        store
            .document(workspace, result.document_id)
            .unwrap()
            .elements[&id]
            .title,
        "Changed"
    );
}

#[test]
fn archive_cascades_edges_and_undo_restores_prior_acceptance() {
    let (mut store, workspace, request, result) = setup();
    let id = request.elements[0].id;
    let edge = request.relationships[0].id;
    store
        .apply(
            workspace,
            result.document_id,
            1,
            &ContentAction::Accept {
                ids: vec![id, edge],
            },
        )
        .unwrap();
    let archived = store
        .apply(
            workspace,
            result.document_id,
            2,
            &ContentAction::Archive { ids: vec![id] },
        )
        .unwrap();
    let document = store.document(workspace, result.document_id).unwrap();
    assert!(document.elements[&id].archived);
    assert!(document.relationships[&edge].archived);
    assert!(!document.elements[&request.elements[1].id].archived);
    assert!(!document.groups[&request.groups[0].id].archived);
    let update = edit(&request, &result, 3);
    assert!(matches!(
        store.publish(workspace, &update),
        Err(Error::Invalid(_))
    ));
    let restored = store
        .undo(workspace, result.document_id, 3, archived.undo_id)
        .unwrap();
    assert!(!restored.elements[&id].archived);
    assert!(!restored.relationships[&edge].archived);
    assert_eq!(restored.elements[&id].accepted_revision, Some(1));
    assert_eq!(restored.relationships[&edge].accepted_revision, Some(1));
}

#[test]
fn archiving_group_changes_only_group_record() {
    let (mut store, workspace, request, result) = setup();
    store
        .apply(
            workspace,
            result.document_id,
            1,
            &ContentAction::Archive {
                ids: vec![request.groups[0].id],
            },
        )
        .unwrap();
    let document = store.document(workspace, result.document_id).unwrap();
    assert!(document.groups[&request.groups[0].id].archived);
    assert!(document.elements.values().all(|v| !v.archived));
    assert!(document.relationships.values().all(|v| !v.archived));
}

#[test]
fn identical_conversation_ids_under_different_agents_are_separate_workspaces() {
    let mut store = Store::open(":memory:").unwrap();
    let conversation = Id::new_v4().to_string();
    let codex = store.workspace(&key(&conversation)).unwrap().id;
    let mut claude_key = key(&conversation);
    claude_key.agent = AgentKind::Claude;
    let claude = store.workspace(&claude_key).unwrap().id;
    assert_ne!(codex, claude);
    let result = store.publish(codex, &publication()).unwrap();
    assert!(store.documents(claude).unwrap().is_empty());
    assert!(store.document(claude, result.document_id).is_err());
    assert_eq!(store.workspace(&claude_key).unwrap().id, claude);
}

#[test]
fn workspace_identity_and_private_views_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("exact-thread")).unwrap().id;
    let request = publication();
    let result = store.publish(workspace, &request).unwrap();
    let other = store.workspace(&key("other-thread")).unwrap().id;
    let mut other_agent_key = key("exact-thread");
    other_agent_key.agent = AgentKind::Claude;
    assert_ne!(store.workspace(&other_agent_key).unwrap().id, workspace);
    let mut other_project_key = key("exact-thread");
    other_project_key.project = "/other-project".into();
    assert_ne!(store.workspace(&other_project_key).unwrap().id, workspace);
    assert!(store.document(other, result.document_id).is_err());
    let view = ViewState {
        id: Id::new_v4(),
        document_id: result.document_id,
        kind: ViewKind::Diagram,
        selection: Some(Selection {
            document_id: result.document_id,
            revision: 1,
            ids: vec![request.elements[0].id],
        }),
        positions: [(request.elements[0].id, Position { x: 4, y: 8 })]
            .into_iter()
            .collect(),
        hidden: BTreeSet::new(),
        filter: "private-filter".into(),
        draft: "private-question".into(),
    };
    store.save_view(workspace, &view).unwrap();
    let inspected = serde_json::to_string(&store.documents(workspace).unwrap()).unwrap();
    assert!(!inspected.contains("private-question"));
    assert!(!inspected.contains("private-filter"));
    store
        .publish(workspace, &edit(&request, &result, 1))
        .unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.workspace(&key("exact-thread")).unwrap().id, workspace);
    assert_eq!(store.views(workspace).unwrap(), vec![view]);
    assert!(store.views(other).unwrap().is_empty());
    assert_eq!(
        store
            .document(workspace, result.document_id)
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn question_snapshot_and_delivery_persist_without_resend() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread")).unwrap().id;
    let publication = publication();
    let result = store.publish(workspace, &publication).unwrap();
    let request = PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 7,
        selection: Selection {
            document_id: result.document_id,
            revision: 1,
            ids: vec![publication.relationships[0].id],
        },
        text: "Explain this flow".into(),
        mode: DeliveryMode::Start,
    };
    let prepared = store.prepare_question(workspace, &request).unwrap();
    assert_eq!(prepared.snapshot.elements.len(), 2);
    let mut edit = edit(&publication, &result, 1);
    edit.elements[0].body = "Changed later".into();
    store.publish(workspace, &edit).unwrap();
    assert_eq!(
        store.prepare_question(workspace, &request).unwrap(),
        prepared
    );
    store
        .update_delivery(
            workspace,
            request.request_id,
            7,
            &DeliveryOutcome::Pending,
            DeliveryOutcome::Submitted,
        )
        .unwrap();
    assert!(
        store
            .update_delivery(
                workspace,
                request.request_id,
                7,
                &DeliveryOutcome::Pending,
                DeliveryOutcome::Submitted
            )
            .is_err()
    );
    assert_eq!(
        store
            .invalidate_generation(workspace, 7, "disconnected")
            .unwrap(),
        1
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    let question = store.question(workspace, request.request_id).unwrap();
    assert_eq!(
        question.snapshot.elements[&publication.elements[0].id].body,
        "Original"
    );
    assert!(matches!(question.outcome, DeliveryOutcome::Unknown { .. }));
    assert_eq!(store.questions(workspace).unwrap().len(), 1);
}

#[test]
fn shared_questions_survive_retirement_and_resume() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("shared-thread")).unwrap().id;
    let publication = publication();
    let result = store.publish(workspace, &publication).unwrap();
    let mut request = PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 1,
        selection: Selection {
            document_id: result.document_id,
            revision: 1,
            ids: vec![publication.elements[0].id],
        },
        text: "Explain the selected content".into(),
        mode: DeliveryMode::Share,
    };
    let shared = store.prepare_question(workspace, &request).unwrap();
    request.request_id = Id::new_v4();
    request.mode = DeliveryMode::Start;
    let native = store.prepare_question(workspace, &request).unwrap();
    assert_eq!(
        store
            .invalidate_generation(workspace, 1, "disconnected")
            .unwrap(),
        1
    );
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.workspace(&key("shared-thread")).unwrap().id,
        workspace
    );
    assert_eq!(
        store.question(workspace, shared.request_id).unwrap(),
        shared
    );
    assert!(matches!(
        store
            .question(workspace, native.request_id)
            .unwrap()
            .outcome,
        DeliveryOutcome::Failed { .. }
    ));
}

#[test]
fn stale_selection_wrong_generation_and_illegal_delivery_transition_fail() {
    let (mut store, workspace, publication, result) = setup();
    let mut request = PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 1,
        selection: Selection {
            document_id: result.document_id,
            revision: 0,
            ids: vec![publication.elements[0].id],
        },
        text: "Why?".into(),
        mode: DeliveryMode::Queue,
    };
    assert!(matches!(
        store.prepare_question(workspace, &request),
        Err(Error::Conflict { current: 1 })
    ));
    request.selection.revision = 1;
    store.prepare_question(workspace, &request).unwrap();
    request.text = "Different payload".into();
    assert!(matches!(
        store.prepare_question(workspace, &request),
        Err(Error::RequestConflict)
    ));
    assert!(
        store
            .update_delivery(
                workspace,
                request.request_id,
                2,
                &DeliveryOutcome::Pending,
                DeliveryOutcome::Submitted
            )
            .is_err()
    );
    assert!(
        store
            .update_delivery(
                workspace,
                request.request_id,
                1,
                &DeliveryOutcome::Pending,
                DeliveryOutcome::Started {
                    turn_id: "turn".into()
                }
            )
            .is_err()
    );
    store
        .update_delivery(
            workspace,
            request.request_id,
            1,
            &DeliveryOutcome::Pending,
            DeliveryOutcome::Submitted,
        )
        .unwrap();
    let queued = DeliveryOutcome::Queued {
        reason: Some("native queue receipt".into()),
    };
    store
        .update_delivery(
            workspace,
            request.request_id,
            1,
            &DeliveryOutcome::Submitted,
            queued.clone(),
        )
        .unwrap();
    store
        .update_delivery(
            workspace,
            request.request_id,
            1,
            &queued,
            DeliveryOutcome::Started {
                turn_id: "native-turn".into(),
            },
        )
        .unwrap();
    assert_eq!(
        store.invalidate_generation(workspace, 1, "switch").unwrap(),
        0
    );
}

#[test]
fn concurrent_connections_reject_stale_writes_and_share_replay() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut first = Store::open(&path).unwrap();
    let mut second = Store::open(&path).unwrap();
    let workspace = first.workspace(&key("thread")).unwrap().id;
    let request = publication();
    let result = first.publish(workspace, &request).unwrap();
    assert_eq!(second.publish(workspace, &request).unwrap(), result);
    let mut update = edit(&request, &result, 1);
    first.publish(workspace, &update).unwrap();
    update.request_id = Id::new_v4();
    assert!(matches!(
        second.publish(workspace, &update),
        Err(Error::Conflict { current: 2 })
    ));
    assert_eq!(
        second
            .document(workspace, result.document_id)
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn bounds_duplicate_ids_and_terminal_controls_are_handled() {
    let (mut store, workspace, request, result) = setup();
    let mut update = edit(&request, &result, 1);
    update.elements[0].body = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(matches!(
        store.publish(workspace, &update),
        Err(Error::Invalid(_))
    ));
    update.elements[0].body.clear();
    update.elements.push(update.elements[0].clone());
    assert!(matches!(
        store.publish(workspace, &update),
        Err(Error::Invalid(_))
    ));
    assert_eq!(terminal_safe("a\x1b[2J\r\nb"), "a\\u{1b}[2J\\u{d}\nb");
    assert_eq!(
        store
            .document(workspace, result.document_id)
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn independent_views_do_not_change_semantic_acceptance() {
    let (mut store, workspace, request, result) = setup();
    let id = request.elements[0].id;
    store
        .apply(
            workspace,
            result.document_id,
            1,
            &ContentAction::Accept { ids: vec![id] },
        )
        .unwrap();
    let mut view = ViewState {
        id: Id::new_v4(),
        document_id: result.document_id,
        kind: ViewKind::List,
        selection: None,
        positions: BTreeMap::new(),
        hidden: BTreeSet::new(),
        filter: String::new(),
        draft: "first".into(),
    };
    store.save_view(workspace, &view).unwrap();
    view.id = Id::new_v4();
    view.draft = "second".into();
    view.kind = ViewKind::Diagram;
    store.save_view(workspace, &view).unwrap();
    let document = store.document(workspace, result.document_id).unwrap();
    assert_eq!(document.revision, 2);
    assert_eq!(
        document.elements[&id].accepted_revision,
        Some(document.elements[&id].semantic_revision)
    );
    assert_eq!(store.views(workspace).unwrap().len(), 2);
}

#[test]
fn publication_result_failure_rolls_back_content_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread")).unwrap().id;
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_result BEFORE INSERT ON publications BEGIN SELECT RAISE(ABORT, 'injected publication result failure'); END;").unwrap();
    let request = publication();
    assert!(matches!(
        store.publish(workspace, &request),
        Err(Error::Sql(_))
    ));
    assert!(store.documents(workspace).unwrap().is_empty());
    connection
        .execute_batch("DROP TRIGGER fail_result")
        .unwrap();
    assert_eq!(store.publish(workspace, &request).unwrap().revision, 1);
}

#[test]
fn simultaneous_writers_commit_exactly_one_expected_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread")).unwrap().id;
    let request = publication();
    let result = store.publish(workspace, &request).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let barrier = barrier.clone();
            let path = path.clone();
            let mut update = edit(&request, &result, 1);
            update.elements[0].body = format!("Writer {index}");
            std::thread::spawn(move || {
                let mut store = Store::open(path).unwrap();
                barrier.wait();
                store.publish(workspace, &update)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::Conflict { current: 2 })))
            .count(),
        1
    );
    assert_eq!(
        store
            .document(workspace, result.document_id)
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn unknown_delivery_requires_new_request_for_retry() {
    let (mut store, workspace, publication, result) = setup();
    let mut request = PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 1,
        selection: Selection {
            document_id: result.document_id,
            revision: 1,
            ids: vec![publication.elements[0].id],
        },
        text: "Explain".into(),
        mode: DeliveryMode::Start,
    };
    store.prepare_question(workspace, &request).unwrap();
    store
        .update_delivery(
            workspace,
            request.request_id,
            1,
            &DeliveryOutcome::Pending,
            DeliveryOutcome::Submitted,
        )
        .unwrap();
    let unknown = DeliveryOutcome::Unknown {
        reason: "timeout".into(),
    };
    store
        .update_delivery(
            workspace,
            request.request_id,
            1,
            &DeliveryOutcome::Submitted,
            unknown.clone(),
        )
        .unwrap();
    assert!(
        store
            .update_delivery(
                workspace,
                request.request_id,
                1,
                &unknown,
                DeliveryOutcome::Pending
            )
            .is_err()
    );
    assert!(
        store
            .update_delivery(
                workspace,
                request.request_id,
                1,
                &unknown,
                DeliveryOutcome::Submitted
            )
            .is_err()
    );
    let original_id = request.request_id;
    request.request_id = Id::new_v4();
    request.generation = 2;
    assert_eq!(
        store.prepare_question(workspace, &request).unwrap().outcome,
        DeliveryOutcome::Pending
    );
    assert_eq!(
        store.question(workspace, original_id).unwrap().outcome,
        unknown
    );
}

#[test]
fn atomic_send_replay_preserves_a_newer_private_draft() {
    let (mut store, workspace, publication, result) = setup();
    let request = PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 1,
        selection: Selection {
            document_id: result.document_id,
            revision: 1,
            ids: vec![publication.elements[0].id],
        },
        text: "Explain this".into(),
        mode: DeliveryMode::Share,
    };
    let mut view = ViewState {
        id: Id::new_v4(),
        document_id: result.document_id,
        kind: ViewKind::List,
        selection: Some(request.selection.clone()),
        positions: BTreeMap::new(),
        hidden: BTreeSet::new(),
        filter: String::new(),
        draft: request.text.clone(),
    };
    store.save_view(workspace, &view).unwrap();
    let sent = store
        .prepare_question_and_clear_draft(workspace, &request, view.id)
        .unwrap();
    assert!(store.views(workspace).unwrap()[0].draft.is_empty());
    view.draft = "New private draft".into();
    store.save_view(workspace, &view).unwrap();
    assert_eq!(
        store
            .prepare_question_and_clear_draft(workspace, &request, view.id)
            .unwrap(),
        sent
    );
    assert_eq!(
        store.views(workspace).unwrap()[0].draft,
        "New private draft"
    );
    assert_eq!(store.questions(workspace).unwrap().len(), 1);
    let mut stale = request.clone();
    stale.request_id = Id::new_v4();
    assert!(
        store
            .prepare_question_and_clear_draft(workspace, &stale, view.id)
            .is_err()
    );
    assert_eq!(store.questions(workspace).unwrap().len(), 1);
    assert_eq!(
        store.views(workspace).unwrap()[0].draft,
        "New private draft"
    );
}
