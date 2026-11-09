//! Renders the graphic diagram to PNG files for a visual check (temporary, not shipped).
use prism::actions::*;
use prism::model::*;
use prism::store::Store;
use prism::ui::graph::{Detail, load_fonts, rasterize, visible_layout};
use std::collections::BTreeSet;

fn element(id: u128, title: &str, body: &str, kind: ElementKind) -> ElementInput {
    ElementInput {
        id: Id::from_u128(id),
        title: title.into(),
        body: body.into(),
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

fn main() {
    let out = std::env::args().nth(1).expect("output directory");
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(directory.path().join("g.sqlite3")).unwrap();
    let workspace = store
        .workspace(&WorkspaceKey {
            project: "/p".into(),
            agent: AgentKind::Claude,
            conversation: "s".into(),
        })
        .unwrap();
    let request = PublishRequest {
        request_id: Id::new_v4(),
        document_id: None,
        expected_revision: None,
        title: "Prism architecture".into(),
        elements: vec![
            element(
                1,
                "Claude mod",
                "hooks/prism.js registers tools and the /prism command. It polls shared questions and fills the prompt box.",
                ElementKind::Step,
            ),
            element(
                2,
                "Skill",
                "prism-workspace tells the agent when to publish and how to answer.",
                ElementKind::Text,
            ),
            element(
                3,
                "Prompt box",
                "Filled by the mod with the item reference and quoted text.",
                ElementKind::Text,
            ),
            element(
                4,
                "Bridge",
                "Unix socket with a bearer token. Owns the adapter connection and the question dispatcher.",
                ElementKind::Step,
            ),
            element(
                5,
                "Questions",
                "Frozen selection snapshots with text and outcome per request id.",
                ElementKind::Text,
            ),
            element(
                6,
                "Store",
                ".prism/state.db, SQLite WAL schema 2: content, private views, questions.",
                ElementKind::Step,
            ),
            element(
                7,
                "Cards",
                "List view with expanded focused card and ask/hide/drop pills.",
                ElementKind::Text,
            ),
            element(
                8,
                "Diagram",
                "Image when the terminal has a protocol, text otherwise.",
                ElementKind::Text,
            ),
            element(
                9,
                "Binding verified",
                "The bridge refuses calls until tmux pane ancestry matches the binding.",
                ElementKind::Finding,
            ),
            element(
                10,
                "Clear draft after share?",
                "Open question about keeping context when retrieval is refused.",
                ElementKind::Question,
            ),
        ],
        relationships: vec![
            link(20, 1, 4, "publish", false),
            link(21, 4, 6, "commit revision", false),
            link(22, 6, 5, "share question", true),
            link(23, 5, 3, "fill prompt", true),
            link(24, 6, 7, "rows", false),
            link(25, 6, 8, "layout", false),
            link(26, 9, 4, "gates", false),
            link(27, 10, 5, "about", false),
        ],
        groups: vec![
            GroupInput {
                id: Id::from_u128(30),
                title: "CLI pane".into(),
                members: [1u128, 2, 3].into_iter().map(Id::from_u128).collect(),
            },
            GroupInput {
                id: Id::from_u128(31),
                title: "Bridge".into(),
                members: [4u128, 5].into_iter().map(Id::from_u128).collect(),
            },
            GroupInput {
                id: Id::from_u128(32),
                title: "Prism UI pane".into(),
                members: [6u128, 7, 8].into_iter().map(Id::from_u128).collect(),
            },
        ],
    };
    let document_id = store.publish(workspace.id, &request).unwrap().document_id;
    let document = store.document(workspace.id, document_id).unwrap();
    let view = ViewState {
        id: Id::new_v4(),
        document_id,
        kind: ViewKind::Diagram,
        selection: None,
        positions: Default::default(),
        hidden: Default::default(),
        filter: String::new(),
        draft: String::new(),
    };
    let fonts = load_fonts().expect("fonts");
    // A 78x40 cell pane at 8x17 px per cell.
    let (w, h) = (78 * 8, 40 * 17);
    for (name, detail, zoom) in [("fit", Detail::Names, 1.0f32), ("zoom", Detail::Full, 1.0)] {
        let layout = visible_layout(&document, &view, detail);
        let b = layout.bounds;
        let fit = (w as f32 / b.width).min(h as f32 / b.height).min(1.5);
        let scale = if name == "fit" { fit } else { 1.2 };
        let svg = layout.svg(Some(Id::from_u128(6)), &BTreeSet::from([Id::from_u128(4)]));
        std::fs::write(format!("{out}/{name}.svg"), &svg).unwrap();
        let png = rasterize(&svg, w, h, scale, (0.0, 0.0), &fonts).unwrap();
        std::fs::write(format!("{out}/{name}.png"), png).unwrap();
        println!(
            "{name}: bounds {}x{} scale {scale:.2} detail {:?} zoom {zoom}",
            b.width, b.height, layout.detail
        );
    }
}
