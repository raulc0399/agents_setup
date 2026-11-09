use prism::actions::*;
use prism::model::*;
use prism::store::Store;
use prism::ui::graph::{Detail, GraphLayout, GraphView, load_fonts};
use ratatui::layout::Rect;
use std::collections::BTreeSet;

const STEP_A: u128 = 1;
const STEP_B: u128 = 2;
const FINDING: u128 = 3;
const FLOW: u128 = 10;
const GATE_LINK: u128 = 11;
const LANE: u128 = 20;

fn element(id: u128, title: &str, kind: ElementKind) -> ElementInput {
    ElementInput {
        id: Id::from_u128(id),
        title: title.into(),
        body: "Body text for the box".into(),
        kind,
        source: None,
    }
}

fn link(id: u128, from: u128, to: u128, label: &str, manual: bool) -> RelationshipInput {
    RelationshipInput {
        id: Id::from_u128(id),
        from: Id::from_u128(from),
        to: Id::from_u128(to),
        label: label.into(),
        direction: Direction::Directed,
        manual,
    }
}

fn fixture() -> (Document, ViewState) {
    let mut store = Store::open(":memory:").unwrap();
    let workspace = store
        .workspace(&WorkspaceKey {
            project: "/project".into(),
            agent: AgentKind::Claude,
            conversation: "session".into(),
        })
        .unwrap();
    let request = PublishRequest {
        request_id: Id::new_v4(),
        document_id: None,
        expected_revision: None,
        title: "Flow".into(),
        elements: vec![
            element(STEP_A, "Collect", ElementKind::Step),
            element(STEP_B, "Store", ElementKind::Step),
            element(FINDING, "Token missing", ElementKind::Finding),
        ],
        relationships: vec![
            link(FLOW, STEP_A, STEP_B, "hands over", true),
            link(GATE_LINK, FINDING, STEP_A, "blocks", false),
        ],
        groups: vec![GroupInput {
            id: Id::from_u128(LANE),
            title: "Device".into(),
            members: [STEP_A, STEP_B].into_iter().map(Id::from_u128).collect(),
        }],
    };
    let id = store.publish(workspace.id, &request).unwrap().document_id;
    let document = store.document(workspace.id, id).unwrap();
    let view = ViewState {
        id: Id::new_v4(),
        document_id: id,
        kind: ViewKind::Diagram,
        selection: None,
        positions: Default::default(),
        hidden: Default::default(),
        filter: String::new(),
        draft: String::new(),
    };
    (document, view)
}

#[test]
fn a_finding_attached_to_one_box_becomes_its_gate_badge() {
    let (document, view) = fixture();
    let layout = GraphLayout::new(&document, &view, Detail::Subtitles);
    assert_eq!(layout.node_count(), 2);
    assert_eq!(
        layout.ids(),
        vec![
            Id::from_u128(STEP_A),
            Id::from_u128(FINDING),
            Id::from_u128(STEP_B),
            Id::from_u128(FLOW)
        ]
    );
    let node = layout.rect_of(Id::from_u128(STEP_A)).unwrap();
    let gate = layout.rect_of(Id::from_u128(FINDING)).unwrap();
    assert!(gate.y < node.y && gate.x + gate.width <= node.x + node.width + 8.0);
    assert_eq!(
        layout.hit(gate.x + gate.width / 2.0, gate.y + gate.height / 2.0),
        Some(Id::from_u128(FINDING))
    );
    assert_eq!(
        layout.hit(node.x + node.width / 2.0, node.y + node.height - 2.0),
        Some(Id::from_u128(STEP_A))
    );
    assert_eq!(layout.hit(-50.0, -50.0), None);
}

#[test]
fn arrows_are_numbered_dashed_when_manual_and_clickable_on_their_pill() {
    let (document, view) = fixture();
    let layout = GraphLayout::new(&document, &view, Detail::Names);
    let svg = layout.svg(Some(Id::from_u128(FLOW)), &BTreeSet::new());
    assert!(svg.contains("stroke-dasharray"));
    assert!(svg.contains(">1 hands over<"));
    assert!(svg.contains(">DEVICE<"));
    assert!(
        !svg.contains("blocks"),
        "a gate link is drawn as a badge, not an arrow"
    );
    let pill = layout.rect_of(Id::from_u128(FLOW)).unwrap();
    assert_eq!(
        layout.hit(pill.x + pill.width / 2.0, pill.y + pill.height / 2.0),
        Some(Id::from_u128(FLOW))
    );
}

#[test]
fn full_detail_grows_boxes_with_their_text() {
    let (document, view) = fixture();
    let names = GraphLayout::new(&document, &view, Detail::Names);
    let full = GraphLayout::new(&document, &view, Detail::Full);
    assert!(full.bounds.height > names.bounds.height);
    assert!(
        full.svg(None, &BTreeSet::new())
            .contains("Body text for the box")
    );
    assert!(!names.svg(None, &BTreeSet::new()).contains("Body text"));
}

#[test]
fn viewport_maps_cells_to_shapes_and_exports_svg() {
    let Some(fonts) = load_fonts() else {
        eprintln!("no system fonts; skipping");
        return;
    };
    let (document, view) = fixture();
    let mut graph = GraphView::new(false, (10, 20), fonts);
    let inner = Rect::new(1, 1, 76, 38);
    graph.prepare(&document, &view, (inner.width, inner.height));
    // Zoom 1 fits the whole diagram, so a shape's center maps back to itself.
    let rect = graph.layout().rect_of(Id::from_u128(STEP_B)).unwrap();
    let mut found = None;
    for row in inner.y..inner.bottom() {
        for column in inner.x..inner.right() {
            if graph.hit(inner, column, row) == Some(Id::from_u128(STEP_B)) {
                found = Some((column, row));
            }
        }
    }
    assert!(found.is_some(), "box {rect:?} not reachable");
    assert_eq!(graph.hit(inner, 0, 0), None);
    graph.zoom_by(prism::ui::graph::ZOOM_STEP, Some(rect));
    assert!(graph.zoom > 1.0);
    graph.reset_zoom();
    assert_eq!(graph.zoom, 1.0);
    let directory = tempfile::tempdir().unwrap();
    let path = graph
        .export_svg(&document, &view, &directory.path().join("export"))
        .unwrap();
    assert_eq!(path.file_name().unwrap(), "flow.svg");
    assert!(std::fs::read_to_string(path).unwrap().starts_with("<svg"));
}
