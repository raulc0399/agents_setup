use prism::actions::*;
use prism::model::*;
use prism::store::Store;
use prism_core::graph::GraphLayout;
use std::collections::BTreeSet;

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
            manual: false,
        }],
        groups: vec![GroupInput {
            id: Id::new_v4(),
            title: "Both".into(),
            members: [a, b].into_iter().collect(),
        }],
        lanes: None,
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
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.document(workspace, published.document_id).unwrap(),
        before
    );
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    // Schema 2 keeps the table an older binary expects, though nothing uses it any more.
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM private_ui", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
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
fn workspace_identity_survives_restart() {
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
    store
        .publish(workspace, &edit(&request, &result, 1))
        .unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.workspace(&key("exact-thread")).unwrap().id, workspace);
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

fn element_input(title: &str) -> ElementInput {
    ElementInput {
        id: Id::new_v4(),
        title: title.into(),
        body: String::new(),
        kind: ElementKind::Step,
        source: None,
    }
}

fn link_input(from: Id, to: Id) -> RelationshipInput {
    RelationshipInput {
        id: Id::new_v4(),
        from,
        to,
        label: "next".into(),
        direction: Direction::Directed,
        manual: false,
    }
}

/// Record IDs in the order the list and the diagram number them.
fn numbered(store: &Store, workspace: Id, document: Id) -> Vec<Id> {
    store.document(workspace, document).unwrap().list_order()
}

/// Three elements, a link and a group: numbered in that order on the first publication.
fn three_steps(store: &mut Store, workspace: Id) -> (PublishRequest, PublishResult, Vec<Id>) {
    let mut request = publication();
    request.elements = ["First", "Second", "Third"]
        .into_iter()
        .map(element_input)
        .collect();
    let ids: Vec<Id> = request.elements.iter().map(|item| item.id).collect();
    request.relationships = vec![link_input(ids[0], ids[1])];
    request.groups[0].members = ids.iter().copied().collect();
    let result = store.publish(workspace, &request).unwrap();
    let mut published = ids;
    published.push(request.relationships[0].id);
    published.push(request.groups[0].id);
    assert_eq!(numbered(store, workspace, result.document_id), published);
    (request, result, published)
}

#[test]
fn publishing_never_renumbers_existing_records() {
    let mut store = Store::open(":memory:").unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let (request, result, published) = three_steps(&mut store, workspace);
    let document = result.document_id;
    let first = request.elements[0].id;
    store
        .apply(
            workspace,
            document,
            1,
            &ContentAction::Accept { ids: vec![first] },
        )
        .unwrap();

    // Revising only the second element keeps it in its place.
    let mut revision = edit(&request, &result, 2);
    revision.elements = vec![request.elements[1].clone()];
    revision.relationships.clear();
    revision.groups.clear();
    revision.elements[0].body = "Revised".into();
    store.publish(workspace, &revision).unwrap();
    assert_eq!(numbered(&store, workspace, document), published);

    // Sending everything in reverse order renumbers nothing and keeps acceptance.
    let mut reversed = edit(&request, &result, 3);
    reversed.elements.reverse();
    store.publish(workspace, &reversed).unwrap();
    assert_eq!(numbered(&store, workspace, document), published);
    let stored = store.document(workspace, document).unwrap();
    assert_eq!(stored.elements[&first].accepted_revision, Some(1));
    assert_eq!(stored.elements[&first].semantic_revision, 1);

    // A new element in the middle of a request and a new link go after the last record.
    let mut added = edit(&request, &result, 4);
    let new = element_input("New");
    added.elements.insert(1, new.clone());
    let new_link = link_input(new.id, request.elements[0].id);
    added.relationships.insert(0, new_link.clone());
    store.publish(workspace, &added).unwrap();
    let mut expected = published.clone();
    expected.extend([new.id, new_link.id]);
    assert_eq!(numbered(&store, workspace, document), expected);
}

