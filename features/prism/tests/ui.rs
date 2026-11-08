use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use prism::actions::*;
use prism::model::*;
use prism::store::Store;
use prism::ui::{ConnectionInfo, InputMode, UiState};
use ratatui::{Terminal, backend::TestBackend};
use tempfile::TempDir;

struct Fixture {
    _directory: TempDir,
    path: std::path::PathBuf,
    store: Store,
    workspace: Workspace,
    request: PublishRequest,
    document: Id,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ui.sqlite3");
        let mut store = Store::open(&path).unwrap();
        let workspace = store
            .workspace(&WorkspaceKey {
                project: "/project".into(),
                agent: AgentKind::Codex,
                conversation: "exact-thread".into(),
            })
            .unwrap();
        let request = PublishRequest {
            request_id: Id::new_v4(),
            document_id: None,
            expected_revision: None,
            title: "Plan".into(),
            elements: vec![element(1, "Source"), element(2, "Sink")],
            relationships: vec![RelationshipInput {
                id: Id::from_u128(3),
                from: Id::from_u128(1),
                to: Id::from_u128(2),
                label: "flows".into(),
                direction: Direction::Directed,
            }],
            groups: vec![GroupInput {
                id: Id::from_u128(4),
                title: "Both".into(),
                members: [Id::from_u128(1), Id::from_u128(2)].into_iter().collect(),
            }],
        };
        let document = store.publish(workspace.id, &request).unwrap().document_id;
        Self {
            _directory: directory,
            path,
            store,
            workspace,
            request,
            document,
        }
    }

    fn state(&mut self) -> UiState {
        UiState::new(&mut self.store, self.workspace.clone()).unwrap()
    }

    fn publish_change(&self) {
        let mut store = Store::open(&self.path).unwrap();
        let mut request = self.request.clone();
        request.request_id = Id::new_v4();
        request.document_id = Some(self.document);
        request.expected_revision = Some(
            store
                .document(self.workspace.id, self.document)
                .unwrap()
                .revision,
        );
        request.elements[0].body = "Published edit".into();
        store.publish(self.workspace.id, &request).unwrap();
    }
}

fn element(id: u128, title: &str) -> ElementInput {
    ElementInput {
        id: Id::from_u128(id),
        title: title.into(),
        body: "Body".into(),
        kind: ElementKind::Step,
        source: None,
    }
}

fn connection() -> ConnectionInfo {
    ConnectionInfo {
        generation: 7,
        connected: true,
        send_modes: vec![DeliveryMode::Start, DeliveryMode::Steer],
        message: "Verified binding".into(),
    }
}

fn key(state: &mut UiState, store: &mut Store, code: KeyCode) {
    state.handle_event(
        store,
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
        &connection(),
    );
}

fn control(state: &mut UiState, store: &mut Store, character: char, info: &ConnectionInfo) {
    state.handle_event(
        store,
        Event::Key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::CONTROL,
        )),
        info,
    );
}

fn type_text(state: &mut UiState, store: &mut Store, text: &str) {
    for character in text.chars() {
        key(state, store, KeyCode::Char(character));
    }
}

fn render(state: &mut UiState, width: u16, height: u16, info: &ConnectionInfo) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| state.render(frame, info)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn list_actions_use_shared_acceptance_archive_and_undo() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    assert_eq!(state.rows().len(), 4);
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('a'));
    let document = fixture
        .store
        .document(fixture.workspace.id, fixture.document)
        .unwrap();
    assert_eq!(
        document.elements[&Id::from_u128(1)].accepted_revision,
        Some(1)
    );
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('x'));
    let document = fixture
        .store
        .document(fixture.workspace.id, fixture.document)
        .unwrap();
    assert!(document.elements[&Id::from_u128(1)].archived);
    assert!(document.relationships[&Id::from_u128(3)].archived);
    assert_eq!(state.rows().len(), 2);
    key(&mut state, &mut fixture.store, KeyCode::Char('u'));
    assert_eq!(state.rows().len(), 4);
    assert!(state.rows()[0].accepted);
}

#[test]
fn stale_selection_is_preserved_visibly_and_blocks_actions_and_send() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "Why?");
    fixture.publish_change();
    state.refresh(&mut fixture.store).unwrap();
    assert!(state.selection_stale());
    assert!(render(&mut state, 100, 30, &connection()).contains("STALE selection"));
    control(&mut state, &mut fixture.store, 's', &connection());
    assert!(
        fixture
            .store
            .questions(fixture.workspace.id)
            .unwrap()
            .is_empty()
    );
    key(&mut state, &mut fixture.store, KeyCode::Esc);
    key(&mut state, &mut fixture.store, KeyCode::Char('x'));
    assert!(state.message.contains("stale"));
    assert_eq!(
        fixture
            .store
            .document(fixture.workspace.id, fixture.document)
            .unwrap()
            .revision,
        2
    );
    key(&mut state, &mut fixture.store, KeyCode::Char('r'));
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    assert!(!state.selection_stale());
}

