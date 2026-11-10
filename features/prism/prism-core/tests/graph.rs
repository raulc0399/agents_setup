use prism_core::graph::{
    ARROW_COLORS, Detail, GraphLayout, KIND_ARROW, KIND_BOX, KIND_GATE, KIND_LANE, KIND_PILL,
    WorldRect,
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
    assert!(svg.contains(r#"aria-label="Arrow 1 hands over: Collect to Store""#));
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
    assert!(pill.contains("<title>1 hands over</title>") && pill.contains(">1</text>"));
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
    assert!(svg.contains(r#"aria-label="Arrow 1 long label for the arrow: Step 1 to Step 2""#));
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

/// The polyline of every arrow, from the `d` of its visible line.
fn lines(svg: &str) -> Vec<(Id, Vec<(f32, f32)>)> {
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
            let path = line.split('"').next().unwrap();
            let points = path
                .split(['M', 'L'])
                .map(str::trim)
                .filter(|point| !point.is_empty())
                .map(|point| {
                    let (x, y) = point.split_once(',').unwrap();
                    (number(x), number(y))
                })
                .collect();
            (group.id, points)
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

fn segments(points: &[(f32, f32)]) -> Vec<Segment> {
    points
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .filter(|(a, b)| a != b)
        .collect()
}

#[test]
fn no_two_arrows_share_a_line() {
    let (document, visible) = three_lanes();
    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    let arrows = lines(&svg);
    assert_eq!(arrows.len(), 5);
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
    let (document, visible) = three_lanes();
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let svg = layout.svg(None, &BTreeSet::new());
    let lanes = lane_rects(&svg);
    assert_eq!(lanes.len(), 3);
    let badges: Vec<WorldRect> = [40, 41]
        .into_iter()
        .map(|id| layout.rect_of(Id::from_u128(id)).unwrap())
        .collect();
    for (id, points) in lines(&svg) {
        for (a, b) in segments(&points) {
            let line = WorldRect {
                x: a.0.min(b.0),
                y: a.1.min(b.1),
                width: (a.0 - b.0).abs(),
                height: (a.1 - b.1).abs(),
            };
            for badge in &badges {
                assert!(!overlaps(&line, badge), "{id} crosses a badge");
            }
            if a.1 != b.1 {
                continue;
            }
            let y = a.1;
            let inside = lanes
                .iter()
                .any(|lane| y > lane.y + 22.0 && y < lane.bottom() - 2.0);
            let in_gap = lanes.windows(2).any(|pair| {
                let middle = (pair[0].bottom() + pair[1].y) / 2.0;
                y > pair[0].bottom() + 2.0 && y < pair[1].y - 2.0 && (y - middle).abs() <= 12.0
            });
            assert!(
                inside || in_gap,
                "{id} runs at {y}, on a lane edge or title"
            );
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

#[test]
fn arrows_of_one_lane_pair_take_the_colors_in_turn() {
    // Eight arrows inside the one lane of ungrouped boxes, in ID order.
    let (mut document, mut visible) = steps(9);
    let ids: Vec<u128> = (70..78).collect();
    for (offset, id) in ids.iter().enumerate() {
        let from = offset as u128 + 1;
        let (key, relationship) = link(*id, from, from + 1, "next", offset == 1);
        document.relationships.insert(key, relationship);
        visible.insert(key);
    }
    let layout = GraphLayout::new(&document, &visible, Detail::Subtitles);
    let svg = layout.svg(None, &BTreeSet::new());
    let colors = arrow_colors(&svg);
    let expected: Vec<(Id, usize)> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (Id::from_u128(*id), index % ARROW_COLORS.len()))
        .collect();
    assert_eq!(
        colors, expected,
        "distinct up to six, then the cycle repeats"
    );
    assert_eq!(
        colors.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        layout.ids()[9..],
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
    let manual = Id::from_u128(ids[1]);
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
            .contains(r#"class="prism-arrow prism-manual prism-arrow-c1""#)
    );
    assert!(
        pill.open
            .contains(r#"class="prism-pill prism-manual prism-arrow-c1""#)
    );
    let color = ARROW_COLORS[1];
    assert!(arrow.inner.contains(&format!(r#"stroke="{color}""#)));
    assert!(arrow.inner.contains(r#"stroke-dasharray="6 5""#));
    assert!(arrow.inner.contains(r#"marker-end="url(#prism-head-c1)""#));
    assert!(svg.contains(&format!(
        r#"<marker id="prism-head-c1" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{color}"/>"#
    )));
    assert!(pill.inner.contains(&format!(r#"fill="{color}""#)));
}

#[test]
fn each_lane_pair_counts_its_own_arrows_in_either_direction() {
    let (mut document, mut visible) = three_lanes();
    // Device to Staging, and back: one pair.
    let (key, relationship) = link(65, 4, 1, "back", false);
    document.relationships.insert(key, relationship);
    visible.insert(key);
    let svg = GraphLayout::new(&document, &visible, Detail::Subtitles).svg(None, &BTreeSet::new());
    let colors: BTreeMap<u128, usize> = arrow_colors(&svg)
        .into_iter()
        .map(|(id, color)| (id.as_u128(), color))
        .collect();
    // Device: 60; Device-Staging: 61, 65; Staging: 62; Staging-Live: 63, 64.
    assert_eq!(
        colors,
        BTreeMap::from([(60, 0), (61, 0), (62, 0), (63, 0), (64, 1), (65, 1)])
    );
}
