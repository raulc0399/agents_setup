use prism_core::graph::{
    Detail, GraphLayout, KIND_ARROW, KIND_BOX, KIND_GATE, KIND_LANE, KIND_PILL,
};
use prism_core::model::*;
use std::collections::{BTreeMap, BTreeSet};

const STEP_A: u128 = 1;
const STEP_B: u128 = 2;
const FINDING: u128 = 3;
const FLOW: u128 = 10;
const GATE_LINK: u128 = 11;
const LANE: u128 = 20;
const REVISION: u64 = 1;

fn element(id: u128, order: u64, title: &str, kind: ElementKind) -> (Id, Element) {
    let id = Id::from_u128(id);
    let element = Element {
        id,
        title: title.into(),
        body: "Body text for the box".into(),
        kind,
        source: None,
        order,
        semantic_revision: REVISION,
        accepted_revision: None,
        archived: false,
    };
    (id, element)
}

fn link(id: u128, from: u128, to: u128, label: &str, manual: bool) -> (Id, Relationship) {
    let id = Id::from_u128(id);
    let relationship = Relationship {
        id,
        from: Id::from_u128(from),
        to: Id::from_u128(to),
        label: label.into(),
        direction: Direction::Directed,
        manual,
        semantic_revision: REVISION,
        accepted_revision: None,
        archived: false,
    };
    (id, relationship)
}

/// Two steps in one lane joined by a manual arrow, and a finding attached to the first.
fn fixture() -> (Document, BTreeSet<Id>) {
    let lane = Group {
        id: Id::from_u128(LANE),
        title: "Device".into(),
        members: [STEP_A, STEP_B].into_iter().map(Id::from_u128).collect(),
        semantic_revision: REVISION,
        accepted_revision: None,
        archived: false,
    };
    let document = Document {
        id: Id::from_u128(100),
        title: "Flow".into(),
        revision: REVISION,
        elements: BTreeMap::from([
            element(STEP_A, 0, "Collect", ElementKind::Step),
            element(STEP_B, 1, "Store", ElementKind::Step),
            element(FINDING, 2, "Token missing", ElementKind::Finding),
        ]),
        relationships: BTreeMap::from([
            link(FLOW, STEP_A, STEP_B, "hands over", true),
            link(GATE_LINK, FINDING, STEP_A, "blocks", false),
        ]),
        groups: BTreeMap::from([(lane.id, lane)]),
    };
    let visible = document
        .elements
        .keys()
        .chain(document.relationships.keys())
        .chain(document.groups.keys())
        .copied()
        .collect();
    (document, visible)
}

#[test]
fn a_finding_attached_to_one_box_becomes_its_gate_badge() {
    let (document, visible) = fixture();
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
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
    let (document, visible) = fixture();
    let layout = GraphLayout::new(&document, &visible, Detail::Names);
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
    let (document, visible) = fixture();
    let names = GraphLayout::new(&document, &visible, Detail::Names);
    let full = GraphLayout::new(&document, &visible, Detail::Full);
    assert!(full.bounds.height > names.bounds.height);
    assert!(
        full.svg(None, &BTreeSet::new())
            .contains("Body text for the box")
    );
    assert!(!names.svg(None, &BTreeSet::new()).contains("Body text"));
}

#[test]
fn only_visible_content_is_laid_out() {
    let (document, mut visible) = fixture();
    visible.remove(&Id::from_u128(STEP_B));
    let layout = GraphLayout::new(&document, &visible, Detail::Names);
    assert_eq!(layout.rect_of(Id::from_u128(STEP_B)), None);
    assert_eq!(
        layout.rect_of(Id::from_u128(FLOW)),
        None,
        "an arrow to hidden content is left out"
    );
}

#[test]
fn archived_content_is_left_out_even_when_visible() {
    let (mut document, visible) = fixture();
    document
        .relationships
        .get_mut(&Id::from_u128(FLOW))
        .unwrap()
        .archived = true;
    let layout = GraphLayout::new(&document, &visible, Detail::Names);
    assert!(layout.rect_of(Id::from_u128(STEP_B)).is_some());
    assert_eq!(layout.rect_of(Id::from_u128(FLOW)), None);

    document
        .elements
        .get_mut(&Id::from_u128(STEP_B))
        .unwrap()
        .archived = true;
    document
        .relationships
        .get_mut(&Id::from_u128(FLOW))
        .unwrap()
        .archived = false;
    let layout = GraphLayout::new(&document, &visible, Detail::Names);
    assert_eq!(layout.rect_of(Id::from_u128(STEP_B)), None);
    assert_eq!(
        layout.rect_of(Id::from_u128(FLOW)),
        None,
        "an arrow to an archived box is left out"
    );
}