#[test]
fn a_new_element_after_an_archived_last_one_takes_a_new_position() {
    let mut store = Store::open(":memory:").unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let (request, result, published) = three_steps(&mut store, workspace);
    let document = result.document_id;
    let group = request.groups[0].id;
    store
        .apply(
            workspace,
            document,
            1,
            &ContentAction::Archive { ids: vec![group] },
        )
        .unwrap();
    let archived = store.document(workspace, document).unwrap().groups[&group].order;

    let mut added = edit(&request, &result, 2);
    let new = element_input("New");
    added.elements = vec![new.clone()];
    added.relationships.clear();
    added.groups.clear();
    store.publish(workspace, &added).unwrap();
    let stored = store.document(workspace, document).unwrap();
    assert!(Some(stored.elements[&new.id].order) > archived);
    let mut expected = published;
    expected.push(new.id);
    assert_eq!(stored.list_order(), expected);
}

#[test]
fn a_document_stored_before_positions_keeps_its_numbering_and_appends() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let (request, result, _) = three_steps(&mut store, workspace);
    let document = result.document_id;
    drop(store);

    // As stored before: no position on links and groups, every element at the default 0.
    let connection = rusqlite::Connection::open(&path).unwrap();
    let content: String = connection
        .query_row(
            "SELECT content FROM documents WHERE id=?1",
            [document.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let mut stored: serde_json::Value = serde_json::from_str(&content).unwrap();
    for records in ["relationships", "groups"] {
        for record in stored[records].as_object_mut().unwrap().values_mut() {
            record.as_object_mut().unwrap().remove("order");
        }
    }
    for record in stored["elements"].as_object_mut().unwrap().values_mut() {
        record["order"] = 0.into();
    }
    connection
        .execute(
            "UPDATE documents SET content=?2 WHERE id=?1",
            rusqlite::params![document.to_string(), stored.to_string()],
        )
        .unwrap();
    drop(connection);

    // Today's numbering: elements by order and ID, then links by ID, then groups by ID.
    let mut store = Store::open(&path).unwrap();
    let before = store.document(workspace, document).unwrap();
    let mut today: Vec<Id> = before.elements.keys().copied().collect();
    today.push(request.relationships[0].id);
    today.push(request.groups[0].id);
    assert_eq!(before.list_order(), today);

    let mut added = edit(&request, &result, 1);
    let new = element_input("New");
    added.elements = vec![new.clone()];
    added.relationships = vec![link_input(new.id, request.elements[0].id)];
    added.groups.clear();
    store.publish(workspace, &added).unwrap();
    today.extend([new.id, added.relationships[0].id]);
    assert_eq!(numbered(&store, workspace, document), today);
}

#[test]
fn undo_records_are_listed_in_order_and_conflicts_discard_them() {
    let (mut store, workspace, request, result) = setup();
    let id = request.elements[0].id;
    let document = result.document_id;
    let first = store
        .apply(
            workspace,
            document,
            1,
            &ContentAction::Accept { ids: vec![id] },
        )
        .unwrap();
    let second = store
        .apply(
            workspace,
            document,
            2,
            &ContentAction::Archive { ids: vec![id] },
        )
        .unwrap();
    assert_eq!(
        store.undo_records(workspace, document).unwrap(),
        vec![first.undo_id, second.undo_id]
    );
    store.undo(workspace, document, 3, second.undo_id).unwrap();
    assert_eq!(
        store.undo_records(workspace, document).unwrap(),
        vec![first.undo_id]
    );
    let mut update = edit(&request, &result, 4);
    update.elements[0].title = "Changed".into();
    store.publish(workspace, &update).unwrap();
    assert!(matches!(
        store.undo(workspace, document, 5, first.undo_id),
        Err(Error::UndoConflict)
    ));
    // A record that can never apply again is discarded so it cannot block later undo.
    assert!(store.undo_records(workspace, document).unwrap().is_empty());
}

#[test]
fn close_and_reopen_check_the_revision_and_undo_reopens() {
    let (mut store, workspace, _, result) = setup();
    let document = result.document_id;
    assert!(matches!(
        store.apply(workspace, document, 0, &ContentAction::Close {}),
        Err(Error::Conflict { current: 1 })
    ));
    let closed = store
        .apply(workspace, document, 1, &ContentAction::Close {})
        .unwrap();
    assert_eq!(closed.revision, 2);
    assert!(store.is_closed(workspace, document).unwrap());
    assert_eq!(store.closed_documents(workspace).unwrap(), vec![document]);
    // The content stays in the store and in the listing.
    assert_eq!(store.documents(workspace).unwrap().len(), 1);
    assert!(matches!(
        store.apply(workspace, document, 2, &ContentAction::Close {}),
        Err(Error::Invalid(message)) if message == "document is already closed"
    ));

    let undone = store.undo_latest(workspace, document, 2).unwrap();
    assert_eq!(undone.revision, 3);
    assert!(!store.is_closed(workspace, document).unwrap());
    assert!(store.closed_documents(workspace).unwrap().is_empty());
    assert!(matches!(
        store.apply(workspace, document, 3, &ContentAction::Reopen {}),
        Err(Error::Invalid(message)) if message == "document is already open"
    ));

    store
        .apply(workspace, document, 3, &ContentAction::Close {})
        .unwrap();
    store
        .apply(workspace, document, 4, &ContentAction::Reopen {})
        .unwrap();
    assert!(!store.is_closed(workspace, document).unwrap());
    // Undoing the reopen closes the document again.
    store.undo_latest(workspace, document, 5).unwrap();
    assert!(store.is_closed(workspace, document).unwrap());
}

#[test]
fn publishing_to_a_closed_document_reopens_it_and_consumes_the_close_undo() {
    let (mut store, workspace, request, result) = setup();
    let document = result.document_id;
    let element = request.elements[0].id;
    store
        .apply(
            workspace,
            document,
            1,
            &ContentAction::Accept { ids: vec![element] },
        )
        .unwrap();
    store
        .apply(workspace, document, 2, &ContentAction::Close {})
        .unwrap();
    let revised = store
        .publish(workspace, &edit(&request, &result, 3))
        .unwrap();
    assert_eq!(revised.revision, 4);
    assert!(!store.is_closed(workspace, document).unwrap());
    // The close can no longer be undone: one undo reaches the acceptance before it.
    assert_eq!(store.undo_records(workspace, document).unwrap().len(), 1);
    let undone = store.undo_latest(workspace, document, 4).unwrap();
    assert_eq!(undone.elements[&element].accepted_revision, None);
    assert!(!store.is_closed(workspace, document).unwrap());
}

#[test]
fn undo_records_written_before_documents_could_close_still_undo() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let request = publication();
    let result = store.publish(workspace, &request).unwrap();
    let element = request.elements[0].id;
    store
        .apply(
            workspace,
            result.document_id,
            1,
            &ContentAction::Archive { ids: vec![element] },
        )
        .unwrap();
    drop(store);
    // The record as an older binary wrote it: no closed state before and after.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "UPDATE undo_records SET content=json_remove(content, '$.before_closed', '$.after_closed');",
        )
        .unwrap();
    let content: String = connection
        .query_row("SELECT content FROM undo_records", [], |row| row.get(0))
        .unwrap();
    assert!(!content.contains("closed"), "{content}");
    drop(connection);
    let mut store = Store::open(&path).unwrap();
    let undone = store.undo_latest(workspace, result.document_id, 2).unwrap();
    assert!(!undone.elements[&element].archived);
    assert!(!store.is_closed(workspace, result.document_id).unwrap());
}

