//! Graphic diagram: lanes, boxes, arrows and gates laid out in world units, written as SVG
//! and rasterized for terminals with an image protocol. The same geometry answers clicks.
use crate::model::{Direction, Document, ElementKind, Id, ViewState};
use crate::ui::list;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

pub const BOX_WIDTH: f32 = 220.0;
const BOX_GAP_X: f32 = 40.0;
const BOX_GAP_Y: f32 = 64.0;
const LANE_PADDING: f32 = 24.0;
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
pub const ZOOM_STEP: f32 = 1.5;
pub const MAX_ZOOM: f32 = 8.0;

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

    fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }

    fn right(&self) -> f32 {
        self.x + self.width
    }

    fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

struct Lane {
    title: String,
    color: &'static str,
    rect: WorldRect,
}

struct Gate {
    id: Id,
    label: String,
    rect: WorldRect,
}

struct Node {
    id: Id,
    rect: WorldRect,
    name: String,
    subtitle: String,
    body: Vec<String>,
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
    pub fn new(document: &Document, view: &ViewState, detail: Detail) -> Self {
        let rows = list::rows(document, view);
        let visible: BTreeSet<Id> = rows.iter().map(|row| row.id).collect();
        let elements: Vec<_> = {
            let mut items: Vec<_> = document
                .elements
                .values()
                .filter(|item| visible.contains(&item.id))
                .collect();
            items.sort_by_key(|item| (item.order, item.id));
            items
        };
        let relationships: Vec<_> = document
            .relationships
            .values()
            .filter(|item| {
                visible.contains(&item.id)
                    && visible.contains(&item.from)
                    && visible.contains(&item.to)
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
                label: list::single_line(&element.title),
                rect: WorldRect::default(),
            });
        }

        // Lanes follow groups; ungrouped boxes share one lane at the end.
        let mut lane_members: Vec<(String, Vec<Id>)> = Vec::new();
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
            lane_members.push((list::single_line(&group.title), members));
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
            lane_members.push((title, rest));
        }

        let columns = lane_members
            .iter()
            .map(|(_, members)| members.len())
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
        for (index, (title, members)) in lane_members.into_iter().enumerate() {
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
                    name: list::single_line(&element.title),
                    subtitle: list::single_line(&subtitle(&element.body)),
                    body,
                    color: kind_color(&element.kind),
                    gates: node_gates,
                });
            }
            let lane_height = title_height
                + LANE_PADDING * 2.0
                + (row_y - lane_top - title_height - LANE_PADDING)
                + row_height;
            layout.lanes.push(Lane {
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
            let label = list::single_line(&relationship.label);
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
        }
        for edge in &self.edges {
            let focused = focus == Some(edge.id);
            let stroke = if focused {
                FOCUS
            } else if edge.dashed {
                DIM
            } else {
                FLOW
            };
            let marker = if focused {
                "f"
            } else if edge.dashed {
                "d"
            } else {
                "a"
            };
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
            let _ = write!(
                svg,
                r#"<path d="{path}" fill="none" stroke="{stroke}" stroke-width="{}"{}{start}{end}/>"#,
                if focused { 3.0 } else { 1.8 },
                if edge.dashed {
                    r#" stroke-dasharray="6 5""#
                } else {
                    ""
                }
            );
            let p = edge.pill;
            let _ = write!(
                svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="10" fill="{stroke}"/><text x="{}" y="{}" text-anchor="middle" fill="{PILL_TEXT}" font-size="11" font-weight="700">{}</text>"#,
                p.x,
                p.y,
                p.width,
                p.height,
                p.center_x(),
                p.center_y() + 4.0,
                escape(&edge.label)
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
            for gate in &node.gates {
                let g = gate.rect;
                let focused = focus == Some(gate.id);
                let _ = write!(
                    svg,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" rx="9" fill="{AMBER}" stroke="{}" stroke-width="{}"/><text x="{}" y="{}" fill="{GATE_TEXT}" font-size="11" font-weight="700">! {}</text>"#,
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
        }
        svg.push_str("</svg>");
        svg
    }
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

/// System fonts for the rasterizer; the generic families point at whatever sans and mono
/// faces the host has, so text renders without bundled fonts.
pub fn load_fonts() -> Option<Arc<fontdb::Database>> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    let names: Vec<String> = database
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.clone()))
        .collect();
    let family = |needle: &str| names.iter().find(|name| name.contains(needle)).cloned();
    let sans = family("Liberation Sans")
        .or_else(|| family("DejaVu Sans"))
        .or_else(|| family("Sans"))
        .or_else(|| names.first().cloned())?;
    database.set_sans_serif_family(sans);
    if let Some(mono) = family("Mono") {
        database.set_monospace_family(mono);
    }
    Some(Arc::new(database))
}