#[test]
fn concurrent_change_before_refresh_rejects_mutation_without_overwrite() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    fixture.publish_change();
    key(&mut state, &mut fixture.store, KeyCode::Char('a'));
    assert!(state.message.contains("revision conflict"));
    assert_eq!(
        fixture
            .store
            .document(fixture.workspace.id, fixture.document)
            .unwrap()
            .elements[&Id::from_u128(1)]
            .accepted_revision,
        None
    );
}

#[test]
fn private_unicode_draft_and_selection_survive_restart_without_publication() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "private 界é");
    key(&mut state, &mut fixture.store, KeyCode::Left);
    key(&mut state, &mut fixture.store, KeyCode::Backspace);
    key(&mut state, &mut fixture.store, KeyCode::Enter);
    assert_eq!(state.view.as_ref().unwrap().draft, "private \né");
    assert!(
        fixture
            .store
            .questions(fixture.workspace.id)
            .unwrap()
            .is_empty()
    );
    let document = fixture
        .store
        .document(fixture.workspace.id, fixture.document)
        .unwrap();
    assert!(
        !serde_json::to_string(&document)
            .unwrap()
            .contains("private")
    );
    drop(state);
    let mut reopened = Store::open(&fixture.path).unwrap();
    let state = UiState::new(&mut reopened, fixture.workspace.clone()).unwrap();
    assert_eq!(state.view.as_ref().unwrap().draft, "private \né");
    assert_eq!(
        state.view.as_ref().unwrap().selection.as_ref().unwrap().ids,
        vec![Id::from_u128(1)]
    );
}

#[test]
fn explicit_send_freezes_selection_and_requires_current_verified_generation() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "Question");
    key(&mut state, &mut fixture.store, KeyCode::Enter);
    assert!(
        fixture
            .store
            .questions(fixture.workspace.id)
            .unwrap()
            .is_empty()
    );
    control(&mut state, &mut fixture.store, 'n', &connection());
    assert_eq!(state.delivery_mode, DeliveryMode::Start);
    let changed = ConnectionInfo {
        generation: 8,
        ..connection()
    };
    control(&mut state, &mut fixture.store, 's', &changed);
    assert!(state.message.contains("Binding changed"));
    assert!(
        fixture
            .store
            .questions(fixture.workspace.id)
            .unwrap()
            .is_empty()
    );
    control(&mut state, &mut fixture.store, 's', &connection());
    let questions = fixture.store.questions(fixture.workspace.id).unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].generation, 7);
    assert_eq!(questions[0].snapshot.selection.ids, vec![Id::from_u128(1)]);
    assert_eq!(questions[0].mode, DeliveryMode::Start);
    assert_eq!(questions[0].outcome, DeliveryOutcome::Pending);
    assert_eq!(questions[0].text, "Question\n");
    assert_eq!(state.input_mode, InputMode::Browse);
    assert!(state.view.as_ref().unwrap().draft.is_empty());
    control(&mut state, &mut fixture.store, 's', &connection());
    assert_eq!(
        fixture.store.questions(fixture.workspace.id).unwrap().len(),
        1
    );
}

#[test]
fn disconnected_native_send_is_disabled_and_share_remains_explicit() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "Share me");
    control(&mut state, &mut fixture.store, 'n', &connection());
    let disconnected = ConnectionInfo {
        connected: false,
        ..connection()
    };
    control(&mut state, &mut fixture.store, 's', &disconnected);
    assert!(state.message.contains("unavailable"));
    assert!(
        fixture
            .store
            .questions(fixture.workspace.id)
            .unwrap()
            .is_empty()
    );
    control(&mut state, &mut fixture.store, 'n', &disconnected);
    assert_eq!(state.delivery_mode, DeliveryMode::Share);
    control(&mut state, &mut fixture.store, 's', &disconnected);
    let questions = fixture.store.questions(fixture.workspace.id).unwrap();
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].mode, DeliveryMode::Share);
    let output = render(&mut state, 160, 40, &disconnected);
    assert!(output.contains("no native turn started"));
}