#[test]
fn questions_and_answers_on_a_closed_document_stay_and_return_on_reopen() {
    let (mut store, workspace, request, result) = setup();
    let document = result.document_id;
    let question = PrepareQuestion {
        request_id: Id::new_v4(),
        generation: 1,
        selection: Selection {
            document_id: document,
            revision: 1,
            ids: vec![request.elements[0].id],
        },
        text: "Why this source?".into(),
        mode: DeliveryMode::Share,
    };
    store.prepare_question(workspace, &question).unwrap();
    store
        .apply(workspace, document, 1, &ContentAction::Close {})
        .unwrap();
    store
        .answer_question(workspace, question.request_id, "It is stable.")
        .unwrap();
    store
        .apply(workspace, document, 2, &ContentAction::Reopen {})
        .unwrap();
    let shared = store.shared_questions(workspace).unwrap();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].answer.as_deref(), Some("It is stable."));
}

#[test]
fn closed_column_is_added_to_an_older_store_and_kept_by_older_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let mut store = Store::open(&path).unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let published = store.publish(workspace, &publication()).unwrap();
    drop(store);
    // A schema-2 store an older binary created has no `closed` column.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("ALTER TABLE documents DROP COLUMN closed;")
        .unwrap();
    drop(connection);
    let mut store = Store::open(&path).unwrap();
    assert!(!store.is_closed(workspace, published.document_id).unwrap());
    store
        .apply(
            workspace,
            published.document_id,
            1,
            &ContentAction::Close {},
        )
        .unwrap();
    drop(store);
    // An older binary's write: its upsert names only the columns it knows.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO documents(workspace_id, id, revision, content)
             SELECT workspace_id, id, revision + 1, content FROM documents WHERE true
             ON CONFLICT(workspace_id, id) DO UPDATE SET revision=excluded.revision, content=excluded.content",
            [],
        )
        .unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(connection);
    let store = Store::open(&path).unwrap();
    assert!(store.is_closed(workspace, published.document_id).unwrap());
}

