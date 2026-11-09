//! Renders UI frames with colors to HTML fragments for design review (temporary, not shipped).
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use prism::actions::*;
use prism::model::*;
use prism::store::Store;
use prism::ui::{ConnectionInfo, UiState};
use ratatui::style::{Color, Modifier};
use ratatui::{Terminal, backend::TestBackend};

fn element(id: u128, title: &str, body: &str, kind: ElementKind) -> ElementInput {
    ElementInput {
        id: Id::from_u128(id),
        title: title.into(),
        body: body.into(),
        kind,
        source: Some("src/session.rs:120".into()),
    }
}

fn css(color: Color, fallback: &str) -> String {
    match color {
        Color::Reset => fallback.into(),
        Color::Black => "#000".into(),
        Color::Red => "#e5484d".into(),
        Color::Green => "#3dd68c".into(),
        Color::Yellow => "#f5d90a".into(),
        Color::Blue => "#6e9fff".into(),
        Color::Magenta => "#d783ff".into(),
        Color::Cyan => "#4cc9f0".into(),
        Color::Gray => "#bbb".into(),
        Color::DarkGray => "#6b7280".into(),
        Color::LightRed => "#ff8a80".into(),
        Color::LightGreen => "#8ef0b5".into(),
        Color::LightYellow => "#fff59d".into(),
        Color::LightBlue => "#9ec1ff".into(),
        Color::LightMagenta => "#efa7ff".into(),
        Color::LightCyan => "#a5f3fc".into(),
        Color::White => "#fff".into(),
        _ => fallback.into(),
    }
}

fn html(state: &mut UiState, width: u16, height: u16, info: &ConnectionInfo) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| state.render(frame, info)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::from("<pre class=\"term\">");
    for y in 0..height {
        let mut x = 0;
        while x < width {
            let cell = &buffer[(x, y)];
            let fg = css(cell.fg, "var(--fg)");
            let bg = css(cell.bg, "transparent");
            let bold = cell.modifier.contains(Modifier::BOLD);
            let symbol = cell.symbol().replace('&', "&amp;").replace('<', "&lt;");
            out.push_str(&format!(
                "<span style=\"color:{fg};background:{bg};{}\">{symbol}</span>",
                if bold { "font-weight:700" } else { "" }
            ));
            let w = unicode_width(cell.symbol());
            x += w.max(1);
        }
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}

fn unicode_width(s: &str) -> u16 {
    ratatui::text::Line::raw(s).width() as u16
}

fn key(
    state: &mut UiState,
    store: &mut Store,
    code: KeyCode,
    modifiers: KeyModifiers,
    info: &ConnectionInfo,
) {
    state.handle_event(store, Event::Key(KeyEvent::new(code, modifiers)), info);
}

