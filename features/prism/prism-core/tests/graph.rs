use prism_core::graph::{
    ARROW_COLORS, ARROW_END_RUN, ARROW_HEAD_SIZE, Detail, GraphLayout, KIND_ARROW, KIND_BOX,
    KIND_GATE, KIND_LANE, KIND_PILL, WorldRect,
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
        created_revision: None,
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
        order: None,
        created_revision: None,
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
        order: None,
        created_revision: None,
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
        lanes: None,
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
    assert!(svg.contains(r#"aria-label="Step 1, link 4, hands over: Collect to Store""#));
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
fn steps_give_the_number_every_pill_shows_and_none_for_a_badge() {
    let (mut document, mut visible) = fixture();
    let (own, relationship) = link(12, STEP_B, STEP_A, "5 attach", false);
    document.relationships.insert(own, relationship);
    visible.insert(own);
    let (_, steps) = GraphLayout::bounds_and_steps(&document, &visible, Detail::Names);
    assert_eq!(
        steps,
        BTreeMap::from([(Id::from_u128(FLOW), 1), (own, 2)]),
        "arrows count from 1 in publication order; the badge's link has none"
    );
    for detail in Detail::ALL {
        let layout = GraphLayout::new(&document, &visible, detail);
        assert_eq!(
            layout.ids()[layout.ids().len() - 2..],
            [Id::from_u128(FLOW), own],
            "J and K go through the arrows in step order"
        );
        let svg = layout.svg(None, &BTreeSet::new());
        let (groups, _) = split(&svg);
        let pills: BTreeMap<Id, usize> = groups
            .iter()
            .filter(|group| group.kind == KIND_PILL)
            .map(|group| {
                let shown = group.inner.rsplit_once("\">").unwrap().1;
                let number = shown.trim_end_matches("</text>").split(' ').next().unwrap();
                (group.id, number.parse().unwrap())
            })
            .collect();
        assert_eq!(pills, steps, "{detail:?}");
    }
    // A label's own number is text after the step.
    let svg = GraphLayout::new(&document, &visible, Detail::Full).svg(None, &BTreeSet::new());
    assert!(svg.contains(r#"aria-label="Step 2, link 6, 5 attach: Store to Collect""#));
    assert!(svg.contains("<title>Step 2, link 6, 5 attach</title>"));
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
fn dropped_content_keeps_its_place_and_number_and_is_drawn_faded() {
    let (mut document, visible) = fixture();
    let before = GraphLayout::new(&document, &visible, Detail::Names);
    let boxes = [STEP_A, STEP_B].map(|id| before.rect_of(Id::from_u128(id)));

    // A dropped arrow stays, faded and dashed lightly.
    document
        .relationships
        .get_mut(&Id::from_u128(FLOW))
        .unwrap()
        .archived = true;
    let layout = GraphLayout::new(&document, &visible, Detail::Names);
    assert_eq!(
        layout.rect_of(Id::from_u128(FLOW)),
        before.rect_of(Id::from_u128(FLOW))
    );
    let svg = layout.svg(None, &BTreeSet::new());
    assert!(svg.contains(r#"stroke-dasharray="2 4""#));
    assert!(svg.contains(r#"aria-label="Step 1, link 4, hands over: Collect to Store (dropped)""#));

    // A dropped box keeps its place and number, with its arrow and the dropped label.
    document
        .elements
        .get_mut(&Id::from_u128(STEP_B))
        .unwrap()
        .archived = true;
    let layout = GraphLayout::new(&document, &visible, Detail::Names);
    assert_eq!(
        [STEP_A, STEP_B].map(|id| layout.rect_of(Id::from_u128(id))),
        boxes
    );
    let svg = layout.svg(None, &BTreeSet::new());
    let (groups, _) = split(&svg);
    let dropped: Vec<_> = groups
        .iter()
        .filter(|group| group.open.contains("prism-dropped"))
        .map(|group| (group.kind.as_str(), group.id))
        .collect();
    assert_eq!(
        dropped,
        vec![
            (KIND_ARROW, Id::from_u128(FLOW)),
            (KIND_BOX, Id::from_u128(STEP_B)),
            (KIND_PILL, Id::from_u128(FLOW))
        ]
    );
    let store = groups
        .iter()
        .find(|group| group.kind == KIND_BOX && group.id == Id::from_u128(STEP_B))
        .unwrap();
    assert!(store.open.contains(r#"opacity="0.45""#));
    assert!(store.open.contains(r#"aria-label="Step: Store (dropped)""#));
    assert!(store.inner.contains(">DROPPED<"));
    assert_eq!(box_numbers_as_listed(&svg, &document, &visible).len(), 2);

    // A dropped group keeps its lane, named dropped.
    document
        .groups
        .get_mut(&Id::from_u128(LANE))
        .unwrap()
        .archived = true;
    let layout = GraphLayout::new(&document, &visible, Detail::Names);
    assert_eq!(
        [STEP_A, STEP_B].map(|id| layout.rect_of(Id::from_u128(id))),
        boxes
    );
    assert!(
        layout
            .svg(None, &BTreeSet::new())
            .contains(">DEVICE · DROPPED<")
    );
}

struct Tagged {
    kind: String,
    id: Id,
    open: String,
    inner: String,
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!(r#" {name}=""#))? + name.len() + 3;
    Some(tag[start..].split('"').next().unwrap().to_owned())
}

/// Splits the SVG into its clickable groups, in document order, and the markup outside them.
/// The lane of ungrouped boxes is a group without content ID; it counts as outside.
fn split(svg: &str) -> (Vec<Tagged>, String) {
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
        if let Some(id) = attribute(&open, "data-id") {
            groups.push(Tagged {
                kind: attribute(&open, "data-kind").unwrap(),
                id: Id::parse_str(&id).unwrap(),
                open,
                inner,
            });
        } else {
            outside.push_str(&rest[start..close + "</g>".len()]);
        }
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
            (KIND_GATE, Id::from_u128(FINDING)),
            (KIND_BOX, Id::from_u128(STEP_B)),
            (KIND_PILL, Id::from_u128(FLOW)),
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
            .open
            .contains(r#"aria-label="Finding on Collect: Token missing""#)
    );
    assert!(
        group(KIND_GATE, FINDING)
            .inner
            .contains(">! Token missing<")
    );
    let pill = &group(KIND_PILL, FLOW).inner;
    assert!(
        pill.contains("<title>Step 1, link 4, hands over</title>") && pill.contains(">1</text>")
    );
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
            .contains(r#"class="prism-arrow prism-manual prism-arrow-c0""#)
    );
    assert!(
        group(KIND_PILL, FLOW)
            .open
            .contains(r#"class="prism-pill prism-manual prism-arrow-c0""#)
    );
    assert!(
        group(KIND_LANE, LANE)
            .open
            .contains(r#"class="prism-lane""#)
    );
    for group in &groups {
        let stop = group
            .open
            .contains(r#"tabindex="0" role="button" aria-label=""#);
        if group.kind == KIND_ARROW {
            assert!(group.open.contains(r#"tabindex="-1" aria-hidden="true""#));
        } else {
            assert!(stop, "{} is a keyboard stop with a name", group.kind);
        }
    }

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

fn steps(count: u128) -> (Document, BTreeSet<Id>) {
    let elements = (1..=count)
        .map(|id| element(id, id as u64, &format!("Step {id}"), ElementKind::Step))
        .collect();
    let document = Document {
        id: Id::from_u128(100),
        title: "Steps".into(),
        revision: REVISION,
        elements,
        relationships: BTreeMap::new(),
        groups: BTreeMap::new(),
        lanes: None,
    };
    let visible = document.elements.keys().copied().collect();
    (document, visible)
}

fn with_link(document: &mut Document, visible: &mut BTreeSet<Id>, from: u128, to: u128) -> Id {
    let (id, relationship) = link(FLOW, from, to, "long label for the arrow", false);
    document.relationships.insert(id, relationship);
    visible.insert(id);
    id
}

fn box_rects(layout: &GraphLayout, count: u128) -> Vec<WorldRect> {
    (1..=count)
        .map(|id| layout.rect_of(Id::from_u128(id)).unwrap())
        .collect()
}

fn overlaps(a: &WorldRect, b: &WorldRect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

#[test]
fn a_pill_between_side_by_side_boxes_shows_its_number_and_covers_no_box() {
    let (mut document, mut visible) = steps(2);
    let arrow = with_link(&mut document, &mut visible, 1, 2);
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let pill = layout.rect_of(arrow).unwrap();
    let boxes = box_rects(&layout, 2);
    assert_eq!(boxes[0].y, boxes[1].y, "side by side");
    assert!(boxes.iter().all(|rect| !overlaps(&pill, rect)));
    let svg = layout.svg(None, &BTreeSet::new());
    assert!(svg.contains(r#"font-weight="700">1</text>"#));
    assert!(
        svg.contains(r#"aria-label="Step 1, link 3, long label for the arrow: Step 1 to Step 2""#)
    );
}

#[test]
fn a_pill_in_the_gap_between_rows_keeps_its_label() {
    let (mut document, mut visible) = steps(3);
    let arrow = with_link(&mut document, &mut visible, 1, 3);
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let pill = layout.rect_of(arrow).unwrap();
    let boxes = box_rects(&layout, 3);
    assert!(
        boxes[2].y > boxes[0].bottom(),
        "the target is in the next row"
    );
    assert!(boxes.iter().all(|rect| !overlaps(&pill, rect)));
    let svg = layout.svg(None, &BTreeSet::new());
    assert!(svg.contains(">1 long label for the arrow</text>"));
}

#[test]
fn an_arrow_past_a_box_in_its_row_runs_below_the_row() {
    let (mut document, mut visible) = steps(5);
    let arrow = with_link(&mut document, &mut visible, 1, 3);
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let boxes = box_rects(&layout, 3);
    assert!(boxes[0].y == boxes[2].y && boxes[1].x > boxes[0].x && boxes[1].x < boxes[2].x);
    let pill = layout.rect_of(arrow).unwrap();
    assert!(
        pill.y > boxes[1].bottom(),
        "the arrow and its pill run below the row, not through the box between"
    );
}

#[test]
fn badges_share_the_right_part_of_their_box() {
    let (mut document, mut visible) = steps(1);
    for (id, title) in [(50, "A finding with a long title"), (51, "Another one")] {
        let (key, finding) = element(id, id as u64, title, ElementKind::Finding);
        document.elements.insert(key, finding);
        visible.insert(key);
        let (key, relationship) = link(id + 10, id, 1, "blocks", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let node = layout.rect_of(Id::from_u128(1)).unwrap();
    let first = layout.rect_of(Id::from_u128(50)).unwrap();
    let second = layout.rect_of(Id::from_u128(51)).unwrap();
    assert!(!overlaps(&first, &second));
    for gate in [first, second] {
        assert!(gate.x >= node.x + node.width / 4.0 && gate.right() <= node.right());
    }
}

#[test]
fn keyboard_stops_follow_lanes_then_the_focus_order() {
    let (document, visible) = fixture();
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let svg = layout.svg(None, &BTreeSet::new());
    let (groups, _) = split(&svg);
    let stops: Vec<Id> = groups
        .iter()
        .filter(|group| group.open.contains(r#"tabindex="0""#))
        .map(|group| group.id)
        .collect();
    let mut expected = vec![Id::from_u128(LANE)];
    expected.extend(layout.ids());
    assert_eq!(stops, expected);
}

#[test]
fn many_badges_show_two_and_a_count_for_the_rest() {
    let (mut document, mut visible) = steps(1);
    for id in 50..55 {
        let (key, finding) = element(
            id,
            id as u64,
            &format!("Finding {id}"),
            ElementKind::Finding,
        );
        document.elements.insert(key, finding);
        visible.insert(key);
        let (key, relationship) = link(id + 10, id, 1, "blocks", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let node = layout.rect_of(Id::from_u128(1)).unwrap();
    let (groups, _) = split(&layout.svg(None, &BTreeSet::new()));
    let badges: Vec<&Tagged> = groups
        .iter()
        .filter(|group| group.kind == KIND_GATE)
        .collect();
    assert_eq!(badges.len(), 3, "two badges and one for the rest");
    let more = badges[2];
    assert_eq!(
        more.id,
        Id::from_u128(52),
        "the count stands for the first one left out"
    );
    assert!(more.inner.contains(">+3</text>"));
    assert!(
        more.open
            .contains(r#"aria-label="+3 more on Step 1: Finding 52, Finding 53, Finding 54""#)
    );
    for id in 50..53 {
        let rect = layout.rect_of(Id::from_u128(id)).unwrap();
        assert!(rect.x >= node.x && rect.right() <= node.right());
    }
    assert_eq!(layout.rect_of(Id::from_u128(53)), None);
}

/// Three lanes of two boxes: Device (1, 2), Staging (3, 4), Live (5, 6), badges on 1 and 4,
/// and arrows 1→2, 2→3, 3→4, 4→5 and 3→6, so that two arrows cross between Staging and Live.
fn three_lanes() -> (Document, BTreeSet<Id>) {
    let (mut document, mut visible) = steps(6);
    for (id, members, title) in [
        (30, [1, 2], "Device"),
        (31, [3, 4], "Staging"),
        (32, [5, 6], "Live"),
    ] {
        let group = Group {
            id: Id::from_u128(id),
            title: title.into(),
            members: members.into_iter().map(Id::from_u128).collect(),
            order: None,
            created_revision: None,
            semantic_revision: REVISION,
            accepted_revision: None,
            archived: false,
        };
        visible.insert(group.id);
        document.groups.insert(group.id, group);
    }
    for (id, target) in [(40, 1), (41, 4)] {
        let (key, finding) = element(id, id as u64, "A finding", ElementKind::Finding);
        document.elements.insert(key, finding);
        visible.insert(key);
        let (key, relationship) = link(id + 10, id, target, "blocks", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    for (id, from, to) in [(60, 1, 2), (61, 2, 3), (62, 3, 4), (63, 4, 5), (64, 3, 6)] {
        let (key, relationship) = link(id, from, to, "next", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    (document, visible)
}

fn number(text: &str) -> f32 {
    text.parse().unwrap()
}

/// The `d` of every arrow's visible line.
fn paths(svg: &str) -> Vec<(Id, String)> {
    let (groups, _) = split(svg);
    groups
        .iter()
        .filter(|group| group.kind == KIND_ARROW)
        .map(|group| {
            let line = group
                .inner
                .split(r#"class="prism-line" d=""#)
                .nth(1)
                .unwrap();
            (group.id, line.split('"').next().unwrap().to_owned())
        })
        .collect()
}

fn point(text: &str) -> (f32, f32) {
    let (x, y) = text.split_once(',').unwrap();
    (number(x), number(y))
}

/// The polyline of every arrow, from the `d` of its visible line: its `M` point, the bend of
/// every rounded corner (the control point of its `Q`) and its last point.
fn lines(svg: &str) -> Vec<(Id, Vec<(f32, f32)>)> {
    paths(svg)
        .into_iter()
        .map(|(id, path)| {
            let tokens: Vec<&str> = path.split(' ').collect();
            let mut points = vec![point(tokens[0].strip_prefix('M').unwrap())];
            points.extend(
                tokens
                    .iter()
                    .filter_map(|token| token.strip_prefix('Q'))
                    .map(point),
            );
            if tokens.len() > 1 {
                points.push(point(tokens[tokens.len() - 1].strip_prefix('L').unwrap()));
            }
            (id, points)
        })
        .collect()
}

/// The rectangle of every lane, from its shape.
fn lane_rects(svg: &str) -> Vec<WorldRect> {
    svg.split(r#"<g class="prism-lane"#)
        .skip(1)
        .map(|lane| {
            let shape = lane.split("/>").next().unwrap();
            let value = |name: &str| number(&attribute(shape, name).unwrap());
            WorldRect {
                x: value("x"),
                y: value("y"),
                width: value("width"),
                height: value("height"),
            }
        })
        .collect()
}

type Segment = ((f32, f32), (f32, f32));

/// The rectangle a level or upright line from `a` to `b` covers: zero high or wide.
fn span(a: (f32, f32), b: (f32, f32)) -> WorldRect {
    WorldRect {
        x: a.0.min(b.0),
        y: a.1.min(b.1),
        width: (a.0 - b.0).abs(),
        height: (a.1 - b.1).abs(),
    }
}

/// Nine ungrouped boxes in rows of three and 14 arrows: side by side, past a box in the row,
/// several into one side, up and down rows; the third is manual.
fn dense() -> (Document, BTreeSet<Id>) {
    arrows(9, &DENSE, 72)
}

const DENSE: [(u128, u128); 14] = [
    (1, 2),
    (2, 3),
    (1, 3),
    (3, 1),
    (1, 5),
    (2, 5),
    (3, 5),
    (4, 5),
    (6, 5),
    (9, 5),
    (7, 5),
    (5, 1),
    (8, 2),
    (7, 3),
];

/// `three_lanes` with more arrows across lanes, both ways, into one side.
fn crossing_lanes() -> (Document, BTreeSet<Id>) {
    let (mut document, mut visible) = three_lanes();
    for (id, from, to) in [(65, 1, 6), (66, 6, 1), (67, 5, 2), (68, 2, 6), (69, 1, 4)] {
        let (key, relationship) = link(id, from, to, "more", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    (document, visible)
}

fn segments(points: &[(f32, f32)]) -> Vec<Segment> {
    points
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .filter(|(a, b)| a != b)
        .collect()
}

const ORIENTATIONS: [Lanes; 2] = [Lanes::Rows, Lanes::Columns];

/// The document with its lanes laid out as `lanes`.
fn as_lanes(
    (document, visible): &(Document, BTreeSet<Id>),
    lanes: Lanes,
) -> (Document, BTreeSet<Id>) {
    let mut document = document.clone();
    document.lanes = Some(lanes);
    (document, visible.clone())
}

/// `rect` mirrored on the diagonal, so that column lanes read as rows.
fn transposed(rect: WorldRect) -> WorldRect {
    WorldRect {
        x: rect.y,
        y: rect.x,
        width: rect.height,
        height: rect.width,
    }
}

#[test]
fn no_two_arrows_share_a_line() {
    // Crowded lanes still share lines where more arrows pass a gap than fit in it (rows of
    // `crossing_lanes`, `dense` both ways), as before columns came.
    let cases = [
        (Lanes::Rows, three_lanes(), 5),
        (Lanes::Columns, three_lanes(), 5),
        (Lanes::Columns, crossing_lanes(), 10),
    ];
    for (lanes, fixture, count) in &cases {
        let (document, visible) = as_lanes(fixture, *lanes);
        let svg =
            GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
        assert_no_shared_line(&svg, *count);
    }
}

#[test]
fn arrows_past_a_box_in_a_lanes_last_row_keep_apart_and_off_its_edge() {
    // Rows of three: 7 -> 9 and 9 -> 7 both pass box 8 below the last row. As columns of five,
    // 1 -> 3 and 3 -> 1 pass box 2 right of the last column.
    for (lanes, ends) in [
        (Lanes::Rows, [(7, 9), (9, 7)]),
        (Lanes::Columns, [(1, 3), (3, 1)]),
    ] {
        let (document, visible) = as_lanes(&arrows(9, &ends, 0), lanes);
        let svg =
            GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
        assert_no_shared_line(&svg, 2);
        let lane = lane_rects(&svg)[0];
        for (id, points) in lines(&svg) {
            for (a, b) in segments(&points) {
                // Across the lane: level in a row lane, upright in a column lane.
                let (across, edge) = match lanes {
                    Lanes::Rows => ((a.1 == b.1).then_some(a.1), lane.bottom()),
                    Lanes::Columns => ((a.0 == b.0).then_some(a.0), lane.right()),
                };
                if let Some(at) = across {
                    assert!(
                        at <= edge - 4.0,
                        "{lanes:?}: {id} runs at {at}, on the lane's edge at {edge}"
                    );
                }
            }
        }
    }
}

/// Asserts that the SVG has `count` arrows and no two of them run on one line.
fn assert_no_shared_line(svg: &str, count: usize) {
    let arrows = lines(svg);
    assert_eq!(arrows.len(), count);
    for (index, (first, points)) in arrows.iter().enumerate() {
        for (second, others) in &arrows[index + 1..] {
            for (a, b) in segments(points) {
                for (c, d) in segments(others) {
                    let horizontal = a.1 == b.1;
                    if horizontal != (c.1 == d.1) {
                        continue;
                    }
                    let (level, other, range, other_range) = if horizontal {
                        (
                            a.1,
                            c.1,
                            (a.0.min(b.0), a.0.max(b.0)),
                            (c.0.min(d.0), c.0.max(d.0)),
                        )
                    } else {
                        (
                            a.0,
                            c.0,
                            (a.1.min(b.1), a.1.max(b.1)),
                            (c.1.min(d.1), c.1.max(d.1)),
                        )
                    };
                    let apart = range.1 + 16.0 < other_range.0 || other_range.1 + 16.0 < range.0;
                    assert!(
                        (level - other).abs() >= 12.0 || apart,
                        "{first} and {second} run on one line at {level}"
                    );
                }
            }
        }
    }
}

#[test]
fn lines_stay_in_their_lane_or_the_middle_of_a_gap_and_clear_of_badges() {
    for orientation in ORIENTATIONS {
        let (document, visible) = as_lanes(&three_lanes(), orientation);
        let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
        let svg = layout.svg(None, &BTreeSet::new());
        // Read as rows: a column lane's title is on top, not beside where its lines run.
        let (lanes, title, read) = match orientation {
            Lanes::Rows => (lane_rects(&svg), 22.0, None),
            Lanes::Columns => (
                lane_rects(&svg).into_iter().map(transposed).collect(),
                0.0,
                Some(transposed as fn(WorldRect) -> WorldRect),
            ),
        };
        assert_eq!(lanes.len(), 3);
        let badges: Vec<WorldRect> = [40, 41]
            .into_iter()
            .map(|id| layout.rect_of(Id::from_u128(id)).unwrap())
            .collect();
        for (id, points) in lines(&svg) {
            for (a, b) in segments(&points) {
                let line = span(a, b);
                for badge in &badges {
                    assert!(
                        !overlaps(&line, badge),
                        "{orientation:?}: {id} crosses a badge"
                    );
                }
                let line = read.map_or(line, |read| read(line));
                if line.height != 0.0 {
                    continue;
                }
                let y = line.y;
                let inside = lanes
                    .iter()
                    .any(|lane| y > lane.y + title && y < lane.bottom() - 2.0);
                let in_gap = lanes.windows(2).any(|pair| {
                    let middle = (pair[0].bottom() + pair[1].y) / 2.0;
                    // Two steps of 6 off the middle at most, give or take rounding.
                    y > pair[0].bottom() + 2.0 && y < pair[1].y - 2.0 && (y - middle).abs() < 12.01
                });
                assert!(
                    inside || in_gap,
                    "{orientation:?}: {id} runs at {y}, on a lane edge or title"
                );
            }
        }
    }
}

#[test]
fn arrows_into_one_side_of_a_box_end_apart() {
    let (mut document, mut visible) = steps(3);
    let (first, relationship) = link(60, 1, 3, "first", false);
    document.relationships.insert(first, relationship);
    let (second, relationship) = link(61, 2, 3, "second", false);
    document.relationships.insert(second, relationship);
    visible.extend([first, second]);
    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    let ends: Vec<(f32, f32)> = lines(&svg)
        .into_iter()
        .map(|(_, points)| *points.last().unwrap())
        .collect();
    assert_eq!(ends.len(), 2);
    assert_eq!(ends[0].1, ends[1].1, "both end on the top of box 3");
    assert!((ends[0].0 - ends[1].0).abs() >= 12.0, "apart: {ends:?}");
}

/// The color index of every arrow, from its class, in the SVG's order.
fn arrow_colors(svg: &str) -> Vec<(Id, usize)> {
    let (groups, _) = split(svg);
    groups
        .iter()
        .filter(|group| group.kind == KIND_ARROW)
        .map(|group| {
            let class = attribute(&group.open, "class").unwrap();
            let index = class.split("prism-arrow-c").nth(1).unwrap();
            (group.id, index.parse().unwrap())
        })
        .collect()
}

/// Ungrouped boxes 1..=`boxes` joined by arrows 70, 71, ... with the given ends, in order.
fn arrows(boxes: u128, ends: &[(u128, u128)], manual: u128) -> (Document, BTreeSet<Id>) {
    let (mut document, mut visible) = steps(boxes);
    for (id, (from, to)) in (70..).zip(ends) {
        let (key, relationship) = link(id, *from, *to, "next", id == manual);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    (document, visible)
}

#[test]
fn neighboring_arrows_differ_in_color() {
    // Connecting 1, Waiting 2, Connected 3, Retrying 4, Superseded 5, Closed 6, all in one
    // lane: every arrow is a neighbor of every other, so the first six differ.
    let ends = [
        (1, 2),
        (2, 1),
        (1, 3),
        (1, 4),
        (4, 3),
        (3, 4),
        (3, 5),
        (3, 6),
    ];
    let (document, visible) = arrows(6, &ends, 72);
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let svg = layout.svg(None, &BTreeSet::new());
    let colors = arrow_colors(&svg);
    let expected: Vec<(Id, usize)> = (70..)
        .zip([0, 1, 2, 3, 4, 5, 0, 1])
        .map(|(id, color)| (Id::from_u128(id), color))
        .collect();
    assert_eq!(
        colors, expected,
        "neighbors differ while a color is free, then the least used is reused"
    );
    assert_eq!(
        colors.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        layout.ids()[6..],
        "numbered in focus order"
    );
    let again = GraphLayout::new(&document, &visible, Detail::Subtitles);
    assert_eq!(
        arrow_colors(&again.svg(None, &BTreeSet::new())),
        colors,
        "the same document gets the same colors"
    );

    // Line, head and pill carry the color as presentation attributes; dashes stay.
    let (groups, _) = split(&svg);
    let manual = Id::from_u128(72);
    let arrow = groups
        .iter()
        .find(|group| group.kind == KIND_ARROW && group.id == manual)
        .unwrap();
    let pill = groups
        .iter()
        .find(|group| group.kind == KIND_PILL && group.id == manual)
        .unwrap();
    assert!(
        arrow
            .open
            .contains(r#"class="prism-arrow prism-manual prism-arrow-c2""#)
    );
    assert!(
        pill.open
            .contains(r#"class="prism-pill prism-manual prism-arrow-c2""#)
    );
    let color = ARROW_COLORS[2];
    assert!(arrow.inner.contains(&format!(r#"stroke="{color}""#)));
    assert!(arrow.inner.contains(r#"stroke-dasharray="6 5""#));
    assert!(arrow.inner.contains(r#"marker-end="url(#prism-head-c2)""#));
    assert!(svg.contains(&format!(
        r#"<marker id="prism-head-c2" class="prism-head-c2" viewBox="0 0 10 10" refX="9" refY="5" markerUnits="userSpaceOnUse" markerWidth="10" markerHeight="10" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{color}"/>"#
    )));
    assert!(pill.inner.contains(&format!(r#"fill="{color}""#)));
}

#[test]
fn arrows_leaving_one_box_differ_while_a_color_is_free() {
    // Into box 1 from 8, then out of it to 2..=7: every arrow touches box 1.
    let ends: Vec<(u128, u128)> = std::iter::once((8, 1))
        .chain((2..=7).map(|to| (1, to)))
        .collect();
    let (document, visible) = arrows(8, &ends, 0);
    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    let leaving: BTreeSet<usize> = arrow_colors(&svg)
        .into_iter()
        .filter(|(id, _)| id.as_u128() != 70)
        .map(|(_, color)| color)
        .collect();
    assert_eq!(leaving.len(), ARROW_COLORS.len());
}

/// The numbers the list shows (`rows` in app/ui/js/rows.js), written as it does: every record
/// shown, archived ones included, by position, then elements before links before groups,
/// then by ID as a string, numbered from 1. A link or group without a stored position gets
/// one after the highest stored position, links before groups, each by ID as a string.
fn list_numbers(document: &Document, visible: &BTreeSet<Id>) -> BTreeMap<Id, usize> {
    let stored = document
        .elements
        .values()
        .map(|item| item.order)
        .chain(
            document
                .relationships
                .values()
                .filter_map(|item| item.order),
        )
        .chain(document.groups.values().filter_map(|item| item.order))
        .max();
    let mut next = stored.map_or(0, |highest| highest + 1);
    let mut records: Vec<(u64, u8, String, Id)> = document
        .elements
        .values()
        .map(|item| (item.order, 0, item.id.to_string(), item.id))
        .collect();
    let mut links: Vec<_> = document.relationships.values().collect();
    links.sort_by_key(|item| item.id.to_string());
    for item in links {
        let position = item.order.unwrap_or_else(|| {
            next += 1;
            next - 1
        });
        records.push((position, 1, item.id.to_string(), item.id));
    }
    let mut groups: Vec<_> = document.groups.values().collect();
    groups.sort_by_key(|item| item.id.to_string());
    for item in groups {
        let position = item.order.unwrap_or_else(|| {
            next += 1;
            next - 1
        });
        records.push((position, 2, item.id.to_string(), item.id));
    }
    records.sort();
    records
        .into_iter()
        .map(|(_, _, _, id)| id)
        .filter(|id| visible.contains(id))
        .enumerate()
        .map(|(index, id)| (id, index + 1))
        .collect()
}

/// The number each box shows, checked against the number the list shows for it.
fn box_numbers_as_listed(
    svg: &str,
    document: &Document,
    visible: &BTreeSet<Id>,
) -> BTreeMap<Id, usize> {
    let shown = box_numbers(svg);
    let listed = list_numbers(document, visible);
    for (id, number) in &shown {
        assert_eq!(listed.get(id), Some(number), "box {id}");
    }
    shown
}

/// The number each box shows.
fn box_numbers(svg: &str) -> BTreeMap<Id, usize> {
    let (groups, _) = split(svg);
    groups
        .iter()
        .filter(|group| group.kind == KIND_BOX)
        .map(|group| {
            let text = group.inner.split(r#"class="prism-number""#).nth(1).unwrap();
            let number = text.split('>').nth(1).unwrap().split('<').next().unwrap();
            (group.id, number.parse().unwrap())
        })
        .collect()
}

#[test]
fn boxes_show_the_number_the_list_shows() {
    // Box 4 comes first by order, 2 and 3 tie on it, 1 is hidden, 5 archived keeps its
    // number, and the finding is a badge on 3 that keeps its number.
    let (mut document, mut visible) = steps(5);
    for (id, order) in [(4, 0), (2, 2), (3, 2)] {
        document.elements.get_mut(&Id::from_u128(id)).unwrap().order = order;
    }
    visible.remove(&Id::from_u128(1));
    document
        .elements
        .get_mut(&Id::from_u128(5))
        .unwrap()
        .archived = true;
    let (key, finding) = element(50, 1, "A finding", ElementKind::Finding);
    document.elements.insert(key, finding);
    visible.insert(key);
    let (key, relationship) = link(60, 50, 3, "blocks", false);
    document.relationships.insert(key, relationship);
    visible.insert(key);

    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    let shown = box_numbers_as_listed(&svg, &document, &visible);
    assert_eq!(
        shown,
        BTreeMap::from([
            (Id::from_u128(4), 1),
            (Id::from_u128(2), 3),
            (Id::from_u128(3), 4),
            (Id::from_u128(5), 5)
        ])
    );
}

#[test]
fn every_record_is_numbered_in_publication_order_as_the_list_numbers_it() {
    // Elements at 0, 2 and 4 with a link published at 1 between them; a link and a group
    // stored before they had a position follow the highest stored one, link first.
    let (mut document, visible) = steps(3);
    for (id, order) in [(1, 0), (2, 2), (3, 4)] {
        document.elements.get_mut(&Id::from_u128(id)).unwrap().order = order;
    }
    let mut visible = visible;
    let (key, mut between) = link(72, 1, 2, "first", false);
    between.order = Some(1);
    document.relationships.insert(key, between);
    visible.insert(key);
    let (key, legacy) = link(71, 2, 3, "second", false);
    document.relationships.insert(key, legacy);
    visible.insert(key);
    let group = Group {
        id: Id::from_u128(80),
        title: "Lane".into(),
        members: [1, 2].into_iter().map(Id::from_u128).collect(),
        order: None,
        created_revision: None,
        semantic_revision: REVISION,
        accepted_revision: None,
        archived: false,
    };
    visible.insert(group.id);
    document.groups.insert(group.id, group);

    assert_eq!(
        document.list_order(),
        [1, 72, 2, 3, 71, 80].map(Id::from_u128).to_vec()
    );
    let listed = list_numbers(&document, &visible);
    assert_eq!(
        document
            .list_order()
            .iter()
            .map(|id| listed[id])
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6]
    );
    let svg = GraphLayout::new(&document, &visible, Detail::Names).svg(None, &BTreeSet::new());
    assert_eq!(
        box_numbers_as_listed(&svg, &document, &visible),
        BTreeMap::from([
            (Id::from_u128(1), 1),
            (Id::from_u128(2), 3),
            (Id::from_u128(3), 4)
        ])
    );
    // Pills count the arrows in that order too, not by ID.
    assert!(svg.contains(r#"aria-label="Step 1, link 2, first: Step 1 to Step 2""#));
    assert!(svg.contains(r#"aria-label="Step 2, link 5, second: Step 2 to Step 3""#));
}

#[test]
fn a_box_shows_number_kind_title_and_one_line_of_text() {
    let (mut document, visible) = fixture();
    document
        .elements
        .get_mut(&Id::from_u128(STEP_A))
        .unwrap()
        .body =
        "First line of the text\nand a second line that runs on past the width of the box".into();
    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    let (groups, _) = split(&svg);
    let inner = &groups
        .iter()
        .find(|group| group.kind == KIND_BOX && group.id == Id::from_u128(STEP_A))
        .unwrap()
        .inner;
    let texts: Vec<&str> = inner
        .split("<text ")
        .skip(1)
        .map(|text| text.split('>').nth(1).unwrap().split('<').next().unwrap())
        .collect();
    assert_eq!(texts[..3], ["1", "STEP", "Collect"]);
    assert_eq!(texts.len(), 4, "one line of text");
    assert!(
        texts[3].starts_with("First line of the text and") && texts[3].ends_with('…'),
        "the text flows onto one line, clipped: {}",
        texts[3]
    );
}

/// The texts of one box, by class, in the SVG's order.
fn box_texts(svg: &str, id: u128, class: &str) -> Vec<String> {
    let (groups, _) = split(svg);
    let inner = &groups
        .iter()
        .find(|group| group.kind == KIND_BOX && group.id == Id::from_u128(id))
        .unwrap()
        .inner;
    inner
        .split(&format!(r#"<text class="{class}""#))
        .skip(1)
        .map(|text| {
            text.split('>')
                .nth(1)
                .unwrap()
                .split('<')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn details_go_from_narrow_titles_to_wide_boxes_with_the_whole_text() {
    let (mut document, visible) = steps(4);
    document.elements.get_mut(&Id::from_u128(1)).unwrap().title =
        "A title long enough to need a second line in the box".into();
    let layouts = Detail::ALL.map(|detail| GraphLayout::new(&document, &visible, detail));
    let widths = layouts
        .each_ref()
        .map(|layout| layout.rect_of(Id::from_u128(1)).unwrap().width);
    assert!(widths[0] < widths[1] && widths[1] < widths[2], "{widths:?}");
    assert!(layouts[0].bounds.width < layouts[1].bounds.width);
    assert!(layouts[1].bounds.width < layouts[2].bounds.width);
    let svgs = layouts
        .each_ref()
        .map(|layout| layout.svg(None, &BTreeSet::new()));
    assert_eq!(box_texts(&svgs[0], 1, "prism-name").len(), 1);
    assert!(box_texts(&svgs[0], 2, "prism-subtitle").is_empty());
    assert_eq!(
        box_texts(&svgs[1], 2, "prism-subtitle"),
        ["Body text for the box"]
    );
    let title = box_texts(&svgs[2], 1, "prism-name");
    assert!(
        title.len() == 2 && !title.concat().contains('…'),
        "the whole title: {title:?}"
    );
    assert_eq!(
        box_texts(&svgs[2], 2, "prism-body"),
        ["Body text for the box"]
    );
}

#[test]
fn the_full_detail_wraps_long_text_inside_its_box_and_clips_very_long_text() {
    let (mut document, visible) = steps(3);
    let long = "word ".repeat(40);
    let word = "x".repeat(90);
    document.elements.get_mut(&Id::from_u128(1)).unwrap().body = long.clone();
    document.elements.get_mut(&Id::from_u128(2)).unwrap().body = "word ".repeat(400);
    document.elements.get_mut(&Id::from_u128(3)).unwrap().body = word.clone();
    let layout = GraphLayout::new(&document, &visible, Detail::Full);
    let svg = layout.svg(None, &BTreeSet::new());

    let text = box_texts(&svg, 1, "prism-body");
    assert!(text.len() > 1, "wrapped: {text:?}");
    assert_eq!(text.join(" "), long.trim(), "the whole text");
    let clipped = box_texts(&svg, 2, "prism-body");
    assert!(
        clipped.last().unwrap().ends_with('…'),
        "a very long text is clipped"
    );
    assert!(
        clipped.join(" ").matches("word").count() < 400,
        "within a sensible number of lines: {}",
        clipped.len()
    );
    // A word longer than a line is broken into pieces of a whole line each: no line is longer.
    let pieces = box_texts(&svg, 3, "prism-body");
    assert!(pieces.len() > 1 && pieces.concat() == word, "{pieces:?}");
    let line = pieces[0].chars().count();
    assert!(
        text.iter()
            .chain(&clipped)
            .all(|other| other.chars().count() <= line),
        "lines are no longer than a line's width"
    );
    for id in 1..=3 {
        let rect = layout.rect_of(Id::from_u128(id)).unwrap();
        let rows = box_rows(&svg, id);
        assert!(
            rows.iter().all(|y| *y > rect.y && *y < rect.bottom()),
            "box {id} holds its lines: {rows:?} in {rect:?}"
        );
    }
    let heights: Vec<f32> = (1..=3)
        .map(|id| layout.rect_of(Id::from_u128(id)).unwrap().height)
        .collect();
    assert!(
        heights[0] < heights[1],
        "each box is as tall as its text: {heights:?}"
    );
}

/// The baseline of every text of one box.
fn box_rows(svg: &str, id: u128) -> Vec<f32> {
    let (groups, _) = split(svg);
    let inner = &groups
        .iter()
        .find(|group| group.kind == KIND_BOX && group.id == Id::from_u128(id))
        .unwrap()
        .inner;
    inner
        .split("<text ")
        .skip(1)
        .map(|text| number(&attribute(&format!(" {text}"), "y").unwrap()))
        .collect()
}

#[test]
fn control_characters_in_the_text_are_escaped_at_every_detail() {
    let (mut document, visible) = steps(1);
    document.elements.get_mut(&Id::from_u128(1)).unwrap().body =
        "Colored \u{1b}[31mred\u{7} text".into();
    for detail in Detail::ALL {
        let svg = GraphLayout::new(&document, &visible, detail).svg(None, &BTreeSet::new());
        assert!(
            !svg.contains(['\u{1b}', '\u{7}']),
            "{detail:?} writes no control character"
        );
        if detail != Detail::Names {
            assert!(
                svg.contains(r"\u{1b}[31mred"),
                "{detail:?} shows it escaped"
            );
        }
    }
}

#[test]
fn the_bounds_come_from_placing_the_boxes_alone() {
    for fixture in [three_lanes(), crossing_lanes(), dense()] {
        for lanes in ORIENTATIONS {
            let (document, visible) = as_lanes(&fixture, lanes);
            for detail in Detail::ALL {
                let layout = GraphLayout::new(&document, &visible, detail);
                assert_eq!(
                    GraphLayout::bounds_for(&document, &visible, detail),
                    layout.bounds,
                    "{lanes:?} {detail:?}"
                );
                assert_eq!(
                    GraphLayout::bounds_and_steps(&document, &visible, detail).0,
                    layout.bounds,
                    "{lanes:?} {detail:?}"
                );
                // Everything drawn is inside the bounds.
                let svg = layout.svg(None, &BTreeSet::new());
                for rect in lane_rects(&svg) {
                    assert!(rect.right() <= layout.bounds.width);
                    assert!(rect.bottom() <= layout.bounds.height);
                }
            }
        }
    }
}

#[test]
fn with_the_full_text_arrows_keep_clear_of_the_taller_boxes_of_a_row() {
    // Rows of three: 1 (short), 2 (tall), 3 (short); 4 to 6 below. 1 → 5 turns below the
    // row, 1 → 3 passes box 2, so both must run below the tall box 2.
    let (mut document, mut visible) = steps(6);
    document.elements.get_mut(&Id::from_u128(2)).unwrap().body = "word ".repeat(60);
    for (id, from, to) in [(60, 1, 5), (61, 1, 3)] {
        let (key, relationship) = link(id, from, to, "next", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    let layout = GraphLayout::new(&document, &visible, Detail::Full);
    let boxes = box_rects(&layout, 6);
    assert!(boxes[1].height > boxes[0].height + 100.0 && boxes[0].y == boxes[1].y);
    let svg = layout.svg(None, &BTreeSet::new());
    let arrows = lines(&svg);
    assert_eq!(arrows.len(), 2);
    for (id, points) in arrows {
        for (a, b) in segments(&points) {
            let line = span(a, b);
            assert!(!overlaps(&line, &boxes[1]), "{id} crosses the tall box 2");
        }
    }
    for id in [60, 61] {
        let pill = layout.rect_of(Id::from_u128(id)).unwrap();
        assert!(
            boxes.iter().all(|rect| !overlaps(&pill, rect)),
            "pill {id} on a box"
        );
    }
}

#[test]
fn details_are_named_in_lower_case_for_the_app() {
    assert_eq!(
        serde_json::to_string(&Detail::ALL).unwrap(),
        r#"["names","subtitles","full"]"#
    );
    assert_eq!(
        serde_json::from_str::<Detail>(r#""full""#).unwrap(),
        Detail::Full
    );
}

/// The sign of `value`: -1, 0 or 1 (`f32::signum` gives 1 for 0).
fn sign(value: f32) -> i8 {
    (value > 0.0) as i8 - (value < 0.0) as i8
}

/// The length of the straight run a polyline ends with: its last segment and the ones before
/// it that go on in the same direction.
fn end_run(points: &[(f32, f32)]) -> f32 {
    let segments = segments(points);
    let direction = |(a, b): &Segment| (sign(b.0 - a.0), sign(b.1 - a.1));
    let Some(last) = segments.last() else {
        return 0.0;
    };
    segments
        .iter()
        .rev()
        .take_while(|segment| direction(segment) == direction(last))
        .map(|(a, b)| (b.0 - a.0).abs() + (b.1 - a.1).abs())
        .sum()
}

#[test]
fn arrows_run_straight_into_their_heads() {
    // Side by side, past a box in the row, several arrows into one side, up and down rows.
    let mut one_lane = arrows(9, &DENSE, 0);
    // Across lanes, both ways, into one side.
    let mut lanes = crossing_lanes();
    for (document, _) in [&mut one_lane, &mut lanes] {
        for (index, relationship) in document.relationships.values_mut().enumerate() {
            if index % 3 == 0 {
                relationship.direction = Direction::Bidirectional;
            }
        }
    }
    let fixtures = ORIENTATIONS.into_iter().flat_map(|orientation| {
        [
            as_lanes(&one_lane, orientation),
            as_lanes(&lanes, orientation),
        ]
    });
    for (document, visible) in &fixtures.collect::<Vec<_>>() {
        for detail in Detail::ALL {
            let svg = GraphLayout::new(document, visible, detail).svg(None, &BTreeSet::new());
            let arrows = lines(&svg);
            assert!(arrows.len() >= 10, "{} arrows", arrows.len());
            for (id, points) in arrows {
                let end = end_run(&points);
                assert!(
                    end >= ARROW_END_RUN,
                    "{id} at {detail:?} ends in a run of {end}: {points:?}"
                );
                if document.relationships[&id].direction == Direction::Bidirectional {
                    let reversed: Vec<_> = points.iter().rev().copied().collect();
                    let start = end_run(&reversed);
                    assert!(
                        start >= ARROW_END_RUN,
                        "{id} at {detail:?} starts with a run of {start}: {points:?}"
                    );
                }
            }
        }
    }
}

/// Asserts that no two of the layout's pills overlap, at every detail and with lanes as rows
/// and as columns, and that the layout is the same when made again.
fn assert_no_pills_overlap(document: &Document, visible: &BTreeSet<Id>, count: usize) {
    let fixture = (document.clone(), visible.clone());
    for (lanes, detail) in ORIENTATIONS
        .into_iter()
        .flat_map(|lanes| Detail::ALL.map(|detail| (lanes, detail)))
    {
        let (document, visible) = &as_lanes(&fixture, lanes);
        let layout = GraphLayout::new(document, visible, detail);
        let svg = layout.svg(None, &BTreeSet::new());
        let pills: Vec<(Id, WorldRect)> = lines(&svg)
            .into_iter()
            .map(|(id, _)| (id, layout.rect_of(id).unwrap()))
            .collect();
        assert_eq!(pills.len(), count, "{lanes:?} {detail:?}");
        for (index, (first, rect)) in pills.iter().enumerate() {
            for (second, other) in &pills[index + 1..] {
                assert!(
                    !overlaps(rect, other),
                    "{lanes:?} {detail:?}: pill {first} {rect:?} overlaps pill {second} {other:?}"
                );
            }
        }
        assert_eq!(
            GraphLayout::new(document, visible, detail).svg(None, &BTreeSet::new()),
            svg,
            "{lanes:?} {detail:?}: the same document gets the same layout"
        );
    }
}

#[test]
fn pills_of_several_arrows_between_the_same_two_boxes_do_not_overlap() {
    // Rows of two: 1 and 4 sit diagonally apart, joined both ways.
    let (document, visible) = arrows(4, &[(1, 4), (1, 4), (4, 1), (1, 4)], 71);
    assert_no_pills_overlap(&document, &visible, 4);
}

#[test]
fn pills_of_many_arrows_into_one_box_side_do_not_overlap() {
    // Rows of three: 1, 2 and 3 into the top of 5, 7, 8 and 9 into its bottom.
    let (document, visible) = arrows(9, &[(1, 5), (2, 5), (3, 5), (7, 5), (8, 5), (9, 5)], 0);
    assert_no_pills_overlap(&document, &visible, 6);
}

#[test]
fn pills_of_crossing_arrows_between_lanes_do_not_overlap() {
    let (mut document, mut visible) = three_lanes();
    // Staging (3, 4) and Live (5, 6): arrows cross between the lanes both ways.
    for (id, from, to) in [
        (65, 3, 5),
        (66, 4, 6),
        (67, 3, 6),
        (68, 4, 5),
        (69, 6, 3),
        (70, 5, 4),
    ] {
        let (key, relationship) = link(id, from, to, "crosses over", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    assert_no_pills_overlap(&document, &visible, 11);
}

#[test]
fn pills_of_a_dense_diagram_do_not_overlap() {
    let (document, visible) = dense();
    assert_no_pills_overlap(&document, &visible, 14);
}

#[test]
fn a_text_box_shows_its_number_beside_the_title_and_is_one_line_shorter() {
    // Rows of three: step 1, text 2, step 3; 4 below. Arrows join the boxes of mixed height.
    let (mut document, mut visible) = steps(4);
    document.elements.get_mut(&Id::from_u128(2)).unwrap().kind = ElementKind::Text;
    for (id, from, to) in [(60, 1, 2), (61, 2, 3), (62, 2, 4), (63, 1, 3)] {
        let (key, relationship) = link(id, from, to, "next", false);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    for detail in Detail::ALL {
        let layout = GraphLayout::new(&document, &visible, detail);
        let boxes = box_rects(&layout, 4);
        assert_eq!(boxes[0].y, boxes[1].y, "{detail:?}: one row");
        assert_eq!(
            boxes[0].height - boxes[1].height,
            19.0,
            "{detail:?}: one head line shorter"
        );
        let svg = layout.svg(None, &BTreeSet::new());
        assert!(box_texts(&svg, 2, "prism-kind").is_empty(), "{detail:?}");
        assert_eq!(box_texts(&svg, 1, "prism-kind"), ["STEP"], "{detail:?}");
        assert_eq!(box_texts(&svg, 2, "prism-number"), ["2"], "{detail:?}");
        let rows = box_rows(&svg, 2);
        assert_eq!(rows[0], rows[1], "{detail:?}: number on the title's line");
        assert!(
            rows.iter()
                .all(|y| *y > boxes[1].y && *y < boxes[1].bottom()),
            "{detail:?}: the box holds its lines: {rows:?} in {:?}",
            boxes[1]
        );
        let step_rows = box_rows(&svg, 1);
        assert_eq!(
            step_rows[2] - boxes[0].y,
            rows[1] - boxes[1].y + 19.0,
            "{detail:?}: the title moves up by the head line"
        );
        for (id, points) in lines(&svg) {
            let relationship = &document.relationships[&id];
            for (a, b) in segments(&points) {
                let line = span(a, b);
                for (index, rect) in boxes.iter().enumerate() {
                    let own = [relationship.from, relationship.to]
                        .contains(&Id::from_u128(index as u128 + 1));
                    assert!(
                        own || !overlaps(&line, rect),
                        "{detail:?}: {id} crosses box {}",
                        index + 1
                    );
                }
            }
        }
    }

    // Dropped, it keeps its number and says so on the title's line.
    document
        .elements
        .get_mut(&Id::from_u128(2))
        .unwrap()
        .archived = true;
    document.elements.get_mut(&Id::from_u128(2)).unwrap().title = "A".repeat(60);
    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    assert_eq!(box_texts(&svg, 2, "prism-dropped-label"), ["DROPPED"]);
    assert_eq!(box_texts(&svg, 2, "prism-number"), ["2"]);
    let title = &box_texts(&svg, 2, "prism-name")[0];
    assert!(
        title.ends_with('…') && title.chars().count() < 20,
        "the title leaves room for the label: {title}"
    );
}

#[test]
fn pills_of_two_arrows_both_ways_between_side_by_side_boxes_do_not_overlap() {
    let (document, visible) = arrows(2, &[(1, 2), (2, 1)], 0);
    assert_no_pills_overlap(&document, &visible, 2);
}

#[test]
fn pills_of_several_arrows_in_one_column_do_not_overlap() {
    // Rows of two: 1 above 3.
    let (document, visible) = arrows(3, &[(1, 3), (3, 1), (1, 3)], 0);
    assert_no_pills_overlap(&document, &visible, 3);
}

/// The rectangle of every box, from the layout.
fn all_boxes(layout: &GraphLayout, svg: &str) -> Vec<(Id, WorldRect)> {
    let (groups, _) = split(svg);
    groups
        .iter()
        .filter(|group| group.kind == KIND_BOX)
        .map(|group| (group.id, layout.rect_of(group.id).unwrap()))
        .collect()
}

#[test]
fn no_arrow_runs_through_a_box() {
    let neighbors = [
        (1, 2),
        (2, 1),
        (1, 3),
        (1, 4),
        (4, 3),
        (3, 4),
        (3, 5),
        (3, 6),
    ];
    let documents = [
        fixture(),
        three_lanes(),
        crossing_lanes(),
        dense(),
        arrows(6, &neighbors, 72),
        arrows(9, &[(7, 9), (9, 7)], 0),
        arrows(4, &[(1, 4), (1, 4), (4, 1), (1, 4)], 71),
        arrows(2, &[(1, 2), (2, 1)], 0),
        arrows(3, &[(1, 3), (3, 1), (1, 3)], 0),
        side_by_side(&[(1, 2), (2, 1)], true),
        side_by_side(&[(1, 2), (2, 1), (1, 2)], false),
        side_by_side(&[(1, 2), (2, 1), (1, 2)], true),
    ];
    for (lanes, fixture) in ORIENTATIONS
        .into_iter()
        .flat_map(|lanes| documents.iter().map(move |fixture| (lanes, fixture)))
    {
        let (document, visible) = &as_lanes(fixture, lanes);
        for detail in Detail::ALL {
            let layout = GraphLayout::new(document, visible, detail);
            let svg = layout.svg(None, &BTreeSet::new());
            let boxes = all_boxes(&layout, &svg);
            for (id, points) in lines(&svg) {
                for (a, b) in segments(&points) {
                    let line = span(a, b);
                    for (node, rect) in &boxes {
                        assert!(
                            !overlaps(&line, rect),
                            "{lanes:?} {detail:?}: {id} runs through box {node}: {points:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn pills_keep_off_lane_borders_where_they_can() {
    // The app's release flow: its labels, box 6 a store.
    let (mut document, visible) = three_lanes();
    for (id, label) in [
        (60, "queue"),
        (61, "upload"),
        (62, "flag anomalies"),
        (63, "approve release"),
        (64, "store raw batch"),
    ] {
        document
            .relationships
            .get_mut(&Id::from_u128(id))
            .unwrap()
            .label = label.into();
    }
    document.elements.get_mut(&Id::from_u128(6)).unwrap().kind = ElementKind::Text;
    let fixtures = [(document, visible), crossing_lanes(), dense()];
    for (fixture, lanes, detail) in fixtures.iter().flat_map(|fixture| {
        ORIENTATIONS
            .into_iter()
            .flat_map(move |lanes| Detail::ALL.map(move |detail| (fixture, lanes, detail)))
    }) {
        let (document, visible) = as_lanes(fixture, lanes);
        let layout = GraphLayout::new(&document, &visible, detail);
        let svg = layout.svg(None, &BTreeSet::new());
        let lanes_drawn = lane_rects(&svg);
        // The borders arrows cross, between row lanes level and between column lanes upright:
        // not on them, nor flush against them. No pill straddles any other lane border.
        let read = |rect: WorldRect| match lanes {
            Lanes::Rows => rect,
            Lanes::Columns => transposed(rect),
        };
        for (id, _) in lines(&svg) {
            let pill = layout.rect_of(id).unwrap();
            for lane in &lanes_drawn {
                let (across, lane_across) = (read(pill), read(*lane));
                for y in [lane_across.y, lane_across.bottom()] {
                    assert!(
                        !(across.y - 4.0 < y && y < across.bottom() + 4.0),
                        "{lanes:?} {detail:?}: pill {id} {pill:?} on the lane border at {y}"
                    );
                }
                let straddles = |low: f32, high: f32, edge: f32| low < edge && edge < high;
                let beside = pill.y < lane.bottom() && lane.y < pill.bottom();
                let below = pill.x < lane.right() && lane.x < pill.right();
                assert!(
                    !(beside
                        && (straddles(pill.x, pill.right(), lane.x)
                            || straddles(pill.x, pill.right(), lane.right()))),
                    "{lanes:?} {detail:?}: pill {id} {pill:?} across the side of {lane:?}"
                );
                assert!(
                    !(below
                        && (straddles(pill.y, pill.bottom(), lane.y)
                            || straddles(pill.y, pill.bottom(), lane.bottom()))),
                    "{lanes:?} {detail:?}: pill {id} {pill:?} across the edge of {lane:?}"
                );
            }
        }
    }
}

/// Side-by-side boxes 1 and 2 joined by arrows with these ends; box 2 a store if `text`.
fn side_by_side(ends: &[(u128, u128)], text: bool) -> (Document, BTreeSet<Id>) {
    let (mut document, visible) = arrows(2, ends, 0);
    if text {
        document.elements.get_mut(&Id::from_u128(2)).unwrap().kind = ElementKind::Text;
    }
    (document, visible)
}

#[test]
fn pills_of_arrows_between_side_by_side_boxes_with_short_sides_do_not_overlap() {
    // Too short a side for the lines' pills: the extra arrows run below the row.
    for (ends, text) in [
        (&[(1, 2), (2, 1)][..], true),
        (&[(1, 2), (2, 1), (1, 2)][..], false),
        (&[(1, 2), (2, 1), (1, 2)][..], true),
    ] {
        let (document, visible) = side_by_side(ends, text);
        assert_no_pills_overlap(&document, &visible, ends.len());
    }
}

#[test]
fn a_dropped_text_box_wraps_its_whole_title_beside_the_label() {
    let (mut document, visible) = steps(1);
    let title = "Archive raw batches for the audit trail of every device";
    let element = document.elements.get_mut(&Id::from_u128(1)).unwrap();
    element.kind = ElementKind::Text;
    element.archived = true;
    element.title = title.into();
    let svg = GraphLayout::new(&document, &visible, Detail::Full).svg(None, &BTreeSet::new());
    assert_eq!(box_texts(&svg, 1, "prism-dropped-label"), ["DROPPED"]);
    let lines = box_texts(&svg, 1, "prism-name");
    assert_eq!(lines.join(" "), title, "every word: {lines:?}");
}

/// `boxes` boxes of every kind in `lanes` lanes, some dropped, and `links` arrows between
/// boxes picked by a fixed pseudo-random sequence.
fn large(boxes: u128, links: u128, lanes: u128) -> (Document, BTreeSet<Id>) {
    let (mut document, mut visible) = steps(boxes);
    let kinds = [ElementKind::Text, ElementKind::Step, ElementKind::Finding];
    for (id, element) in document.elements.iter_mut() {
        element.kind = kinds[(id.as_u128() % 3) as usize].clone();
        element.archived = id.as_u128() % 17 == 0;
    }
    let per_lane = boxes / lanes;
    for lane in 0..lanes {
        let group = Group {
            id: Id::from_u128(1_000_000 + lane),
            title: format!("Lane {lane}"),
            members: (lane * per_lane + 1..=(lane + 1) * per_lane)
                .map(Id::from_u128)
                .collect(),
            order: None,
            created_revision: None,
            semantic_revision: REVISION,
            accepted_revision: None,
            archived: false,
        };
        visible.insert(group.id);
        document.groups.insert(group.id, group);
    }
    let mut seed: u64 = 7;
    let mut pick = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as u128 % boxes + 1
    };
    for index in 0..links {
        let from = pick();
        let mut to = pick();
        while to == from {
            to = pick();
        }
        let (key, mut relationship) = link(2_000_000 + index, from, to, "label", index % 5 == 0);
        relationship.direction = [
            Direction::Directed,
            Direction::Bidirectional,
            Direction::Undirected,
        ][(index % 3) as usize]
            .clone();
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    (document, visible)
}

/// A bound far above what laying out a large diagram takes in the test profile (some seconds),
/// far below what a search through everything for each pill takes (minutes).
const LARGE_LAYOUT_SECONDS: u64 = 60;

#[test]
fn a_large_diagram_lays_out_in_time() {
    let fixture = large(500, 1000, 20);
    for lanes in ORIENTATIONS {
        let (document, visible) = as_lanes(&fixture, lanes);
        let start = std::time::Instant::now();
        let layout = GraphLayout::new(&document, &visible, Detail::Names);
        let elapsed = start.elapsed();
        assert!(layout.edge_count() > 900);
        assert!(
            elapsed < std::time::Duration::from_secs(LARGE_LAYOUT_SECONDS),
            "{lanes:?}: 500 boxes and 1000 arrows took {elapsed:?}"
        );
    }
}

/// `lanes` lanes of `per_lane` boxes each, numbered on from 1 lane by lane, joined by arrows
/// 70, 71, ... with the given ends.
fn grouped(lanes: u128, per_lane: u128, ends: &[(u128, u128)]) -> (Document, BTreeSet<Id>) {
    let (mut document, mut visible) = arrows(lanes * per_lane, ends, 0);
    for lane in 0..lanes {
        let group = Group {
            id: Id::from_u128(30 + lane),
            title: format!("Lane {lane}"),
            members: (lane * per_lane + 1..=(lane + 1) * per_lane)
                .map(Id::from_u128)
                .collect(),
            order: None,
            created_revision: None,
            semantic_revision: REVISION,
            accepted_revision: None,
            archived: false,
        };
        visible.insert(group.id);
        document.groups.insert(group.id, group);
    }
    (document, visible)
}

#[test]
fn the_orientation_follows_the_flow_and_the_canvas_shape() {
    let chosen = |(document, visible): &(Document, BTreeSet<Id>)| {
        let lanes = GraphLayout::orientation(document);
        for detail in Detail::ALL {
            assert_eq!(
                GraphLayout::new(document, visible, detail).orientation,
                lanes,
                "the same at {detail:?}"
            );
        }
        lanes
    };
    // How far a shape is off 16:10, as a factor of at least 1.
    let misfit = |fixture: &(Document, BTreeSet<Id>), lanes| {
        let (document, visible) = as_lanes(fixture, lanes);
        let bounds = GraphLayout::bounds_for(&document, &visible, Detail::Subtitles);
        let ratio = bounds.width / bounds.height / 1.6;
        ratio.max(1.0 / ratio)
    };
    // One lane keeps rows.
    assert_eq!(chosen(&dense()), Lanes::Rows);
    // Most arrows hand over from lane to lane: columns; most stay inside a lane: rows.
    let handover = grouped(3, 4, &[(1, 5), (5, 9), (2, 6), (6, 10)]);
    assert_eq!(chosen(&handover), Lanes::Columns);
    let inside = grouped(3, 2, &[(1, 2), (3, 4), (5, 6), (2, 3)]);
    assert_eq!(chosen(&inside), Lanes::Rows);
    // Against the flow where the other shape is more than 1.5 times closer to 16:10: lanes of
    // two boxes make flat columns, lanes of four a tall stack of rows.
    let flat = three_lanes();
    assert!(misfit(&flat, Lanes::Columns) > 1.5 * misfit(&flat, Lanes::Rows));
    assert_eq!(chosen(&flat), Lanes::Rows);
    let tall = grouped(3, 4, &[(1, 2), (2, 3), (5, 6), (9, 10), (4, 5)]);
    assert!(misfit(&tall, Lanes::Rows) > 1.5 * misfit(&tall, Lanes::Columns));
    assert_eq!(chosen(&tall), Lanes::Columns);
    // Without a majority the shape closer to 16:10 decides.
    for (fixture, lanes) in [
        (grouped(3, 2, &[(1, 2), (2, 3)]), Lanes::Rows),
        (grouped(3, 2, &[]), Lanes::Rows),
        (grouped(3, 6, &[]), Lanes::Columns),
    ] {
        assert_eq!(chosen(&fixture), lanes);
        let other = ORIENTATIONS
            .into_iter()
            .find(|other| *other != lanes)
            .unwrap();
        assert!(misfit(&fixture, lanes) < misfit(&fixture, other));
    }
    // It is the whole document's, whatever is shown: hiding items never flips it.
    let (document, visible) = three_lanes();
    for hidden in [1, 3, 30, 60, 61, 63, 64] {
        let mut shown = visible.clone();
        shown.remove(&Id::from_u128(hidden));
        assert_eq!(
            GraphLayout::new(&document, &shown, Detail::Subtitles).orientation,
            chosen(&(document.clone(), visible.clone()))
        );
    }
    // The agent's choice wins.
    assert_eq!(
        chosen(&as_lanes(&three_lanes(), Lanes::Columns)),
        Lanes::Columns
    );
    assert_eq!(chosen(&as_lanes(&dense(), Lanes::Columns)), Lanes::Columns);
}

#[test]
fn column_lanes_stand_side_by_side_and_fill_top_to_bottom() {
    let rows = as_lanes(&three_lanes(), Lanes::Rows);
    let (document, visible) = as_lanes(&three_lanes(), Lanes::Columns);
    for detail in Detail::ALL {
        let layout = GraphLayout::new(&document, &visible, detail);
        let svg = layout.svg(None, &BTreeSet::new());
        let lanes = lane_rects(&svg);
        assert_eq!(lanes.len(), 3);
        for pair in lanes.windows(2) {
            assert_eq!((pair[0].y, pair[0].height), (pair[1].y, pair[1].height));
            assert!(pair[1].x - pair[0].right() >= 48.0, "{detail:?}: {lanes:?}");
        }
        let boxes = box_rects(&layout, 6);
        for (lane, pair) in lanes.iter().zip(boxes.chunks(2)) {
            assert_eq!(pair[0].x, pair[1].x, "{detail:?}: one column");
            assert!(pair[1].y > pair[0].bottom(), "{detail:?}: top to bottom");
            assert!(pair.iter().all(|rect| overlaps(rect, lane)));
        }
        assert!(
            boxes[0].y == boxes[2].y && boxes[2].y == boxes[4].y,
            "{detail:?}: the rows of all lanes line up"
        );
        // Badges sit on top of their box.
        let badge = layout.rect_of(Id::from_u128(40)).unwrap();
        assert!(badge.y < boxes[0].y && badge.right() <= boxes[0].right());

        // Inside a lane an arrow runs down; from lane to lane it leaves right and enters left.
        let rect = |id: u128| boxes[id as usize - 1];
        for (id, points) in lines(&svg) {
            let relationship = &document.relationships[&id];
            let (from, to) = (
                rect(relationship.from.as_u128()),
                rect(relationship.to.as_u128()),
            );
            let (first, last) = (points[0], *points.last().unwrap());
            if from.x == to.x {
                assert_eq!((first.1, last.1), (from.bottom(), to.y), "{detail:?}: {id}");
            } else {
                assert_eq!((first.0, last.0), (from.right(), to.x), "{detail:?}: {id}");
            }
        }

        // The same order, numbers and steps as in rows; hits answer where the shapes are.
        let in_rows = GraphLayout::new(&rows.0, &rows.1, detail);
        assert_eq!(layout.ids(), in_rows.ids(), "{detail:?}");
        assert_eq!(
            GraphLayout::bounds_and_steps(&document, &visible, detail).1,
            GraphLayout::bounds_and_steps(&rows.0, &rows.1, detail).1
        );
        box_numbers_as_listed(&svg, &document, &visible);
        let middle = |rect: WorldRect| (rect.center_x(), rect.center_y());
        for id in [1, 6] {
            let (x, y) = middle(rect(id));
            assert_eq!(layout.hit(x, y), Some(Id::from_u128(id)));
        }
        for (id, points) in lines(&svg) {
            let (x, y) = middle(layout.rect_of(id).unwrap());
            assert_eq!(layout.hit(x, y), Some(id), "{detail:?}: pill of {id}");
            // Somewhere along its line, outside pills and boxes, the arrow answers.
            let along = segments(&points).into_iter().flat_map(|(a, b)| {
                [0.25, 0.5, 0.75]
                    .map(|share| (a.0 + (b.0 - a.0) * share, a.1 + (b.1 - a.1) * share))
            });
            assert!(
                along.into_iter().any(|(x, y)| layout.hit(x, y) == Some(id)),
                "{detail:?}: line of {id}"
            );
        }
    }
}

#[test]
fn arrow_bends_are_rounded_and_heads_sit_on_straight_runs() {
    let mut bidirectional = crossing_lanes();
    for relationship in bidirectional.0.relationships.values_mut() {
        relationship.direction = Direction::Bidirectional;
    }
    let fixtures = [dense(), three_lanes(), bidirectional];
    let distance = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() + (a.1 - b.1).abs();
    let mut rounded = 0;
    for (lanes, fixture) in ORIENTATIONS
        .into_iter()
        .flat_map(|lanes| fixtures.iter().map(move |fixture| (lanes, fixture)))
    {
        let (document, visible) = as_lanes(fixture, lanes);
        for detail in Detail::ALL {
            let svg = GraphLayout::new(&document, &visible, detail).svg(None, &BTreeSet::new());
            let (groups, _) = split(&svg);
            for (id, path) in paths(&svg) {
                // The pointer's hit band follows the same rounded line.
                let arrow = groups
                    .iter()
                    .find(|group| group.kind == KIND_ARROW && group.id == id)
                    .unwrap();
                assert!(
                    arrow
                        .inner
                        .contains(&format!(r#"class="prism-hit" d="{path}""#))
                );
                assert!(arrow.inner.contains(r#"stroke-width="1.5""#));

                let tokens: Vec<&str> = path.split(' ').collect();
                let bends: Vec<usize> = (0..tokens.len())
                    .filter(|index| tokens[*index].starts_with('Q'))
                    .collect();
                rounded += bends.len();
                for index in &bends {
                    // L before, Q at the bend, the curve's end after: within the radius.
                    let bend = point(&tokens[*index][1..]);
                    let before = point(&tokens[index - 1][1..]);
                    let after = point(tokens[index + 1]);
                    assert!(
                        distance(before, bend) <= 8.0 + 1e-3 && distance(bend, after) <= 8.0 + 1e-3
                    );
                }
                // The last piece, and with a start head the first, is straight and holds the head.
                let tip = point(&tokens[tokens.len() - 1][1..]);
                let end = bends
                    .last()
                    .map_or(point(&tokens[0][1..]), |index| point(tokens[index + 1]));
                assert!(
                    distance(end, tip) >= ARROW_HEAD_SIZE - 1e-3,
                    "{lanes:?} {detail:?}: {id} ends in {path}"
                );
                if document.relationships[&id].direction == Direction::Bidirectional
                    && let Some(first) = bends.first()
                {
                    let start = point(&tokens[0][1..]);
                    let curve = point(&tokens[first - 1][1..]);
                    assert!(
                        distance(start, curve) >= ARROW_HEAD_SIZE - 1e-3,
                        "{lanes:?} {detail:?}: {id} starts in {path}"
                    );
                }
            }
        }
    }
    assert!(rounded > 100, "{rounded} rounded bends");
}

#[test]
fn long_hand_over_labels_keep_their_text_in_columns() {
    // 24 characters: more than the old cap of the room between column lanes took.
    let (mut document, visible) = as_lanes(&three_lanes(), Lanes::Columns);
    for relationship in document.relationships.values_mut() {
        relationship.label = "hands the batch over now".into();
    }
    for detail in Detail::ALL {
        let svg = GraphLayout::new(&document, &visible, detail).svg(None, &BTreeSet::new());
        let (groups, _) = split(&svg);
        for (id, _) in lines(&svg) {
            let pill = groups
                .iter()
                .find(|group| group.kind == KIND_PILL && group.id == id)
                .unwrap();
            assert!(
                pill.inner.contains(" hands the batch over now</text>"),
                "{detail:?}: {id} lost its label"
            );
        }
    }
}

#[test]
fn large_diagrams_lay_their_lanes_out_near_16_to_10() {
    let fixture = large(500, 1000, 20);
    for lanes in ORIENTATIONS {
        let (document, visible) = as_lanes(&fixture, lanes);
        let bounds = GraphLayout::bounds_for(&document, &visible, Detail::Subtitles);
        let ratio = bounds.width / bounds.height / 1.6;
        assert!(
            ratio.max(1.0 / ratio) <= 2.0,
            "{lanes:?}: {} by {}",
            bounds.width,
            bounds.height
        );
    }
}
