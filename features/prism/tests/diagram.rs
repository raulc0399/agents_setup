use prism::model::*;
use prism::ui::diagram::{DiagramLayout, DiagramState};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::collections::{BTreeMap, BTreeSet};

fn fixture() -> (Document, ViewState) {
    let mut document = Document {
        id: Id::from_u128(10),
        title: "Diagram".into(),
        revision: 1,
        elements: BTreeMap::new(),
        relationships: BTreeMap::new(),
        groups: BTreeMap::new(),
    };
    for (id, title) in [(1, "Source"), (2, "Sink")] {
        let id = Id::from_u128(id);
        document.elements.insert(
            id,
            Element {
                id,
                title: title.into(),
                body: "Body".into(),
                kind: ElementKind::Step,
                source: None,
                order: 0,
                semantic_revision: 1,
                accepted_revision: None,
                archived: false,
            },
        );
    }
    let id = Id::from_u128(3);
    document.relationships.insert(
        id,
        Relationship {
            id,
            from: Id::from_u128(1),
            to: Id::from_u128(2),
            label: "flows".into(),
            direction: Direction::Directed,
            semantic_revision: 1,
            accepted_revision: None,
            archived: false,
        },
    );
    let id = Id::from_u128(4);
    document.groups.insert(
        id,
        Group {
            id,
            title: "Both".into(),
            members: document.elements.keys().copied().collect(),
            semantic_revision: 1,
            accepted_revision: None,
            archived: false,
        },
    );
    let view = ViewState {
        id: Id::from_u128(20),
        document_id: document.id,
        kind: ViewKind::Diagram,
        selection: None,
        positions: BTreeMap::new(),
        hidden: BTreeSet::new(),
        filter: String::new(),
        draft: String::new(),
    };
    (document, view)
}

