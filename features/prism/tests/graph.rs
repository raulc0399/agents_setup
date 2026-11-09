use prism::actions::*;
use prism::model::*;
use prism::store::Store;
use prism::ui::graph::{Detail, GraphView, load_fonts, visible_layout};
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
fn visible_layout_follows_the_list_filter() {
    let (document, mut view) = fixture();
    view.filter = "store".into();
    let layout = visible_layout(&document, &view, Detail::Names);
    assert!(layout.rect_of(Id::from_u128(STEP_B)).is_some());
    assert_eq!(layout.rect_of(Id::from_u128(STEP_A)), None);
    assert_eq!(
        layout.rect_of(Id::from_u128(FLOW)),
        None,
        "the arrow matches the filter but one of its ends does not"
    );
}

#[test]
fn visible_layout_leaves_out_archived_content() {
    let (mut document, view) = fixture();
    document
        .elements
        .get_mut(&Id::from_u128(FINDING))
        .unwrap()
        .archived = true;
    document
        .relationships
        .get_mut(&Id::from_u128(GATE_LINK))
        .unwrap()
        .archived = true;
    let layout = visible_layout(&document, &view, Detail::Names);
    assert_eq!(layout.rect_of(Id::from_u128(FINDING)), None);
    assert!(layout.rect_of(Id::from_u128(STEP_A)).is_some());
}

#[test]
fn visible_layout_drops_a_hidden_group_but_keeps_its_boxes() {
    let (document, mut view) = fixture();
    view.hidden.insert(Id::from_u128(LANE));
    let layout = visible_layout(&document, &view, Detail::Names);
    let svg = layout.svg(None, &BTreeSet::new());
    assert!(!svg.contains(r#"data-kind="lane""#) && !svg.contains(">DEVICE<"));
    assert!(layout.rect_of(Id::from_u128(STEP_A)).is_some());
    assert!(layout.rect_of(Id::from_u128(STEP_B)).is_some());
}

#[test]
fn visible_layout_drops_arrows_to_hidden_ends() {
    let (document, mut view) = fixture();
    view.hidden.insert(Id::from_u128(STEP_B));
    let layout = visible_layout(&document, &view, Detail::Names);
    assert_eq!(layout.rect_of(Id::from_u128(STEP_B)), None);
    assert_eq!(layout.rect_of(Id::from_u128(FLOW)), None);
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