/// Rasterizes the world-unit SVG at `scale` with the top-left corner at `pan`.
pub fn rasterize(
    svg: &str,
    width: u32,
    height: u32,
    scale: f32,
    pan: (f32, f32),
    fonts: &Arc<fontdb::Database>,
) -> Option<image::DynamicImage> {
    let options = usvg::Options {
        fontdb: Arc::clone(fonts),
        font_family: "sans-serif".to_owned(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(svg.as_bytes(), &options).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1))?;
    pixmap.fill(tiny_skia::Color::from_rgba8(0x10, 0x14, 0x18, 0xff));
    let transform = tiny_skia::Transform::from_scale(scale, scale)
        .post_translate(-pan.0 * scale, -pan.1 * scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let (width, height) = (pixmap.width(), pixmap.height());
    image::RgbaImage::from_raw(width, height, pixmap.take_demultiplied())
        .map(image::DynamicImage::ImageRgba8)
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

/// Viewport state and image cache for one diagram pane. Zoom 1 fits the whole diagram.
pub struct GraphView {
    picker: ratatui_image::picker::Picker,
    fonts: Arc<fontdb::Database>,
    pub zoom: f32,
    pub pan: (f32, f32),
    cells: (u16, u16),
    scale: f32,
    layout: GraphLayout,
    cache: Option<(CacheKey, ratatui_image::protocol::Protocol)>,
}

#[derive(Clone, PartialEq)]
struct CacheKey {
    revision: u64,
    view: Id,
    detail: Detail,
    scale: u32,
    pan: (i32, i32),
    cells: (u16, u16),
    focus: Option<Id>,
    selected: Vec<Id>,
    nodes: usize,
    edges: usize,
}

/// A tiny diagram is not blown up beyond this many pixels per world unit when fitting.
const MAX_FIT_SCALE: f32 = 1.5;
const WHEEL_ZOOM_STEP: f32 = 1.25;

impl GraphView {
    pub fn new(picker: ratatui_image::picker::Picker, fonts: Arc<fontdb::Database>) -> Self {
        Self {
            picker,
            fonts,
            zoom: 1.0,
            pan: (0.0, 0.0),
            cells: (1, 1),
            scale: 1.0,
            layout: GraphLayout::default(),
            cache: None,
        }
    }

    pub fn protocol_name(&self) -> &'static str {
        use ratatui_image::picker::ProtocolType;
        match self.picker.protocol_type() {
            ProtocolType::Halfblocks => "halfblocks",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Kitty => "kitty",
            ProtocolType::Iterm2 => "iterm2",
        }
    }

    pub fn layout(&self) -> &GraphLayout {
        &self.layout
    }

    pub fn wheel_zoom_step(&self) -> f32 {
        WHEEL_ZOOM_STEP
    }

    fn pixels(&self) -> (f32, f32) {
        let font = self.picker.font_size();
        (
            f32::from(self.cells.0) * f32::from(font.width),
            f32::from(self.cells.1) * f32::from(font.height),
        )
    }

    fn fit_scale(&self, bounds: WorldRect) -> f32 {
        let (width, height) = self.pixels();
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return 1.0;
        }
        (width / bounds.width)
            .min(height / bounds.height)
            .min(MAX_FIT_SCALE)
    }

    /// Lays the diagram out for the current pane size and zoom: fitting uses the richest
    /// detail that still fits, zooming in adds detail as boxes grow.
    pub fn prepare(&mut self, document: &Document, view: &ViewState, cells: (u16, u16)) {
        self.cells = (cells.0.max(1), cells.1.max(1));
        let mut chosen = GraphLayout::new(document, view, Detail::Names);
        let mut fit = self.fit_scale(chosen.bounds);
        for detail in [Detail::Full, Detail::Subtitles] {
            let candidate = GraphLayout::new(document, view, detail);
            let scale = self.fit_scale(candidate.bounds);
            if Detail::for_scale(scale) >= detail {
                chosen = candidate;
                fit = scale;
                break;
            }
        }
        let scale = fit * self.zoom;
        let detail = Detail::for_scale(scale);
        if detail != chosen.detail {
            chosen = GraphLayout::new(document, view, detail);
        }
        self.scale = scale;
        self.layout = chosen;
        self.clamp_pan();
    }

    fn clamp_pan(&mut self) {
        let (width, height) = self.pixels();
        let view = (width / self.scale, height / self.scale);
        let bounds = self.layout.bounds;
        self.pan.0 = if bounds.width <= view.0 {
            (bounds.width - view.0) / 2.0
        } else {
            self.pan.0.clamp(0.0, bounds.width - view.0)
        };
        self.pan.1 = if bounds.height <= view.1 {
            (bounds.height - view.1) / 2.0
        } else {
            self.pan.1.clamp(0.0, bounds.height - view.1)
        };
    }

    /// Zooms around `anchor` (the focused shape or the viewport center) so it stays put.
    pub fn zoom_by(&mut self, factor: f32, anchor: Option<WorldRect>) {
        let previous = self.zoom;
        self.zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
        let ratio = previous / self.zoom;
        let (width, height) = self.pixels();
        let view = (width / self.scale, height / self.scale);
        let (ax, ay) = anchor.map_or(
            (self.pan.0 + view.0 / 2.0, self.pan.1 + view.1 / 2.0),
            |rect| (rect.center_x(), rect.center_y()),
        );
        self.pan = (
            ax - (ax - self.pan.0) * ratio,
            ay - (ay - self.pan.1) * ratio,
        );
    }

    pub fn reset_zoom(&mut self) {
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
    }

    pub fn pan_by_cells(&mut self, dx: i32, dy: i32) {
        let font = self.picker.font_size();
        self.pan.0 += dx as f32 * f32::from(font.width) / self.scale;
        self.pan.1 += dy as f32 * f32::from(font.height) / self.scale;
        self.clamp_pan();
    }

    /// Pans the least amount that brings the shape fully into view.
    pub fn reveal(&mut self, rect: WorldRect) {
        let (width, height) = self.pixels();
        let view = (width / self.scale, height / self.scale);
        if rect.x < self.pan.0 {
            self.pan.0 = rect.x - LANE_PADDING;
        } else if rect.right() > self.pan.0 + view.0 {
            self.pan.0 = rect.right() - view.0 + LANE_PADDING;
        }
        if rect.y < self.pan.1 {
            self.pan.1 = rect.y - LANE_PADDING;
        } else if rect.bottom() > self.pan.1 + view.1 {
            self.pan.1 = rect.bottom() - view.1 + LANE_PADDING;
        }
        self.clamp_pan();
    }

    /// Maps a cell inside `inner` to world units through the cell center.
    pub fn hit(&self, inner: ratatui::layout::Rect, column: u16, row: u16) -> Option<Id> {
        if !inner.contains(ratatui::layout::Position::new(column, row)) {
            return None;
        }
        let font = self.picker.font_size();
        let x =
            (f32::from(column - inner.x) + 0.5) * f32::from(font.width) / self.scale + self.pan.0;
        let y = (f32::from(row - inner.y) + 0.5) * f32::from(font.height) / self.scale + self.pan.1;
        self.layout.hit(x, y)
    }

    pub fn render(
        &mut self,
        frame: &mut ratatui::Frame,
        inner: ratatui::layout::Rect,
        document: &Document,
        view: &ViewState,
        focus: Option<Id>,
        selected: &BTreeSet<Id>,
    ) {
        if inner.is_empty() {
            return;
        }
        let key = CacheKey {
            revision: document.revision,
            view: view.id,
            detail: self.layout.detail,
            scale: self.scale.to_bits(),
            pan: (
                (self.pan.0 * 4.0).round() as i32,
                (self.pan.1 * 4.0).round() as i32,
            ),
            cells: (inner.width, inner.height),
            focus,
            selected: selected.iter().copied().collect(),
            nodes: self.layout.nodes.len(),
            edges: self.layout.edges.len(),
        };
        if self.cache.as_ref().is_none_or(|(cached, _)| *cached != key) {
            let (width, height) = self.pixels();
            let svg = self.layout.svg(focus, selected);
            let Some(image) = rasterize(
                &svg,
                width as u32,
                height as u32,
                self.scale,
                self.pan,
                &self.fonts,
            ) else {
                return;
            };
            let Ok(protocol) = self.picker.new_protocol(
                image,
                ratatui::layout::Size::new(inner.width, inner.height),
                ratatui_image::Resize::Fit(None),
            ) else {
                return;
            };
            self.cache = Some((key, protocol));
        }
        if let Some((_, protocol)) = &self.cache {
            frame.render_widget(ratatui_image::Image::new(protocol), inner);
        }
    }

    /// Writes the whole diagram with full text as an SVG file the browser can open.
    pub fn export_svg(
        &self,
        document: &Document,
        view: &ViewState,
        directory: &std::path::Path,
    ) -> std::io::Result<std::path::PathBuf> {
        let layout = GraphLayout::new(document, view, Detail::Full);
        let slug: String = document
            .title
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(48)
            .collect();
        let slug = if slug.is_empty() {
            "diagram".to_owned()
        } else {
            slug
        };
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("{slug}.svg"));
        std::fs::write(&path, layout.svg(None, &BTreeSet::new()))?;
        Ok(path)
    }
}