fn main() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("ui.sqlite3")).unwrap();
    let workspace = store
        .workspace(&WorkspaceKey {
            project: "/project".into(),
            agent: AgentKind::Claude,
            conversation: "s".into(),
        })
        .unwrap();
    let mut request = PublishRequest {
        request_id: Id::new_v4(),
        document_id: None,
        expected_revision: None,
        title: "Migration plan for session bindings".into(),
        elements: vec![
            element(
                1,
                "Add schema 3 migration",
                "Introduce the bindings table with a transactional migration from schema 2. Preserve views and drafts.",
                ElementKind::Step,
            ),
            element(
                2,
                "Route pane by ancestry",
                "Verify tmux pane ownership using /proc ancestry instead of the active pane.",
                ElementKind::Step,
            ),
            element(
                3,
                "Stale socket is not pruned",
                "Abandoned /tmp/prism-* directories survive a crash and block relaunch for the same project.",
                ElementKind::Finding,
            ),
            element(
                4,
                "Should Share clear the draft?",
                "Clearing the draft after Share loses context when the CLI refuses retrieval.",
                ElementKind::Question,
            ),
            element(
                5,
                "Context",
                "This plan covers the shared core and the Claude adapter only.",
                ElementKind::Text,
            ),
        ],
        relationships: vec![
            RelationshipInput {
                id: Id::from_u128(10),
                from: Id::from_u128(1),
                to: Id::from_u128(2),
                label: "precedes".into(),
                direction: Direction::Directed,
                manual: false,
            },
            RelationshipInput {
                id: Id::from_u128(11),
                from: Id::from_u128(3),
                to: Id::from_u128(2),
                label: "blocks".into(),
                direction: Direction::Directed,
                manual: false,
            },
        ],
        groups: vec![GroupInput {
            id: Id::from_u128(20),
            title: "Batch 1".into(),
            members: [Id::from_u128(1), Id::from_u128(2)].into_iter().collect(),
        }],
    };
    let document = store.publish(workspace.id, &request).unwrap().document_id;
    let info = ConnectionInfo {
        generation: 3,
        connected: true,
        send_modes: vec![],
        message: "Verified binding".into(),
    };
    let mut state = UiState::new(&mut store, workspace.clone()).unwrap();
    let (w, h) = (78, 40);
    let mut frames: Vec<(String, String)> = Vec::new();

    // accept item 1, then revise item 3 so it shows "changed"; ask about item 2
    key(
        &mut state,
        &mut store,
        KeyCode::Char('a'),
        KeyModifiers::NONE,
        &info,
    );
    request.request_id = Id::new_v4();
    request.document_id = Some(document);
    request.expected_revision = Some(2);
    request.elements[2].body = "Abandoned /tmp/prism-* directories survive a crash. Prune them on the next launch for that project.".into();
    store.publish(workspace.id, &request).unwrap();
    state.refresh(&mut store).unwrap();
    key(
        &mut state,
        &mut store,
        KeyCode::Down,
        KeyModifiers::NONE,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Char('e'),
        KeyModifiers::NONE,
        &info,
    );
    for c in "Why does this step depend on the finding?".chars() {
        key(
            &mut state,
            &mut store,
            KeyCode::Char(c),
            KeyModifiers::NONE,
            &info,
        );
    }
    frames.push((
        "Ask editor at the bottom (78x40)".into(),
        html(&mut state, w, h, &info),
    ));
    key(
        &mut state,
        &mut store,
        KeyCode::Char('s'),
        KeyModifiers::CONTROL,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Down,
        KeyModifiers::NONE,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Char(' '),
        KeyModifiers::NONE,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Down,
        KeyModifiers::NONE,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Char(' '),
        KeyModifiers::NONE,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Up,
        KeyModifiers::NONE,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Up,
        KeyModifiers::NONE,
        &info,
    );
    frames.insert(0, ("Single column in a right pane (78x40): focused card expanded, two selected, one accepted, one changed".into(), html(&mut state, w, h, &info)));
    frames.push((
        "Narrow pane (60x30)".into(),
        html(&mut state, 60, 30, &info),
    ));
    key(
        &mut state,
        &mut store,
        KeyCode::Char('v'),
        KeyModifiers::NONE,
        &info,
    );
    frames.push((
        "Diagram, still the old text layout (78x40)".into(),
        html(&mut state, w, h, &info),
    ));
    key(
        &mut state,
        &mut store,
        KeyCode::Char('v'),
        KeyModifiers::NONE,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
        &info,
    );
    key(
        &mut state,
        &mut store,
        KeyCode::Char('\\'),
        KeyModifiers::CONTROL,
        &info,
    );
    frames.push((
        "Two tabs and a top/bottom split (78x40)".into(),
        html(&mut state, w, h, &info),
    ));
    key(
        &mut state,
        &mut store,
        KeyCode::Char('\\'),
        KeyModifiers::CONTROL,
        &info,
    );
    frames.push((
        "Wide terminal fallback (130x34): details beside the list".into(),
        html(&mut state, 130, 34, &info),
    ));
    key(
        &mut state,
        &mut store,
        KeyCode::Char('?'),
        KeyModifiers::NONE,
        &info,
    );
    frames.push(("Help (78x40)".into(), html(&mut state, w, h, &info)));
    let ws2 = store
        .workspace(&WorkspaceKey {
            project: "/project".into(),
            agent: AgentKind::Claude,
            conversation: "empty".into(),
        })
        .unwrap();
    let mut empty = UiState::new(&mut store, ws2).unwrap();
    let off = ConnectionInfo {
        generation: 0,
        connected: false,
        send_modes: vec![],
        message: "Binding retired; explicitly resume".into(),
    };
    frames.push((
        "Disconnected, nothing published (78x14)".into(),
        html(&mut empty, w, 14, &off),
    ));

    for (title, frame) in frames {
        println!("<section><h2>{title}</h2>{frame}</section>");
    }
}