struct Tagged {
    kind: String,
    id: Id,
    open: String,
    inner: String,
}

/// Splits the SVG into its clickable groups, in document order, and the markup outside them.
fn split(svg: &str) -> (Vec<Tagged>, String) {
    let attribute = |tag: &str, name: &str| {
        let start = tag.find(&format!(r#" {name}=""#)).unwrap() + name.len() + 3;
        tag[start..].split('"').next().unwrap().to_owned()
    };
    let mut groups = Vec::new();
    let mut outside = String::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<g ") {
        let open_end = start + rest[start..].find('>').unwrap();
        let close = open_end + rest[open_end..].find("</g>").unwrap();
        let open = rest[start..open_end].to_owned();
        let inner = rest[open_end + 1..close].to_owned();
        assert!(!inner.contains("<g "), "clickable groups do not nest");
        outside.push_str(&rest[..start]);
        groups.push(Tagged {
            kind: attribute(&open, "data-kind"),
            id: Id::parse_str(&attribute(&open, "data-id")).unwrap(),
            open,
            inner,
        });
        rest = &rest[close + "</g>".len()..];
    }
    outside.push_str(rest);
    (groups, outside)
}

#[test]
fn every_clickable_shape_is_tagged_and_wraps_its_own_shape() {
    let (document, visible) = fixture();
    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    let (groups, outside) = split(&svg);
    let shapes: Vec<(&str, Id)> = groups
        .iter()
        .map(|group| (group.kind.as_str(), group.id))
        .collect();
    assert_eq!(
        shapes,
        vec![
            (KIND_LANE, Id::from_u128(LANE)),
            (KIND_ARROW, Id::from_u128(FLOW)),
            (KIND_BOX, Id::from_u128(STEP_A)),
            (KIND_BOX, Id::from_u128(STEP_B)),
            (KIND_PILL, Id::from_u128(FLOW)),
            (KIND_GATE, Id::from_u128(FINDING)),
        ]
    );
    let group = |kind: &str, id: u128| {
        groups
            .iter()
            .find(|group| group.kind == kind && group.id == Id::from_u128(id))
            .unwrap()
    };
    assert!(group(KIND_LANE, LANE).inner.contains(">DEVICE<"));
    assert!(group(KIND_BOX, STEP_A).inner.contains(">Collect<"));
    assert!(group(KIND_BOX, STEP_B).inner.contains(">Store<"));
    assert!(
        group(KIND_GATE, FINDING)
            .inner
            .contains(">! Token missing<")
    );
    assert!(group(KIND_PILL, FLOW).inner.contains(">1 hands over<"));
    let arrow = &group(KIND_ARROW, FLOW).inner;
    assert_eq!(arrow.matches("<path ").count(), 2);
    assert!(arrow.contains(r#"stroke-width="16" pointer-events="stroke""#));

    assert!(
        group(KIND_BOX, STEP_A)
            .open
            .contains(r#"class="prism-box prism-step""#)
    );
    assert!(
        group(KIND_GATE, FINDING)
            .open
            .contains(r#"class="prism-gate prism-finding""#)
    );
    assert!(
        group(KIND_ARROW, FLOW)
            .open
            .contains(r#"class="prism-arrow prism-manual""#)
    );
    assert!(
        group(KIND_PILL, FLOW)
            .open
            .contains(r#"class="prism-pill prism-manual""#)
    );
    assert!(
        group(KIND_LANE, LANE)
            .open
            .contains(r#"class="prism-lane""#)
    );
    assert!(groups.iter().all(|group| {
        group.open.contains(r#"tabindex="0""#) && group.open.contains(r#"role="button""#)
    }));

    // Outside the groups only the background and the arrowhead definitions remain.
    let outside = outside.split("</defs>").nth(1).unwrap();
    assert_eq!(outside.matches("<rect ").count(), 1);
    assert!(!outside.contains("<text") && !outside.contains("<path"));
}

#[test]
fn titles_are_escaped() {
    let (mut document, visible) = fixture();
    document
        .elements
        .get_mut(&Id::from_u128(STEP_A))
        .unwrap()
        .title = r#"A<b & "c""#.into();
    let svg = GraphLayout::new(&document, &visible, Detail::Names).svg(None, &BTreeSet::new());
    assert!(svg.contains(">A&lt;b &amp; &quot;c&quot;<"));
    assert!(!svg.contains("A<b"));
}