fn render(layout: &DiagramLayout, state: &mut DiagramState, area: Rect) -> ratatui::buffer::Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(area.right() + 2, area.bottom() + 2)).unwrap();
    terminal
        .draw(|frame| {
            state.render(
                frame,
                area,
                layout,
                Some(Id::from_u128(1)),
                &[Id::from_u128(3)].into_iter().collect(),
            )
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

#[test]
fn deterministic_layout_renders_directed_edges_and_selectable_groups() {
    let (document, view) = fixture();
    let layout = DiagramLayout::new(&document, &view);
    let again = DiagramLayout::new(&document, &view);
    let area = Rect::new(2, 3, 118, 20);
    let buffer = render(&layout, &mut DiagramState::default(), area);
    assert_eq!(buffer, render(&again, &mut DiagramState::default(), area));
    let rendered = text(&buffer);
    for label in [
        "Step: Source",
        "Step: Sink",
        "Link: flows",
        "Group: Both",
        "Source -> Sink",
        "2 members",
    ] {
        assert!(rendered.contains(label), "missing {label}");
    }
    assert_eq!(buffer[(area.x + 1 + 41, area.y + 1 + 1)].symbol(), ">");
    assert_eq!(buffer[(area.x + 1 + 30, area.y + 1 + 10)].symbol(), "<");
    let state = DiagramState::default();
    assert_eq!(
        state.hit_test(area.x + 85, area.y + 2, area, &layout),
        Some(Id::from_u128(4))
    );
    assert_eq!(layout.focus_navigation(None, 0), Some(Id::from_u128(1)));
    assert_eq!(
        layout.focus_navigation(Some(Id::from_u128(1)), isize::MAX),
        Some(Id::from_u128(4))
    );
    assert_eq!(
        layout.focus_navigation(Some(Id::from_u128(4)), isize::MIN),
        Some(Id::from_u128(1))
    );
}

#[test]
fn relationships_require_visible_live_endpoints() {
    let (mut document, mut view) = fixture();
    view.hidden.insert(Id::from_u128(1));
    assert_eq!(
        DiagramLayout::new(&document, &view).visible_ids(),
        vec![Id::from_u128(2), Id::from_u128(4)]
    );
    view.hidden.clear();
    view.filter = "flows".into();
    assert!(
        DiagramLayout::new(&document, &view)
            .visible_ids()
            .is_empty()
    );
    view.filter.clear();
    document
        .elements
        .get_mut(&Id::from_u128(1))
        .unwrap()
        .archived = true;
    assert!(
        !DiagramLayout::new(&document, &view)
            .visible_ids()
            .contains(&Id::from_u128(3))
    );
    document.elements.remove(&Id::from_u128(1));
    assert!(
        !DiagramLayout::new(&document, &view)
            .visible_ids()
            .contains(&Id::from_u128(3))
    );
    view.hidden.insert(Id::from_u128(4));
    assert_eq!(
        DiagramLayout::new(&document, &view).visible_ids(),
        vec![Id::from_u128(2)]
    );
}

#[test]
fn hit_testing_tracks_pan_and_resize_and_rejects_borders() {
    let (document, mut view) = fixture();
    view.positions
        .insert(Id::from_u128(4), Position { x: 200, y: 100 });
    let layout = DiagramLayout::new(&document, &view);
    let mut state = DiagramState::default();
    let area = Rect::new(7, 4, 40, 12);
    state.reveal(Id::from_u128(4), &layout, area);
    let buffer = render(&layout, &mut state, area);
    let position = layout.position(Id::from_u128(4)).unwrap();
    let x = (i32::from(area.x) + 2 + position.x - state.pan_x) as u16;
    let y = (i32::from(area.y) + 2 + position.y - state.pan_y) as u16;
    assert_eq!(state.hit_test(x, y, area, &layout), Some(Id::from_u128(4)));
    assert!(text(&buffer).contains("Group: Both"));
    assert_eq!(state.hit_test(area.x, area.y, area, &layout), None);
    let resized = Rect::new(3, 2, 240, 120);
    render(&layout, &mut state, resized);
    assert_eq!(state.pan_x, 0);
    assert_eq!(state.pan_y, 0);
    assert_eq!(
        state.hit_test(resized.x + 202, resized.y + 102, resized, &layout),
        Some(Id::from_u128(4))
    );
    state.pan(i32::MAX, i32::MAX, &layout, area);
    assert!(state.pan_x < 230 && state.pan_y < 105);
    state.pan(i32::MIN, i32::MIN, &layout, area);
    assert_eq!((state.pan_x, state.pan_y), (0, 0));
}

#[test]
fn private_positions_are_bounded_and_rendering_sanitizes_unicode_controls() {
    let (mut document, mut view) = fixture();
    document.elements.get_mut(&Id::from_u128(1)).unwrap().title = "界é\u{1b}[31m\n\t".into();
    view.positions.insert(
        Id::from_u128(1),
        Position {
            x: i32::MIN,
            y: i32::MAX,
        },
    );
    let layout = DiagramLayout::new(&document, &view);
    assert!(layout.bounds.width < 2_000_100 && layout.bounds.height < 2_000_100);
    let mut state = DiagramState::default();
    let area = Rect::new(0, 0, 50, 10);
    state.reveal(Id::from_u128(1), &layout, area);
    let rendered = text(&render(&layout, &mut state, area));
    assert!(rendered.contains("界"));
    assert!(rendered.contains("é\\u{1b}"));
    assert!(!rendered.contains('\u{1b}'));
    for width in [0, 1, 2, 3, 8] {
        render(&layout, &mut state, Rect::new(0, 0, width, 2));
    }
}

#[test]
fn maximum_graph_cycles_and_distant_coordinates_have_bounded_rendering() {
    let (mut document, mut view) = fixture();
    document.elements.clear();
    document.relationships.clear();
    document.groups.clear();
    for index in 0..MAX_ELEMENTS {
        let id = Id::from_u128(index as u128 + 1);
        document.elements.insert(
            id,
            Element {
                id,
                title: format!("Node {index}"),
                body: String::new(),
                kind: ElementKind::Step,
                source: None,
                order: 0,
                semantic_revision: 1,
                accepted_revision: None,
                archived: false,
            },
        );
    }
    for index in 0..MAX_RELATIONSHIPS {
        let id = Id::from_u128(index as u128 + 10_000);
        document.relationships.insert(
            id,
            Relationship {
                id,
                from: Id::from_u128((index % MAX_ELEMENTS) as u128 + 1),
                to: Id::from_u128(((index + 1) % MAX_ELEMENTS) as u128 + 1),
                label: format!("Edge {index}"),
                direction: Direction::Bidirectional,
                semantic_revision: 1,
                accepted_revision: None,
                archived: false,
            },
        );
    }
    for index in 0..MAX_GROUPS {
        let id = Id::from_u128(index as u128 + 20_000);
        document.groups.insert(
            id,
            Group {
                id,
                title: format!("Group {index}"),
                members: BTreeSet::new(),
                semantic_revision: 1,
                accepted_revision: None,
                archived: false,
            },
        );
    }
    let layout = DiagramLayout::new(&document, &view);
    assert_eq!(layout.visible_ids().len(), MAX_SELECTION);
    for index in 1..MAX_ELEMENTS {
        let previous = layout.position(Id::from_u128(index as u128)).unwrap();
        let next = layout.position(Id::from_u128(index as u128 + 1)).unwrap();
        assert!(next.y >= previous.y + 5);
    }
    assert!(layout.bounds.width < 200 && layout.bounds.height < 30_000);
    view.positions.insert(
        Id::from_u128(1),
        Position {
            x: i32::MAX,
            y: i32::MIN,
        },
    );
    let layout = DiagramLayout::new(&document, &view);
    let area = Rect::new(0, 0, 100, 30);
    let mut state = DiagramState::default();
    let rendered = render(&layout, &mut state, area);
    assert_eq!(rendered.content.len(), 102 * 32);
    state.reveal(Id::from_u128(1), &layout, area);
    assert!(text(&render(&layout, &mut state, area)).contains("Node 0"));
}

#[test]
fn edge_direction_symbols_follow_each_relationship_kind() {
    let (mut document, view) = fixture();
    let area = Rect::new(0, 0, 118, 20);
    for (direction, source_arrow, target_arrow) in [
        (Direction::Directed, "─", "<"),
        (Direction::Undirected, "─", "─"),
        (Direction::Bidirectional, "<", "<"),
    ] {
        document
            .relationships
            .get_mut(&Id::from_u128(3))
            .unwrap()
            .direction = direction;
        let layout = DiagramLayout::new(&document, &view);
        let buffer = render(&layout, &mut DiagramState::default(), area);
        assert_eq!(buffer[(31, 2)].symbol(), source_arrow);
        assert_eq!(buffer[(31, 11)].symbol(), target_arrow);
    }
}

#[test]
fn bounds_include_routes_when_private_cards_move_beyond_the_default_lanes() {
    let (document, mut view) = fixture();
    view.positions
        .insert(Id::from_u128(3), Position { x: -100, y: 0 });
    view.positions
        .insert(Id::from_u128(1), Position { x: 500, y: 0 });
    let layout = DiagramLayout::new(&document, &view);
    assert!(layout.bounds.x <= -101);
    assert!(layout.bounds.x + layout.bounds.width > 530);
}