#[test]
fn filter_hide_and_document_switch_are_private_and_restore_drafts() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char('/'));
    type_text(&mut state, &mut fixture.store, "Sink");
    key(&mut state, &mut fixture.store, KeyCode::Enter);
    assert_eq!(state.rows().len(), 3);
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "private draft");
    key(&mut state, &mut fixture.store, KeyCode::Esc);
    key(&mut state, &mut fixture.store, KeyCode::Char('h'));
    assert_eq!(state.rows().len(), 2);
    assert_eq!(
        fixture
            .store
            .document(fixture.workspace.id, fixture.document)
            .unwrap()
            .revision,
        1
    );
    let mut second = fixture.request.clone();
    second.request_id = Id::new_v4();
    second.title = "Second document".into();
    fixture
        .store
        .publish(fixture.workspace.id, &second)
        .unwrap();
    state.refresh(&mut fixture.store).unwrap();
    key(&mut state, &mut fixture.store, KeyCode::Tab);
    assert_ne!(state.document().unwrap().id, fixture.document);
    assert!(state.view.as_ref().unwrap().draft.is_empty());
    key(&mut state, &mut fixture.store, KeyCode::Tab);
    assert_eq!(state.document().unwrap().id, fixture.document);
    assert_eq!(state.view.as_ref().unwrap().draft, "private draft");
    assert_eq!(state.view.as_ref().unwrap().filter, "Sink");
    assert!(
        state
            .view
            .as_ref()
            .unwrap()
            .hidden
            .contains(&Id::from_u128(2))
    );
    key(&mut state, &mut fixture.store, KeyCode::Char('H'));
    assert_eq!(state.rows().len(), 3);
}

#[test]
fn rendering_escapes_controls_and_handles_tiny_panes_and_unicode_editing() {
    let mut fixture = Fixture::new();
    let mut update = fixture.request.clone();
    update.document_id = Some(fixture.document);
    update.expected_revision = Some(1);
    update.request_id = Id::new_v4();
    update.elements[0].title = "unsafe\u{1b}[2J".into();
    fixture
        .store
        .publish(fixture.workspace.id, &update)
        .unwrap();
    let mut state = fixture.state();
    let output = render(&mut state, 100, 30, &connection());
    assert!(!output.contains('\u{1b}'));
    assert!(output.contains("\\u{1b}"));
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "界é🙂");
    for size in [(1, 1), (2, 2), (12, 6), (35, 15), (100, 30)] {
        render(&mut state, size.0, size.1, &connection());
    }
}

#[test]
fn mouse_selection_uses_scrolled_list_offset_after_resize() {
    let mut fixture = Fixture::new();
    let mut update = fixture.request.clone();
    update.document_id = Some(fixture.document);
    update.expected_revision = Some(1);
    update.request_id = Id::new_v4();
    update.elements = (10..70)
        .map(|id| element(id, &format!("Item {id}")))
        .collect();
    fixture
        .store
        .publish(fixture.workspace.id, &update)
        .unwrap();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::End);
    render(&mut state, 100, 14, &connection());
    // The 9-row content pane has borders, so its last visible row is y=9.
    state.handle_event(
        &mut fixture.store,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 9,
            modifiers: KeyModifiers::NONE,
        }),
        &connection(),
    );
    assert_eq!(
        state.view.as_ref().unwrap().selection.as_ref().unwrap().ids,
        vec![Id::from_u128(4)]
    );
    render(&mut state, 35, 25, &connection());
    state.handle_event(
        &mut fixture.store,
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 3,
            row: 3,
            modifiers: KeyModifiers::NONE,
        }),
        &connection(),
    );
    assert_eq!(state.focus, Some(Id::from_u128(3)));
}

#[test]
fn delivery_unknown_is_not_retried_and_preserves_native_reason() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "Question");
    control(&mut state, &mut fixture.store, 'n', &connection());
    control(&mut state, &mut fixture.store, 's', &connection());
    let question = fixture
        .store
        .questions(fixture.workspace.id)
        .unwrap()
        .remove(0);
    fixture
        .store
        .update_delivery(
            fixture.workspace.id,
            question.request_id,
            7,
            &DeliveryOutcome::Pending,
            DeliveryOutcome::Submitted,
        )
        .unwrap();
    fixture
        .store
        .update_delivery(
            fixture.workspace.id,
            question.request_id,
            7,
            &DeliveryOutcome::Submitted,
            DeliveryOutcome::Unknown {
                reason: "lost native receipt".into(),
            },
        )
        .unwrap();
    state.refresh(&mut fixture.store).unwrap();
    let output = render(&mut state, 180, 40, &connection());
    assert!(output.contains("lost native receipt"));
    assert!(output.contains("may duplicate"));
    state.refresh(&mut fixture.store).unwrap();
    assert_eq!(
        fixture.store.questions(fixture.workspace.id).unwrap().len(),
        1
    );
}