#[test]
fn the_lane_orientation_is_chosen_at_the_first_publication_with_two_lanes_and_kept() {
    let mut store = Store::open(":memory:").unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let request = publication();
    let result = store.publish(workspace, &request).unwrap();
    let stored = |store: &Store| store.document(workspace, result.document_id).unwrap();
    assert_eq!(
        stored(&store).lanes,
        None,
        "one lane: nothing to choose yet"
    );

    // The second lane brings the automatic choice, stored with the document.
    let mut split = edit(&request, &result, 1);
    let (a, b) = (request.elements[0].id, request.elements[1].id);
    split.groups[0].members = [a].into_iter().collect();
    split.groups.push(GroupInput {
        id: Id::new_v4(),
        title: "Sink side".into(),
        members: [b].into_iter().collect(),
    });
    store.publish(workspace, &split).unwrap();
    let chosen = stored(&store).lanes;
    assert!(chosen.is_some());
    let mut document = stored(&store);
    document.lanes = None;
    assert_eq!(GraphLayout::automatic_lanes(&document), chosen);

    // Omitted (or null) keeps it, whatever the content does; `auto` chooses again; rows and
    // columns set it.
    let other = match chosen.unwrap() {
        Lanes::Rows => Lanes::Columns,
        Lanes::Columns => Lanes::Rows,
    };
    let mut set = edit(&split, &result, 2);
    set.lanes = Some(match other {
        Lanes::Rows => LaneChoice::Rows,
        Lanes::Columns => LaneChoice::Columns,
    });
    store.publish(workspace, &set).unwrap();
    assert_eq!(stored(&store).lanes, Some(other));
    let mut kept = edit(&split, &result, 3);
    kept.elements[0].body = "Changed".into();
    store.publish(workspace, &kept).unwrap();
    assert_eq!(stored(&store).lanes, Some(other));
    let mut null = serde_json::to_value(edit(&split, &result, 4)).unwrap();
    null["lanes"] = serde_json::Value::Null;
    let null: PublishRequest = serde_json::from_value(null).unwrap();
    assert_eq!(null.lanes, None);
    store.publish(workspace, &null).unwrap();
    assert_eq!(stored(&store).lanes, Some(other));
    let mut auto = edit(&split, &result, 5);
    auto.lanes = Some(LaneChoice::Auto);
    store.publish(workspace, &auto).unwrap();
    assert_eq!(stored(&store).lanes, chosen);
    assert_eq!(
        serde_json::to_value(LaneChoice::Auto).unwrap(),
        serde_json::json!("auto")
    );

    // Absent, it is not written: documents and requests from before it read and hash the same.
    let request_json = serde_json::to_value(&request).unwrap();
    assert!(request_json.get("lanes").is_none());
    let (store, workspace, _, result) = setup();
    let document = store.document(workspace, result.document_id).unwrap();
    let json = serde_json::to_value(&document).unwrap();
    assert!(json.get("lanes").is_none());
    assert_eq!(serde_json::from_value::<Document>(json).unwrap(), document);
    let mut columns = serde_json::to_value(&document).unwrap();
    columns["lanes"] = "columns".into();
    let read: Document = serde_json::from_value(columns).unwrap();
    assert_eq!(read.lanes, Some(Lanes::Columns));
}

