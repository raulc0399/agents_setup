//! Graphic diagram: lanes, boxes, arrows and gates laid out in world units and written as
//! SVG. The same geometry answers clicks; every clickable shape in the SVG carries `data-id`
//! and `data-kind` (one of the `KIND_*` values).
use crate::model::{Direction, Document, ElementKind, Id, single_line};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub const BOX_WIDTH: f32 = 220.0;
const BOX_GAP_X: f32 = 40.0;
const BOX_GAP_Y: f32 = 64.0;
pub const LANE_PADDING: f32 = 24.0;
const LANE_TITLE_HEIGHT: f32 = 22.0;
const LANE_GAP: f32 = 24.0;
const NAME_HEIGHT: f32 = 32.0;
const SUBTITLE_HEIGHT: f32 = 52.0;
const BODY_LINE_HEIGHT: f32 = 15.0;
const BODY_LINE_CHARS: usize = 34;
const BODY_MAX_LINES: usize = 8;
const SUBTITLE_WORDS: usize = 5;
const MAX_COLUMNS: usize = 4;
const PILL_HEIGHT: f32 = 20.0;
const PILL_CHAR_WIDTH: f32 = 6.6;
const GATE_HEIGHT: f32 = 18.0;
const ARROW_HIT_DISTANCE: f32 = 8.0;
/// Pixels per world unit below which boxes show names only, and above which full text.
const SUBTITLE_SCALE: f32 = 0.45;
const FULL_SCALE: f32 = 1.1;

/// `data-kind` values of the clickable shapes; `data-id` is the content ID behind each.
pub const KIND_LANE: &str = "lane";
pub const KIND_BOX: &str = "box";
pub const KIND_GATE: &str = "gate";
pub const KIND_ARROW: &str = "arrow";
pub const KIND_PILL: &str = "pill";
/// Class modifier on the arrow and pill of a manual (dashed) step.
const MODIFIER_MANUAL: &str = "manual";

const BACKGROUND: &str = "#101418";
const BOX_FILL: &str = "#1a2027";
const TEXT: &str = "#d8dde3";
const DIM: &str = "#8b93a1";
const FOCUS: &str = "#4cc9f0";
const AMBER: &str = "#f59e0b";
const LANE_COLORS: [&str; 2] = ["#2dd4bf", "#6e9fff"];
const FLOW: &str = "#6e9fff";
const PILL_TEXT: &str = "#0d1117";
const GATE_TEXT: &str = "#1b1b1b";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Detail {
    #[default]
    Names,
    Subtitles,
    Full,
}