#[test]
fn stable_focus_and_stale_acceptance_follow_published_semantics() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('a'));
    assert!(state.rows()[0].accepted);
    fixture.publish_change();
    state.refresh(&mut fixture.store).unwrap();
    assert_eq!(state.focus, Some(Id::from_u128(1)));
    assert!(state.rows()[0].stale_acceptance);
    assert!(!state.rows()[0].accepted);
    assert!(render(&mut state, 120, 30, &connection()).contains("accepted stale"));
}

#[test]
fn narrow_help_exposes_editor_controls_and_never_mutates_content() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('?'));
    key(&mut state, &mut fixture.store, KeyCode::Char('x'));
    assert_eq!(
        fixture
            .store
            .document(fixture.workspace.id, fixture.document)
            .unwrap()
            .revision,
        1
    );
    let mut output = String::new();
    for _ in 0..30 {
        output.push_str(&render(&mut state, 35, 12, &connection()));
        key(&mut state, &mut fixture.store, KeyCode::Down);
    }
    assert!(output.contains("Ctrl-S"));
    assert!(output.contains("Ctrl-N"));
    key(&mut state, &mut fixture.store, KeyCode::Esc);
    assert!(render(&mut state, 100, 30, &connection()).contains("Content"));
}

#[test]
fn failed_private_draft_clear_does_not_commit_a_resendable_question() {
    let mut fixture = Fixture::new();
    let mut state = fixture.state();
    key(&mut state, &mut fixture.store, KeyCode::Char(' '));
    key(&mut state, &mut fixture.store, KeyCode::Char('e'));
    type_text(&mut state, &mut fixture.store, "Send once");
    let database = rusqlite::Connection::open(&fixture.path).unwrap();
    database.execute_batch("CREATE TRIGGER fail_private_clear BEFORE UPDATE ON private_views BEGIN SELECT RAISE(ABORT, 'private view write failed'); END;").unwrap();
    control(&mut state, &mut fixture.store, 's', &connection());
    assert!(
        fixture
            .store
            .questions(fixture.workspace.id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(state.view.as_ref().unwrap().draft, "Send once");
    database
        .execute_batch("DROP TRIGGER fail_private_clear;")
        .unwrap();
    control(&mut state, &mut fixture.store, 's', &connection());
    assert_eq!(
        fixture.store.questions(fixture.workspace.id).unwrap().len(),
        1
    );
    assert!(
        fixture.store.views(fixture.workspace.id).unwrap()[0]
            .draft
            .is_empty()
    );
}

#[test]
fn history_pagination_reaches_delivery_after_two_long_questions() {
    let mut fixture = Fixture::new();
    for id in [100, 101] {
        let question = fixture
            .store
            .prepare_question(
                fixture.workspace.id,
                &PrepareQuestion {
                    request_id: Id::from_u128(id),
                    generation: 7,
                    selection: Selection {
                        document_id: fixture.document,
                        revision: 1,
                        ids: vec![Id::from_u128(1)],
                    },
                    text: format!("{}Question {id}", "\n".repeat(40_000)),
                    mode: DeliveryMode::Start,
                },
            )
            .unwrap();
        fixture
            .store
            .update_delivery(
                fixture.workspace.id,
                question.request_id,
                7,
                &DeliveryOutcome::Pending,
                DeliveryOutcome::Submitted,
            )
            .unwrap();
        fixture
            .store
            .update_delivery(
                fixture.workspace.id,
                question.request_id,
                7,
                &DeliveryOutcome::Submitted,
                DeliveryOutcome::Unknown {
                    reason: format!("receipt {id} unavailable"),
                },
            )
            .unwrap();
    }
    let mut state = fixture.state();
    for _ in 0..24 {
        key(&mut state, &mut fixture.store, KeyCode::Char(']'));
    }
    let mut found_delivery = false;
    for _ in 0..850 {
        if render(&mut state, 120, 30, &connection()).contains("receipt 101 unavailable") {
            found_delivery = true;
            break;
        }
        key(&mut state, &mut fixture.store, KeyCode::PageDown);
    }
    assert!(found_delivery);
}

#[test]
fn multiline_document_title_cannot_hide_connection_status() {
    let mut fixture = Fixture::new();
    let mut request = fixture.request.clone();
    request.request_id = Id::new_v4();
    request.document_id = Some(fixture.document);
    request.expected_revision = Some(1);
    request.title = "First\nSecond\tThird".into();
    fixture
        .store
        .publish(fixture.workspace.id, &request)
        .unwrap();
    let mut state = fixture.state();
    let info = ConnectionInfo {
        connected: false,
        message: "Native transport unavailable".into(),
        ..connection()
    };
    let output = render(&mut state, 120, 30, &info);
    assert!(output.contains("Disconnected"));
    assert!(output.contains("Native transport unavailable"));
    assert!(output.contains("First Second Third"));
}