#[test]
fn a_diagram_built_arrow_by_arrow_keeps_its_lane_orientation() {
    let mut store = Store::open(":memory:").unwrap();
    let workspace = store.workspace(&key("thread-1")).unwrap().id;
    let mut seed: u64 = 7;
    let mut pick = |below: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize % below
    };
    for _ in 0..25 {
        // Three lanes of one to three steps, then up to nine arrows, one publication each.
        let sizes: Vec<usize> = (0..3).map(|_| 1 + pick(3)).collect();
        let elements: Vec<ElementInput> = (0..sizes.iter().sum::<usize>())
            .map(|index| ElementInput {
                id: Id::new_v4(),
                title: format!("Step {index}"),
                body: "Does a thing".into(),
                kind: ElementKind::Step,
                source: None,
            })
            .collect();
        let mut start = 0;
        let groups = sizes
            .iter()
            .map(|size| {
                let members = elements[start..start + size]
                    .iter()
                    .map(|item| item.id)
                    .collect();
                start += size;
                GroupInput {
                    id: Id::new_v4(),
                    title: format!("Lane {start}"),
                    members,
                }
            })
            .collect();
        let mut request = PublishRequest {
            request_id: Id::new_v4(),
            document_id: None,
            expected_revision: None,
            title: "Built".into(),
            elements: elements.clone(),
            relationships: vec![],
            groups,
            lanes: None,
        };
        let result = store.publish(workspace, &request).unwrap();
        let first = store.document(workspace, result.document_id).unwrap().lanes;
        assert!(first.is_some(), "three lanes: chosen at once");
        let mut ends = BTreeSet::new();
        for revision in 1..=9 {
            let (from, to) = (pick(elements.len()), pick(elements.len()));
            if from == to || !ends.insert((from, to)) {
                continue;
            }
            request.request_id = Id::new_v4();
            request.document_id = Some(result.document_id);
            request.expected_revision = Some(
                store
                    .document(workspace, result.document_id)
                    .unwrap()
                    .revision,
            );
            request.relationships.push(RelationshipInput {
                id: Id::new_v4(),
                from: elements[from].id,
                to: elements[to].id,
                label: "next".into(),
                direction: Direction::Directed,
                manual: false,
            });
            store.publish(workspace, &request).unwrap();
            let document = store.document(workspace, result.document_id).unwrap();
            assert_eq!(document.lanes, first, "revision {revision}");
            // Shown with items hidden, the diagram keeps it too.
            let visible: BTreeSet<Id> = document
                .elements
                .keys()
                .chain(document.relationships.keys())
                .chain(document.groups.keys())
                .copied()
                .filter(|_| pick(10) > 2)
                .collect();
            let layout =
                GraphLayout::new(&document, &visible, prism_core::graph::Detail::Subtitles);
            assert_eq!(Some(layout.orientation), first);
        }
    }
}