impl Detail {
    pub fn for_scale(scale: f32) -> Self {
        if scale >= FULL_SCALE {
            Detail::Full
        } else if scale >= SUBTITLE_SCALE {
            Detail::Subtitles
        } else {
            Detail::Names
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WorldRect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }

    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

struct Lane {
    /// The group behind the lane; the lane of ungrouped boxes has none.
    group: Option<Id>,
    title: String,
    color: &'static str,
    rect: WorldRect,
}

struct Gate {
    id: Id,
    kind: ElementKind,
    label: String,
    rect: WorldRect,
}

struct Node {
    id: Id,
    rect: WorldRect,
    name: String,
    subtitle: String,
    body: Vec<String>,
    kind: ElementKind,
    color: &'static str,
    gates: Vec<Gate>,
}

struct Edge {
    id: Id,
    points: Vec<(f32, f32)>,
    label: String,
    dashed: bool,
    direction: Direction,
    pill: WorldRect,
}

#[derive(Default)]
pub struct GraphLayout {
    lanes: Vec<Lane>,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    pub bounds: WorldRect,
    pub detail: Detail,
}

fn kind_color(kind: &ElementKind) -> &'static str {
    match kind {
        ElementKind::Text => TEXT,
        ElementKind::Step => FOCUS,
        ElementKind::Finding => "#e5484d",
        ElementKind::Question => "#d783ff",
    }
}

fn subtitle(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .split_whitespace()
        .take(SUBTITLE_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

fn wrap_words(text: &str, chars: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > chars {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                let last = lines.last_mut().expect("one line pushed");
                last.push('…');
                return lines;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn box_height(detail: Detail, body_lines: usize) -> f32 {
    match detail {
        Detail::Names => NAME_HEIGHT,
        Detail::Subtitles => SUBTITLE_HEIGHT,
        Detail::Full => NAME_HEIGHT + body_lines as f32 * BODY_LINE_HEIGHT + 14.0,
    }
}

impl GraphLayout {
    /// Lays out the content whose IDs are in `visible`: elements, relationships and groups.
    /// Archived content is always left out; the caller leaves out hidden and filtered-out
    /// content. An arrow is drawn only when both of its ends are.
    pub fn new(document: &Document, visible: &BTreeSet<Id>, detail: Detail) -> Self {
        let elements: Vec<_> = {
            let mut items: Vec<_> = document
                .elements
                .values()
                .filter(|item| !item.archived && visible.contains(&item.id))
                .collect();
            items.sort_by_key(|item| (item.order, item.id));
            items
        };
        let shown: BTreeSet<Id> = elements.iter().map(|item| item.id).collect();
        let relationships: Vec<_> = document
            .relationships
            .values()
            .filter(|item| {
                !item.archived
                    && visible.contains(&item.id)
                    && shown.contains(&item.from)
                    && shown.contains(&item.to)
            })
            .collect();

        // A finding or question attached to exactly one other box becomes a gate badge on it.
        let mut gates: BTreeMap<Id, Vec<Gate>> = BTreeMap::new();
        let mut gate_ids = BTreeSet::new();
        let mut consumed_edges = BTreeSet::new();
        for element in &elements {
            if !matches!(element.kind, ElementKind::Finding | ElementKind::Question) {
                continue;
            }
            let touching: Vec<_> = relationships
                .iter()
                .filter(|item| item.from == element.id || item.to == element.id)
                .collect();
            if touching.len() != 1 {
                continue;
            }
            let relationship = touching[0];
            let other = if relationship.from == element.id {
                relationship.to
            } else {
                relationship.from
            };
            if other == element.id
                || !document.elements.get(&other).is_some_and(|target| {
                    !matches!(target.kind, ElementKind::Finding | ElementKind::Question)
                })
            {
                continue;
            }
            gate_ids.insert(element.id);
            consumed_edges.insert(relationship.id);
            gates.entry(other).or_default().push(Gate {
                id: element.id,
                kind: element.kind.clone(),
                label: single_line(&element.title),
                rect: WorldRect::default(),
            });
        }

        // Lanes follow groups; ungrouped boxes share one lane at the end.
        let mut lane_members: Vec<(Option<Id>, String, Vec<Id>)> = Vec::new();
        let mut placed = BTreeSet::new();
        let mut groups: Vec<_> = document
            .groups
            .values()
            .filter(|group| !group.archived && visible.contains(&group.id))
            .collect();
        groups.sort_by_key(|group| {
            group
                .members
                .iter()
                .filter_map(|id| document.elements.get(id).map(|item| item.order))
                .min()
                .unwrap_or(u64::MAX)
        });
        for group in groups {
            let members: Vec<Id> = elements
                .iter()
                .filter(|item| {
                    group.members.contains(&item.id)
                        && !gate_ids.contains(&item.id)
                        && !placed.contains(&item.id)
                })
                .map(|item| item.id)
                .collect();
            if members.is_empty() {
                continue;
            }
            placed.extend(members.iter().copied());
            lane_members.push((Some(group.id), single_line(&group.title), members));
        }
        let rest: Vec<Id> = elements
            .iter()
            .filter(|item| !gate_ids.contains(&item.id) && !placed.contains(&item.id))
            .map(|item| item.id)
            .collect();
        if !rest.is_empty() {
            let title = if lane_members.is_empty() {
                String::new()
            } else {
                "Other".to_owned()
            };
            lane_members.push((None, title, rest));
        }

        let columns = lane_members
            .iter()
            .map(|(_, _, members)| members.len())
            .max()
            .unwrap_or(1)
            .max(1);
        let columns = ((columns as f32).sqrt().ceil() as usize).clamp(1, MAX_COLUMNS);
        let lane_width =
            LANE_PADDING * 2.0 + columns as f32 * BOX_WIDTH + (columns - 1) as f32 * BOX_GAP_X;
        let mut layout = GraphLayout {
            detail,
            ..Default::default()
        };
        let mut y = 0.0;
        for (index, (group, title, members)) in lane_members.into_iter().enumerate() {
            let color = LANE_COLORS[index % LANE_COLORS.len()];
            let title_height = if title.is_empty() {
                0.0
            } else {
                LANE_TITLE_HEIGHT
            };
            let lane_top = y;
            let mut row_y = lane_top + title_height + LANE_PADDING;
            let mut row_height: f32 = 0.0;
            for (position, id) in members.iter().enumerate() {
                let element = &document.elements[id];
                let body = if detail == Detail::Full {
                    wrap_words(&element.body, BODY_LINE_CHARS, BODY_MAX_LINES)
                } else {
                    Vec::new()
                };
                let height = box_height(detail, body.len());
                let column = position % columns;
                if column == 0 && position > 0 {
                    row_y += row_height + BOX_GAP_Y;
                    row_height = 0.0;
                }
                row_height = row_height.max(height);
                let rect = WorldRect {
                    x: LANE_PADDING + column as f32 * (BOX_WIDTH + BOX_GAP_X),
                    y: row_y,
                    width: BOX_WIDTH,
                    height,
                };
                let mut node_gates = gates.remove(id).unwrap_or_default();
                let mut gate_x = rect.right();
                for gate in &mut node_gates {
                    let width = gate.label.chars().count() as f32 * PILL_CHAR_WIDTH + 26.0;
                    gate_x -= width - 6.0;
                    gate.rect = WorldRect {
                        x: gate_x,
                        y: rect.y - GATE_HEIGHT / 2.0,
                        width,
                        height: GATE_HEIGHT,
                    };
                    gate_x -= 6.0;
                }
                layout.nodes.push(Node {
                    id: *id,
                    rect,
                    name: single_line(&element.title),
                    subtitle: single_line(&subtitle(&element.body)),
                    body,
                    kind: element.kind.clone(),
                    color: kind_color(&element.kind),
                    gates: node_gates,
                });
            }
            let lane_height = title_height
                + LANE_PADDING * 2.0
                + (row_y - lane_top - title_height - LANE_PADDING)
                + row_height;
            layout.lanes.push(Lane {
                group,
                title,
                color,
                rect: WorldRect {
                    x: 0.0,
                    y: lane_top,
                    width: lane_width,
                    height: lane_height,
                },
            });
            y = lane_top + lane_height + LANE_GAP;
        }

        let rects: BTreeMap<Id, WorldRect> = layout
            .nodes
            .iter()
            .map(|node| (node.id, node.rect))
            .collect();
        let mut sorted: Vec<_> = relationships
            .iter()
            .filter(|item| !consumed_edges.contains(&item.id))
            .collect();
        sorted.sort_by_key(|item| (item.semantic_revision, item.id));
        for (index, relationship) in sorted.into_iter().enumerate() {
            let (Some(from), Some(to)) =
                (rects.get(&relationship.from), rects.get(&relationship.to))
            else {
                continue;
            };
            let obstacles: Vec<WorldRect> = rects.values().copied().collect();
            let points = route(from, to, lane_width, &obstacles);
            let label = single_line(&relationship.label);
            let label = if label.starts_with(|character: char| character.is_ascii_digit()) {
                label
            } else {
                format!("{} {label}", index + 1)
            };
            let (x1, y1, x2, y2) = longest_segment(&points);
            let pill_width = label.chars().count() as f32 * PILL_CHAR_WIDTH + 16.0;
            layout.edges.push(Edge {
                id: relationship.id,
                points,
                label,
                dashed: relationship.manual,
                direction: relationship.direction.clone(),
                pill: WorldRect {
                    x: (x1 + x2) / 2.0 - pill_width / 2.0,
                    y: (y1 + y2) / 2.0 - PILL_HEIGHT / 2.0,
                    width: pill_width,
                    height: PILL_HEIGHT,
                },
            });
        }
        let height = layout.lanes.last().map_or(0.0, |lane| lane.rect.bottom());
        layout.bounds = WorldRect {
            x: 0.0,
            y: 0.0,
            width: if layout.lanes.is_empty() {
                0.0
            } else {
                lane_width
            },
            height,
        };
        layout
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Focus order: boxes in lane order, each followed by its gates, then arrows.
    pub fn ids(&self) -> Vec<Id> {
        let mut ids = Vec::new();
        for node in &self.nodes {
            ids.push(node.id);
            ids.extend(node.gates.iter().map(|gate| gate.id));
        }
        ids.extend(self.edges.iter().map(|edge| edge.id));
        ids
    }

    pub fn rect_of(&self, id: Id) -> Option<WorldRect> {
        for node in &self.nodes {
            if node.id == id {
                return Some(node.rect);
            }
            if let Some(gate) = node.gates.iter().find(|gate| gate.id == id) {
                return Some(gate.rect);
            }
        }
        self.edges
            .iter()
            .find(|edge| edge.id == id)
            .map(|edge| edge.pill)
    }

    /// Gates and pills sit on top of boxes and arrows, so they win.
    pub fn hit(&self, x: f32, y: f32) -> Option<Id> {
        for node in &self.nodes {
            if let Some(gate) = node.gates.iter().find(|gate| gate.rect.contains(x, y)) {
                return Some(gate.id);
            }
        }
        if let Some(edge) = self.edges.iter().find(|edge| edge.pill.contains(x, y)) {
            return Some(edge.id);
        }
        if let Some(node) = self.nodes.iter().find(|node| node.rect.contains(x, y)) {
            return Some(node.id);
        }
        self.edges
            .iter()
            .find(|edge| {
                edge.points.windows(2).any(|segment| {
                    segment_distance(segment[0], segment[1], (x, y)) <= ARROW_HIT_DISTANCE
                })
            })
            .map(|edge| edge.id)
    }

    pub fn svg(&self, focus: Option<Id>, selected: &BTreeSet<Id>) -> String {
        let mut svg = String::new();
        let width = self.bounds.width.max(1.0);
        let height = self.bounds.height.max(1.0);
        let _ = write!(
            svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" font-family="sans-serif">"#
        );
        let _ = write!(
            svg,
            r#"<defs><marker id="a" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{FLOW}"/></marker><marker id="d" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{DIM}"/></marker><marker id="f" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{FOCUS}"/></marker></defs>"#
        );
        let _ = write!(
            svg,
            r#"<rect x="0" y="0" width="{width}" height="{height}" fill="{BACKGROUND}"/>"#
        );
        for lane in &self.lanes {
            let r = lane.rect;
            if let Some(group) = lane.group {
                open_clickable(&mut svg, group, KIND_LANE, None);
            }
            let _ = write!(
                svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="4" fill="{}" fill-opacity="0.06" stroke="{}" stroke-opacity="0.35"/>"#,
                r.x, r.y, r.width, r.height, lane.color, lane.color
            );
            if !lane.title.is_empty() {
                let _ = write!(
                    svg,
                    r#"<text x="{}" y="{}" fill="{}" font-size="11" font-weight="700" letter-spacing="1">{}</text>"#,
                    r.x + 10.0,
                    r.y + 16.0,
                    lane.color,
                    escape(&lane.title.to_uppercase())
                );
            }
            if lane.group.is_some() {
                svg.push_str("</g>");
            }
        }
        // Arrows first, boxes over them, then pills and gates on top: the order `hit` checks.
        for edge in &self.edges {
            let focused = focus == Some(edge.id);
            let (stroke, marker) = edge_paint(edge, focused);
            let mut path = String::new();
            for (index, (x, y)) in edge.points.iter().enumerate() {
                let _ = write!(path, "{}{x},{y}", if index == 0 { "M" } else { " L" });
            }
            let (start, end) = match edge.direction {
                Direction::Directed => ("", format!(r#" marker-end="url(#{marker})""#)),
                Direction::Bidirectional => (
                    r#" marker-start="url(#MARKER)""#,
                    format!(r#" marker-end="url(#{marker})""#),
                ),
                Direction::Undirected => ("", String::new()),
            };
            let start = start.replace("MARKER", marker);
            open_clickable(&mut svg, edge.id, KIND_ARROW, edge_modifier(edge));
            // An invisible wide stroke makes the thin arrow easy to hit with a pointer.
            let _ = write!(
                svg,
                r#"<path d="{path}" fill="none" stroke="{BACKGROUND}" stroke-opacity="0" stroke-width="{}" pointer-events="stroke"/>"#,
                ARROW_HIT_DISTANCE * 2.0
            );
            let _ = write!(
                svg,
                r#"<path d="{path}" fill="none" stroke="{stroke}" stroke-width="{}"{}{start}{end}/></g>"#,
                if focused { 3.0 } else { 1.8 },
                if edge.dashed {
                    r#" stroke-dasharray="6 5""#
                } else {
                    ""
                }
            );
        }
        for node in &self.nodes {
            let r = node.rect;
            let focused = focus == Some(node.id);
            let stroke = if focused { FOCUS } else { node.color };
            let fill = if selected.contains(&node.id) {
                "#22303a"
            } else {
                BOX_FILL
            };
            open_clickable(&mut svg, node.id, KIND_BOX, Some(kind_name(&node.kind)));
            let _ = write!(
                svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="6" fill="{fill}" stroke="{stroke}" stroke-width="{}"/><rect x="{}" y="{}" width="5" height="{}" rx="2" fill="{}"/>"#,
                r.x,
                r.y,
                r.width,
                r.height,
                if focused { 3.0 } else { 1.5 },
                r.x,
                r.y,
                r.height,
                node.color
            );
            let _ = write!(
                svg,
                r#"<text x="{}" y="{}" fill="{TEXT}" font-weight="700" font-size="14">{}</text>"#,
                r.x + 14.0,
                r.y + 21.0,
                escape(&clip(&node.name, 28))
            );
            if self.detail == Detail::Subtitles && !node.subtitle.is_empty() {
                let _ = write!(
                    svg,
                    r#"<text x="{}" y="{}" fill="{DIM}" font-size="12">{}</text>"#,
                    r.x + 14.0,
                    r.y + 39.0,
                    escape(&clip(&node.subtitle, 32))
                );
            }
            for (index, line) in node.body.iter().enumerate() {
                let _ = write!(
                    svg,
                    r#"<text x="{}" y="{}" fill="{TEXT}" font-size="12">{}</text>"#,
                    r.x + 14.0,
                    r.y + NAME_HEIGHT + 10.0 + index as f32 * BODY_LINE_HEIGHT,
                    escape(line)
                );
            }
            svg.push_str("</g>");
        }
        for edge in &self.edges {
            let (stroke, _) = edge_paint(edge, focus == Some(edge.id));
            let p = edge.pill;
            open_clickable(&mut svg, edge.id, KIND_PILL, edge_modifier(edge));
            let _ = write!(
                svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="10" fill="{stroke}"/><text x="{}" y="{}" text-anchor="middle" fill="{PILL_TEXT}" font-size="11" font-weight="700">{}</text></g>"#,
                p.x,
                p.y,
                p.width,
                p.height,
                p.center_x(),
                p.center_y() + 4.0,
                escape(&edge.label)
            );
        }
        for gate in self.nodes.iter().flat_map(|node| &node.gates) {
            let g = gate.rect;
            let focused = focus == Some(gate.id);
            open_clickable(&mut svg, gate.id, KIND_GATE, Some(kind_name(&gate.kind)));
            let _ = write!(
                svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="9" fill="{AMBER}" stroke="{}" stroke-width="{}"/><text x="{}" y="{}" fill="{GATE_TEXT}" font-size="11" font-weight="700">! {}</text></g>"#,
                g.x,
                g.y,
                g.width,
                g.height,
                if focused { FOCUS } else { AMBER },
                if focused { 2.5 } else { 0.0 },
                g.x + 10.0,
                g.y + 13.0,
                escape(&gate.label)
            );
        }
        svg.push_str("</svg>");
        svg
    }
}

/// Arrow and pill color, and the arrowhead marker to match.
fn edge_paint(edge: &Edge, focused: bool) -> (&'static str, &'static str) {
    if focused {
        (FOCUS, "f")
    } else if edge.dashed {
        (DIM, "d")
    } else {
        (FLOW, "a")
    }
}

fn edge_modifier(edge: &Edge) -> Option<&'static str> {
    edge.dashed.then_some(MODIFIER_MANUAL)
}

fn kind_name(kind: &ElementKind) -> &'static str {
    match kind {
        ElementKind::Text => "text",
        ElementKind::Step => "step",
        ElementKind::Finding => "finding",
        ElementKind::Question => "question",
    }
}

/// Opens the focusable `<g>` around one clickable shape; the caller closes it. CSS themes
/// the shape through its classes: `prism-<kind>`, plus `prism-<modifier>` when given.
fn open_clickable(svg: &mut String, id: Id, kind: &str, modifier: Option<&str>) {
    let modifier = modifier.map_or(String::new(), |name| format!(" prism-{name}"));
    let _ = write!(
        svg,
        r#"<g class="prism-{kind}{modifier}" data-id="{id}" data-kind="{kind}" tabindex="0" role="button">"#
    );
}

fn clip(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(chars.saturating_sub(1)).collect();
    clipped.push('…');
    clipped
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Orthogonal route: vertical when the target is in another row, horizontal when beside.
/// A vertical run that would cross another box detours through the gap beside the column.
fn route(
    from: &WorldRect,
    to: &WorldRect,
    lane_width: f32,
    obstacles: &[WorldRect],
) -> Vec<(f32, f32)> {
    let vertical = |start: &WorldRect, end: &WorldRect, downwards: bool| {
        let (leave, enter) = if downwards {
            (start.bottom(), end.y)
        } else {
            (start.y, end.bottom())
        };
        let (top, bottom) = if downwards {
            (leave, enter)
        } else {
            (enter, leave)
        };
        let blocked = |x: f32| {
            obstacles.iter().any(|other| {
                other != start
                    && other != end
                    && other.x < x
                    && x < other.right()
                    && other.bottom() > top
                    && other.y < bottom
            })
        };
        let step = if downwards {
            BOX_GAP_Y / 4.0
        } else {
            -BOX_GAP_Y / 4.0
        };
        if !blocked(start.center_x()) && !blocked(end.center_x()) {
            let mid = (leave + enter) / 2.0;
            return vec![
                (start.center_x(), leave),
                (start.center_x(), mid),
                (end.center_x(), mid),
                (end.center_x(), enter),
            ];
        }
        let channel = if start.right() + BOX_GAP_X < lane_width {
            start.right() + BOX_GAP_X / 2.0
        } else {
            start.x - BOX_GAP_X / 2.0
        };
        vec![
            (start.center_x(), leave),
            (start.center_x(), leave + step),
            (channel, leave + step),
            (channel, enter - step),
            (end.center_x(), enter - step),
            (end.center_x(), enter),
        ]
    };
    if to.y >= from.bottom() {
        return vertical(from, to, true);
    }
    if to.bottom() <= from.y {
        return vertical(from, to, false);
    }
    if to.x >= from.right() {
        return vec![(from.right(), from.center_y()), (to.x, to.center_y())];
    }
    vec![(from.x, from.center_y()), (to.right(), to.center_y())]
}

fn longest_segment(points: &[(f32, f32)]) -> (f32, f32, f32, f32) {
    points
        .windows(2)
        .map(|segment| (segment[0].0, segment[0].1, segment[1].0, segment[1].1))
        .max_by(|a, b| {
            let la = (a.2 - a.0).abs() + (a.3 - a.1).abs();
            let lb = (b.2 - b.0).abs() + (b.3 - b.1).abs();
            la.total_cmp(&lb)
        })
        .unwrap_or((0.0, 0.0, 0.0, 0.0))
}

fn segment_distance(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_limits_lines_and_marks_truncation() {
        let lines = wrap_words("one two three four five six seven eight", 9, 2);
        assert_eq!(lines, vec!["one two".to_owned(), "three…".to_owned()]);
    }

    #[test]
    fn segment_distance_measures_to_the_closest_point() {
        assert_eq!(segment_distance((0.0, 0.0), (10.0, 0.0), (5.0, 3.0)), 3.0);
        assert_eq!(segment_distance((0.0, 0.0), (10.0, 0.0), (14.0, 0.0)), 4.0);
    }
}
