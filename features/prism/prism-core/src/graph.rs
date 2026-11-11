//! Graphic diagram: lanes, boxes, arrows and gates laid out in world units and written as
//! SVG. The same geometry answers clicks; every clickable shape in the SVG carries `data-id`
//! and `data-kind` (one of the `KIND_*` values). Shapes and texts carry `prism-*` classes, so
//! a stylesheet can theme what the presentation attributes paint by default. Archived
//! (dropped) content keeps its place and number: it is drawn faded, with `prism-dropped`.
use crate::model::{Direction, Document, ElementKind, Id, Relationship, single_line};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Box width with subtitles; the overview's boxes are narrower, those with the full text wider.
pub const BOX_WIDTH: f32 = 300.0;
const NAMES_BOX_WIDTH: f32 = 240.0;
const FULL_BOX_WIDTH: f32 = 360.0;
const BOX_GAP_X: f32 = 48.0;
const BOX_GAP_Y: f32 = 64.0;
pub const LANE_PADDING: f32 = 24.0;
const LANE_TITLE_HEIGHT: f32 = 22.0;
/// Room between lanes; an arrow from one lane to another turns in its middle.
const LANE_GAP: f32 = 48.0;
/// Width per character of a lane title (upper case, letter-spaced), and the room added
/// around it, for keeping pills and lines off it.
const LANE_TITLE_CHAR_WIDTH: f32 = 8.5;
const LANE_TITLE_PADDING: f32 = 20.0;
/// A box reads like a card of the app's list (`.card` in its `ui/styles.css`): the same
/// padding, kind stripe, number column and font sizes, in world units, so that at 100% zoom
/// both match. Line heights are the font size times 1.45, as there.
const BOX_PADDING_Y: f32 = 8.0;
const BOX_PADDING_RIGHT: f32 = 12.0;
const BOX_RADIUS: f32 = 8.0;
const STRIPE_X: f32 = 5.0;
const STRIPE_Y: f32 = 7.0;
const STRIPE_WIDTH: f32 = 3.0;
/// The number is right-aligned in its column; kind, title and text start after it.
const NUMBER_RIGHT: f32 = 33.0;
const TEXT_X: f32 = 42.0;
const NUMBER_SIZE: f32 = 13.0;
const KIND_SIZE: f32 = 11.0;
/// 0.04em, as the card's kind label.
const KIND_LETTER_SPACING: f32 = 0.44;
const TITLE_SIZE: f32 = 16.0;
const TEXT_SIZE: f32 = 15.0;
const HEAD_LINE_HEIGHT: f32 = 19.0;
const TITLE_LINE_HEIGHT: f32 = 23.0;
const TEXT_LINE_HEIGHT: f32 = 22.0;
/// Where a text sits in its line: the baseline is this share of the font size below the
/// middle of the line, which centers the capitals.
const BASELINE_SHIFT: f32 = 0.35;
/// Average character width per font size, generous so that clipped text stays in its box.
const TITLE_CHAR_EM: f32 = 0.56;
const TEXT_CHAR_EM: f32 = 0.52;
/// The full detail wraps the title and the text into at most this many lines; a longer one
/// ends in "…".
const TITLE_MAX_LINES: usize = 3;
const BODY_MAX_LINES: usize = 12;
const MAX_COLUMNS: usize = 4;
const PILL_HEIGHT: f32 = 20.0;
const PILL_CHAR_WIDTH: f32 = 6.6;
const PILL_PADDING: f32 = 16.0;
/// Free space a pill keeps to boxes and badges; a pill without it shows only its number.
const PILL_MARGIN: f32 = 4.0;
const GATE_HEIGHT: f32 = 18.0;
/// Room around a badge's "! " and label; a "+N" badge has no "! ".
const GATE_PADDING: f32 = 26.0;
const MORE_PADDING: f32 = 20.0;
const GATE_GAP: f32 = 6.0;
/// How far a badge reaches down into its box: it stays clear of the kind label.
const GATE_OVERLAP: f32 = 5.0;
/// Badges share this right part of their box's top edge; arrows enter left of them.
const GATE_ROOM_SHARE: f32 = 0.75;
/// Badges shown per box; one "+N" badge stands for the rest.
const MAX_GATES: usize = 2;
/// Two arrows' lines closer than this, side by side, read as one. A line moves away from
/// another in steps of `RUN_STEP`, at most `RUN_STEPS` either way.
const RUN_SPACING: f32 = 12.0;
const RUN_STEP: f32 = 6.0;
const RUN_STEPS: usize = 16;
/// Two collinear lines with less than this between their ends read as one.
const RUN_TOLERANCE: f32 = 16.0;
/// Room a line keeps to boxes, badges, lane titles and lane edges.
const RUN_MARGIN: f32 = 4.0;
/// Room an arrow's end keeps to the corners of its box side.
const PORT_INSET: f32 = 12.0;
/// Where along a segment a pill may sit, tried in this order.
const PILL_POSITIONS: [f32; 3] = [0.5, 0.25, 0.75];
const ARROW_HIT_DISTANCE: f32 = 8.0;

/// `data-kind` values of the clickable shapes; `data-id` is the content ID behind each.
pub const KIND_LANE: &str = "lane";
pub const KIND_BOX: &str = "box";
pub const KIND_GATE: &str = "gate";
pub const KIND_ARROW: &str = "arrow";
pub const KIND_PILL: &str = "pill";
/// Class modifier on the arrow and pill of a manual (dashed) step.
const MODIFIER_MANUAL: &str = "manual";
/// Marks an arrow color, followed by its index into `ARROW_COLORS`: in the class on arrow
/// and pill (`prism-arrow-c0` ...) and in the id of its arrowhead (`prism-head-c0` ...).
const COLOR_MARK: &str = "c";
/// Class modifier on every second lane, which the default colors paint in the second tone.
const MODIFIER_ALTERNATE: &str = "alt";
/// Class modifier on dropped (archived) content, which is drawn at `DROPPED_OPACITY`; a
/// dropped arrow is dashed with `DROPPED_DASH`, a dropped box says `DROPPED_LABEL`.
const MODIFIER_DROPPED: &str = "dropped";
const DROPPED_OPACITY: f32 = 0.45;
const DROPPED_DASH: &str = "2 4";
const DROPPED_LABEL: &str = "dropped";
/// Arrowhead markers: one per arrow color (see `COLOR_MARK`), and the focused one.
const HEAD: &str = "prism-head";
const HEAD_FOCUS: &str = "prism-head-focus";

const BACKGROUND: &str = "#101418";
const BOX_FILL: &str = "#1a2027";
const BOX_BORDER: &str = "#2e3440";
const TEXT: &str = "#d8dde3";
const DIM: &str = "#8b93a1";
const FAINT: &str = "#737c8c";
/// White, so that the focus differs from every kind and arrow color by more than hue.
const FOCUS: &str = "#ffffff";
const STEP: &str = "#4cc9f0";
const AMBER: &str = "#f59e0b";
const LANE_COLORS: [&str; 2] = ["#2dd4bf", "#6e9fff"];
/// Arrows take these colors so that neighbors differ (see `arrow_colors`).
pub const ARROW_COLORS: [&str; 6] = [
    "#94a3b8", "#f472b6", "#2dd4bf", "#fb923c", "#b794f6", "#a3e635",
];
const PILL_TEXT: &str = "#0d1117";
const GATE_TEXT: &str = "#1b1b1b";

/// How much of each item its box shows, least first. Text keeps its size at every detail;
/// boxes grow with what they show, and the layout with them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    /// The overview: number, kind and title in a narrow box.
    #[default]
    Names,
    /// Also the text on one line, as the list's card shows it.
    Subtitles,
    /// The whole title and text, wrapped in a wider box.
    Full,
}

impl Detail {
    pub const ALL: [Detail; 3] = [Detail::Names, Detail::Subtitles, Detail::Full];

    pub fn box_width(self) -> f32 {
        match self {
            Detail::Names => NAMES_BOX_WIDTH,
            Detail::Subtitles => BOX_WIDTH,
            Detail::Full => FULL_BOX_WIDTH,
        }
    }

    /// Characters of a title and of the text that fit on one line of its box.
    fn title_chars(self) -> usize {
        (self.text_width() / (TITLE_SIZE * TITLE_CHAR_EM)) as usize
    }

    fn text_chars(self) -> usize {
        (self.text_width() / (TEXT_SIZE * TEXT_CHAR_EM)) as usize
    }

    fn text_width(self) -> f32 {
        self.box_width() - TEXT_X - BOX_PADDING_RIGHT
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
    /// What the lane shows, " · dropped" included; `name` is its accessible name.
    title: String,
    name: String,
    color: &'static str,
    rect: WorldRect,
    dropped: bool,
}

struct Gate {
    id: Id,
    kind: ElementKind,
    /// The whole title; `label` is what fits on the badge.
    title: String,
    label: String,
    rect: WorldRect,
    /// The "+N" badge for the badges beyond `MAX_GATES`: its ID is the first of them, its
    /// title lists them all.
    more: bool,
    dropped: bool,
}

struct Node {
    id: Id,
    /// The number the list shows for the item.
    number: usize,
    rect: WorldRect,
    /// The whole title on one line, for the accessible name; `title` is what the box shows.
    name: String,
    title: Vec<String>,
    /// The lines of text the detail shows: none, the subtitle, or the wrapped text.
    text: Vec<String>,
    kind: ElementKind,
    color: &'static str,
    gates: Vec<Gate>,
    dropped: bool,
}

struct Edge {
    id: Id,
    points: Vec<(f32, f32)>,
    /// Number and label; `shown` is what fits on the pill.
    label: String,
    shown: String,
    /// `<from> to <to>`, for the accessible name.
    ends: String,
    dashed: bool,
    direction: Direction,
    pill: WorldRect,
    /// Index into `ARROW_COLORS`.
    color: usize,
    dropped: bool,
}

/// What placing lanes and boxes leaves for routing the arrows.
struct Placement<'a> {
    layout: GraphLayout,
    lane_of: BTreeMap<Id, usize>,
    /// The visible relationships between shown boxes; `consumed_edges` became badges.
    relationships: Vec<&'a Relationship>,
    consumed_edges: BTreeSet<Id>,
    lane_width: f32,
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
        ElementKind::Step => STEP,
        ElementKind::Finding => "#e5484d",
        ElementKind::Question => "#d783ff",
    }
}

/// The lines of title the box shows: the whole title wrapped with the full detail, else one
/// clipped line.
fn title_lines(title: &str, detail: Detail) -> Vec<String> {
    let lines = if detail == Detail::Full {
        wrap_words(title, detail.title_chars(), TITLE_MAX_LINES)
    } else {
        vec![clip(title, detail.title_chars())]
    };
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

/// The lines of text the box shows: none, the text on one line as the card's excerpt flows
/// it, or all of it wrapped.
fn text_lines(body: &str, detail: Detail) -> Vec<String> {
    // Controls escaped first: the SVG must stay valid XML.
    let body = single_line(body);
    match detail {
        Detail::Names => Vec::new(),
        Detail::Subtitles => {
            let line = clip(
                &body.split_whitespace().collect::<Vec<_>>().join(" "),
                detail.text_chars(),
            );
            if line.is_empty() {
                Vec::new()
            } else {
                vec![line]
            }
        }
        Detail::Full => wrap_words(&body, detail.text_chars(), BODY_MAX_LINES),
    }
}

/// `text` in lines of at most `chars` characters, broken between words; a word longer than a
/// line is broken too. Beyond `max_lines` lines, the last one ends in "…".
fn wrap_words(text: &str, chars: usize, max_lines: usize) -> Vec<String> {
    let chars = chars.max(1);
    let pieces = text.split_whitespace().flat_map(|word| {
        let letters: Vec<char> = word.chars().collect();
        letters
            .chunks(chars)
            .map(|piece| piece.iter().collect::<String>())
            .collect::<Vec<_>>()
    });
    let mut lines = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        let length = current.chars().count();
        if length > 0 && length + 1 + piece.chars().count() > chars {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                let last = lines.pop().expect("one line pushed");
                lines.push(clip(&format!("{last}…"), chars));
                return lines;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&piece);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Number and kind, the title, then the lines of text the detail shows: always one with
/// subtitles, so that their boxes line up.
fn box_height(detail: Detail, title_lines: usize, text_lines: usize) -> f32 {
    let text_lines = match detail {
        Detail::Names => 0,
        Detail::Subtitles => 1,
        Detail::Full => text_lines,
    };
    BOX_PADDING_Y * 2.0
        + HEAD_LINE_HEIGHT
        + title_lines as f32 * TITLE_LINE_HEIGHT
        + text_lines as f32 * TEXT_LINE_HEIGHT
}

/// The baseline of a text of `size` in the line that starts at `top`.
fn baseline(top: f32, line_height: f32, size: f32) -> f32 {
    top + line_height / 2.0 + size * BASELINE_SHIFT
}

impl GraphLayout {
    /// Lays out the content whose IDs are in `visible`: elements, relationships and groups.
    /// Archived content is drawn as dropped where it was, so numbers and places never shift;
    /// the caller leaves out hidden and filtered-out content. An arrow is drawn only when both
    /// of its ends are.
    pub fn new(document: &Document, visible: &BTreeSet<Id>, detail: Detail) -> Self {
        let Placement {
            mut layout,
            lane_of,
            relationships,
            consumed_edges,
            lane_width,
        } = Self::place(document, visible, detail);
        let positions = document.positions();
        let rects: BTreeMap<Id, WorldRect> = layout
            .nodes
            .iter()
            .map(|node| (node.id, node.rect))
            .collect();
        // What a pill must not cover: boxes, badges and lane titles.
        let titles = layout
            .lanes
            .iter()
            .filter(|lane| !lane.title.is_empty())
            .map(|lane| WorldRect {
                x: lane.rect.x,
                y: lane.rect.y,
                width: lane.title.chars().count() as f32 * LANE_TITLE_CHAR_WIDTH
                    + LANE_TITLE_PADDING,
                height: LANE_TITLE_HEIGHT,
            });
        let blocked: Vec<WorldRect> = layout
            .nodes
            .iter()
            .flat_map(|node| {
                std::iter::once(node.rect).chain(node.gates.iter().map(|gate| gate.rect))
            })
            .chain(titles)
            .collect();
        let obstacles: Vec<WorldRect> = rects.values().copied().collect();
        let lane_bands: Vec<(WorldRect, f32)> = layout
            .lanes
            .iter()
            .map(|lane| (lane.rect, title_height(&lane.title)))
            .collect();
        let runs = Runs {
            blocked: &blocked,
            lanes: &lane_bands,
            width: lane_width,
        };
        let mut sorted: Vec<_> = relationships
            .iter()
            .filter(|item| !consumed_edges.contains(&item.id))
            .collect();
        // Pills count the arrows in publication order, so a revised arrow keeps its number.
        sorted.sort_by_key(|item| (positions[&item.id], item.id));
        let mut ends = Vec::new();
        for (index, relationship) in sorted.into_iter().enumerate() {
            let (Some(from), Some(to)) =
                (rects.get(&relationship.from), rects.get(&relationship.to))
            else {
                continue;
            };
            let lane = |id: &Id| layout.lanes[lane_of[id]].rect;
            let lanes = (lane(&relationship.from), lane(&relationship.to));
            let mut route = route(from, to, (&lanes.0, &lanes.1), lane_width, &obstacles);
            separate_runs(&mut route, &layout.edges, &runs);
            let points = route.points;
            let label = single_line(&relationship.label);
            let label = if label.starts_with(|character: char| character.is_ascii_digit()) {
                label
            } else {
                format!("{} {label}", index + 1)
            };
            let (shown, pill) = place_pill(&points, &label, &blocked, &layout.edges);
            let name = |id: &Id| single_line(&document.elements[id].title);
            ends.push(ArrowEnds {
                boxes: (relationship.from, relationship.to),
                lanes: (lane_of[&relationship.from], lane_of[&relationship.to]),
            });
            layout.edges.push(Edge {
                id: relationship.id,
                points,
                label,
                shown,
                ends: format!("{} to {}", name(&relationship.from), name(&relationship.to)),
                dashed: relationship.manual,
                direction: relationship.direction.clone(),
                pill,
                // Set below, once all arrows and their neighbors are known.
                color: 0,
                dropped: relationship.archived,
            });
        }
        for (edge, color) in layout.edges.iter_mut().zip(arrow_colors(&ends)) {
            edge.color = color;
        }
        layout
    }

    /// The bounds of the layout `new` makes, from placing lanes and boxes only, without
    /// routing the arrows.
    pub fn bounds_for(document: &Document, visible: &BTreeSet<Id>, detail: Detail) -> WorldRect {
        Self::place(document, visible, detail).layout.bounds
    }

    /// Lanes and boxes with their badges, and the bounds; the arrows are left to `new`.
    fn place<'a>(document: &'a Document, visible: &BTreeSet<Id>, detail: Detail) -> Placement<'a> {
        let elements: Vec<_> = {
            let mut items: Vec<_> = document
                .elements
                .values()
                .filter(|item| visible.contains(&item.id))
                .collect();
            items.sort_by_key(|item| (item.order, item.id));
            items
        };
        let shown: BTreeSet<Id> = elements.iter().map(|item| item.id).collect();
        // The list (`rows` in the app's `ui/js/rows.js`) numbers the visible records from 1 in
        // `list_order`, dropped ones included; boxes and badges show the same number.
        let numbers: BTreeMap<Id, usize> = document
            .list_order()
            .into_iter()
            .filter(|id| visible.contains(id))
            .enumerate()
            .map(|(index, id)| (id, index + 1))
            .collect();
        let relationships: Vec<_> = document
            .relationships
            .values()
            .filter(|item| {
                visible.contains(&item.id) && shown.contains(&item.from) && shown.contains(&item.to)
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
                title: single_line(&element.title),
                label: String::new(),
                rect: WorldRect::default(),
                more: false,
                dropped: element.archived,
            });
        }

        // Lanes follow groups; ungrouped boxes share one lane at the end.
        let mut lane_members: Vec<(Option<Id>, String, Vec<Id>)> = Vec::new();
        let mut placed = BTreeSet::new();
        let mut groups: Vec<_> = document
            .groups
            .values()
            .filter(|group| visible.contains(&group.id))
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
            let title = single_line(&group.title);
            let title = if group.archived {
                format!("{title} · {DROPPED_LABEL}")
            } else {
                title
            };
            lane_members.push((Some(group.id), title, members));
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
        let box_width = detail.box_width();
        let lane_width =
            LANE_PADDING * 2.0 + columns as f32 * box_width + (columns - 1) as f32 * BOX_GAP_X;
        let mut layout = GraphLayout {
            detail,
            ..Default::default()
        };
        let mut y = 0.0;
        let mut lane_of: BTreeMap<Id, usize> = BTreeMap::new();
        for (index, (group, title, members)) in lane_members.into_iter().enumerate() {
            let color = LANE_COLORS[index % LANE_COLORS.len()];
            let title_height = title_height(&title);
            let lane_top = y;
            let mut row_y = lane_top + title_height + LANE_PADDING;
            let mut row_height: f32 = 0.0;
            for (position, id) in members.iter().enumerate() {
                let element = &document.elements[id];
                let name = single_line(&element.title);
                let title = title_lines(&name, detail);
                let text = text_lines(&element.body, detail);
                let height = box_height(detail, title.len(), text.len());
                let column = position % columns;
                if column == 0 && position > 0 {
                    row_y += row_height + BOX_GAP_Y;
                    row_height = 0.0;
                }
                row_height = row_height.max(height);
                let rect = WorldRect {
                    x: LANE_PADDING + column as f32 * (box_width + BOX_GAP_X),
                    y: row_y,
                    width: box_width,
                    height,
                };
                let node_gates = place_gates(gates.remove(id).unwrap_or_default(), &rect);
                lane_of.insert(*id, index);
                layout.nodes.push(Node {
                    id: *id,
                    number: numbers[id],
                    rect,
                    name,
                    title,
                    text,
                    kind: element.kind.clone(),
                    color: kind_color(&element.kind),
                    gates: node_gates,
                    dropped: element.archived,
                });
            }
            let lane_height = title_height
                + LANE_PADDING * 2.0
                + (row_y - lane_top - title_height - LANE_PADDING)
                + row_height;
            let dropped = group.is_some_and(|id| document.groups[&id].archived);
            let name = group.map_or(String::new(), |id| {
                let title = single_line(&document.groups[&id].title);
                format!("Group: {title}{}", dropped_suffix(dropped))
            });
            layout.lanes.push(Lane {
                group,
                title,
                name,
                color,
                rect: WorldRect {
                    x: 0.0,
                    y: lane_top,
                    width: lane_width,
                    height: lane_height,
                },
                dropped,
            });
            y = lane_top + lane_height + LANE_GAP;
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
        Placement {
            layout,
            lane_of,
            relationships,
            consumed_edges,
            lane_width,
        }
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

    /// Focus order: boxes in lane order, each followed by its badges, then arrows. Lanes are
    /// not part of it; in the SVG's keyboard order they come first.
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

    /// The topmost shape at a point, in reverse paint order: pills, badges, boxes, arrows.
    pub fn hit(&self, x: f32, y: f32) -> Option<Id> {
        if let Some(edge) = self.edges.iter().find(|edge| edge.pill.contains(x, y)) {
            return Some(edge.id);
        }
        for node in &self.nodes {
            if let Some(gate) = node.gates.iter().find(|gate| gate.rect.contains(x, y)) {
                return Some(gate.id);
            }
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

    /// Paint order, bottom to top (`hit` checks it top to bottom): lanes, arrows, each box
    /// with its badges, pills. Keyboard order: lanes, then `ids()`, with one stop per arrow
    /// on its pill.
    pub fn svg(&self, focus: Option<Id>, selected: &BTreeSet<Id>) -> String {
        let mut svg = String::new();
        let width = self.bounds.width.max(1.0);
        let height = self.bounds.height.max(1.0);
        let _ = write!(
            svg,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" font-family="sans-serif">"#
        );
        svg.push_str("<defs>");
        let heads = ARROW_COLORS
            .iter()
            .enumerate()
            .map(|(index, color)| (head_id(index), *color))
            .chain(std::iter::once((HEAD_FOCUS.to_owned(), FOCUS)));
        for (id, color) in heads {
            let _ = write!(
                svg,
                r#"<marker id="{id}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{color}"/></marker>"#
            );
        }
        svg.push_str("</defs>");
        let _ = write!(
            svg,
            r#"<rect class="prism-background" x="0" y="0" width="{width}" height="{height}" fill="{BACKGROUND}"/>"#
        );
        for (index, lane) in self.lanes.iter().enumerate() {
            let r = lane.rect;
            let modifier = (index % 2 == 1).then_some(MODIFIER_ALTERNATE);
            match lane.group {
                Some(group) => open_clickable(
                    &mut svg,
                    group,
                    KIND_LANE,
                    modifier.as_slice(),
                    Some(&lane.name),
                    lane.dropped,
                ),
                // The lane of ungrouped boxes is not content: themed, not clickable.
                None => {
                    let modifier = modifier.map_or(String::new(), |name| format!(" prism-{name}"));
                    let _ = write!(svg, r#"<g class="prism-{KIND_LANE}{modifier}">"#);
                }
            }
            let _ = write!(
                svg,
                r#"<rect class="prism-shape" x="{}" y="{}" width="{}" height="{}" rx="4" fill="{}" fill-opacity="0.06" stroke="{}" stroke-opacity="0.35"/>"#,
                r.x, r.y, r.width, r.height, lane.color, lane.color
            );
            if !lane.title.is_empty() {
                let _ = write!(
                    svg,
                    r#"<text class="prism-title" x="{}" y="{}" fill="{}" font-size="11" font-weight="700" letter-spacing="1">{}</text>"#,
                    r.x + 10.0,
                    r.y + 16.0,
                    lane.color,
                    escape(&lane.title.to_uppercase())
                );
            }
            svg.push_str("</g>");
        }
        for edge in &self.edges {
            let focused = focus == Some(edge.id);
            let (stroke, marker) = edge_paint(edge, focused);
            let mut path = String::new();
            for (index, (x, y)) in edge.points.iter().enumerate() {
                let _ = write!(path, "{}{x},{y}", if index == 0 { "M" } else { " L" });
            }
            let head = format!("url(#{marker})");
            let markers = match edge.direction {
                Direction::Directed => format!(r#" marker-end="{head}""#),
                Direction::Bidirectional => {
                    format!(r#" marker-start="{head}" marker-end="{head}""#)
                }
                Direction::Undirected => String::new(),
            };
            // The pill is the arrow's keyboard stop; the line answers pointers only.
            open_clickable(
                &mut svg,
                edge.id,
                KIND_ARROW,
                &edge_modifiers(edge),
                None,
                edge.dropped,
            );
            // An invisible wide stroke makes the thin arrow easy to hit with a pointer.
            let _ = write!(
                svg,
                r#"<path class="prism-hit" d="{path}" fill="none" stroke="{BACKGROUND}" stroke-opacity="0" stroke-width="{}" pointer-events="stroke"/>"#,
                ARROW_HIT_DISTANCE * 2.0
            );
            let _ = write!(
                svg,
                r#"<path class="prism-line" d="{path}" fill="none" stroke="{stroke}" stroke-width="{}"{}{markers}/></g>"#,
                if focused { 3.0 } else { 1.8 },
                if edge.dropped {
                    format!(r#" stroke-dasharray="{DROPPED_DASH}""#)
                } else if edge.dashed {
                    r#" stroke-dasharray="6 5""#.to_owned()
                } else {
                    String::new()
                }
            );
        }
        for node in &self.nodes {
            let r = node.rect;
            let focused = focus == Some(node.id);
            let stroke = if focused { FOCUS } else { BOX_BORDER };
            let fill = if selected.contains(&node.id) {
                "#22303a"
            } else {
                BOX_FILL
            };
            open_clickable(
                &mut svg,
                node.id,
                KIND_BOX,
                &[kind_name(&node.kind)],
                Some(&format!(
                    "{}: {}{}",
                    kind_label(&node.kind),
                    node.name,
                    dropped_suffix(node.dropped)
                )),
                node.dropped,
            );
            let _ = write!(
                svg,
                r#"<rect class="prism-shape" x="{}" y="{}" width="{}" height="{}" rx="{BOX_RADIUS}" fill="{fill}" stroke="{stroke}" stroke-width="{}"/><rect class="prism-stripe" x="{}" y="{}" width="{STRIPE_WIDTH}" height="{}" rx="{}" fill="{}"/>"#,
                r.x,
                r.y,
                r.width,
                r.height,
                if focused { 3.0 } else { 1.0 },
                r.x + STRIPE_X,
                r.y + STRIPE_Y,
                r.height - STRIPE_Y * 2.0,
                STRIPE_WIDTH / 2.0,
                node.color
            );
            let head = r.y + BOX_PADDING_Y;
            let _ = write!(
                svg,
                r#"<text class="prism-number" x="{}" y="{}" text-anchor="end" fill="{FAINT}" font-size="{NUMBER_SIZE}">{}</text><text class="prism-kind" x="{}" y="{}" fill="{}" font-size="{KIND_SIZE}" font-weight="650" letter-spacing="{KIND_LETTER_SPACING}">{}</text>"#,
                r.x + NUMBER_RIGHT,
                baseline(head, HEAD_LINE_HEIGHT, NUMBER_SIZE),
                node.number,
                r.x + TEXT_X,
                baseline(head, HEAD_LINE_HEIGHT, KIND_SIZE),
                node.color,
                kind_label(&node.kind).to_uppercase()
            );
            // Right-aligned in the head line, as the card's badge sits at its right end.
            if node.dropped {
                let _ = write!(
                    svg,
                    r#"<text class="prism-dropped-label" x="{}" y="{}" text-anchor="end" fill="{DIM}" font-size="{KIND_SIZE}" font-weight="650" letter-spacing="{KIND_LETTER_SPACING}">{}</text>"#,
                    r.right() - BOX_PADDING_RIGHT,
                    baseline(head, HEAD_LINE_HEIGHT, KIND_SIZE),
                    DROPPED_LABEL.to_uppercase()
                );
            }
            let title = head + HEAD_LINE_HEIGHT;
            for (index, line) in node.title.iter().enumerate() {
                let _ = write!(
                    svg,
                    r#"<text class="prism-name" x="{}" y="{}" fill="{TEXT}" font-weight="600" font-size="{TITLE_SIZE}">{}</text>"#,
                    r.x + TEXT_X,
                    baseline(
                        title + index as f32 * TITLE_LINE_HEIGHT,
                        TITLE_LINE_HEIGHT,
                        TITLE_SIZE
                    ),
                    escape(line)
                );
            }
            let text = title + node.title.len() as f32 * TITLE_LINE_HEIGHT;
            // The one line of the subtitles is themed apart from the wrapped full text.
            let class = if self.detail == Detail::Subtitles {
                "prism-subtitle"
            } else {
                "prism-body"
            };
            for (index, line) in node.text.iter().enumerate() {
                let _ = write!(
                    svg,
                    r#"<text class="{class}" x="{}" y="{}" fill="{DIM}" font-size="{TEXT_SIZE}">{}</text>"#,
                    r.x + TEXT_X,
                    baseline(
                        text + index as f32 * TEXT_LINE_HEIGHT,
                        TEXT_LINE_HEIGHT,
                        TEXT_SIZE
                    ),
                    escape(line)
                );
            }
            svg.push_str("</g>");
            for gate in &node.gates {
                let g = gate.rect;
                let focused = focus == Some(gate.id);
                open_clickable(
                    &mut svg,
                    gate.id,
                    KIND_GATE,
                    &[kind_name(&gate.kind)],
                    Some(&if gate.more {
                        format!("{} more on {}: {}", gate.label, node.name, gate.title)
                    } else {
                        format!(
                            "{} on {}: {}{}",
                            kind_label(&gate.kind),
                            node.name,
                            gate.title,
                            dropped_suffix(gate.dropped)
                        )
                    }),
                    gate.dropped,
                );
                let text = if gate.more {
                    escape(&gate.label)
                } else if gate.label.is_empty() {
                    "!".to_owned()
                } else {
                    format!("! {}", escape(&gate.label))
                };
                let _ = write!(
                    svg,
                    r#"<rect class="prism-shape" x="{}" y="{}" width="{}" height="{}" rx="9" fill="{AMBER}" stroke="{}" stroke-width="{}"/><text class="prism-label" x="{}" y="{}" fill="{GATE_TEXT}" font-size="11" font-weight="700">{text}</text></g>"#,
                    g.x,
                    g.y,
                    g.width,
                    g.height,
                    if focused { FOCUS } else { AMBER },
                    if focused { 2.5 } else { 0.0 },
                    g.x + 10.0,
                    g.y + 13.0,
                );
            }
        }
        for edge in &self.edges {
            let (stroke, _) = edge_paint(edge, focus == Some(edge.id));
            let p = edge.pill;
            open_clickable(
                &mut svg,
                edge.id,
                KIND_PILL,
                &edge_modifiers(edge),
                Some(&format!(
                    "Arrow {}: {}{}",
                    edge.label,
                    edge.ends,
                    dropped_suffix(edge.dropped)
                )),
                edge.dropped,
            );
            let _ = write!(
                svg,
                r#"<title>{}</title><rect class="prism-shape" x="{}" y="{}" width="{}" height="{}" rx="10" fill="{stroke}"/><text class="prism-label" x="{}" y="{}" text-anchor="middle" fill="{PILL_TEXT}" font-size="11" font-weight="700">{}</text></g>"#,
                escape(&edge.label),
                p.x,
                p.y,
                p.width,
                p.height,
                p.center_x(),
                p.center_y() + 4.0,
                escape(&edge.shown)
            );
        }
        svg.push_str("</svg>");
        svg
    }
}

/// Lays the badges of one box along the right part of its top edge, right to left. Beyond
/// `MAX_GATES`, one "+N" badge stands for the rest. Labels are clipped to their share of
/// the `GATE_ROOM_SHARE` of the box's width.
fn place_gates(mut gates: Vec<Gate>, rect: &WorldRect) -> Vec<Gate> {
    if gates.len() > MAX_GATES {
        let rest = gates.split_off(MAX_GATES);
        let titles: Vec<&str> = rest.iter().map(|gate| gate.title.as_str()).collect();
        gates.push(Gate {
            id: rest[0].id,
            kind: rest[0].kind.clone(),
            title: titles.join(", "),
            label: format!("+{}", rest.len()),
            rect: WorldRect::default(),
            more: true,
            dropped: rest.iter().all(|gate| gate.dropped),
        });
    }
    let more_width =
        |gate: &Gate| gate.label.chars().count() as f32 * PILL_CHAR_WIDTH + MORE_PADDING;
    let reserved: f32 = gates.iter().filter(|gate| gate.more).map(more_width).sum();
    let labelled = gates.iter().filter(|gate| !gate.more).count().max(1) as f32;
    let gaps = GATE_GAP * gates.len().saturating_sub(1) as f32;
    let share = (rect.width * GATE_ROOM_SHARE - reserved - gaps) / labelled;
    let chars = ((share - GATE_PADDING) / PILL_CHAR_WIDTH).floor().max(0.0) as usize;
    let mut right = rect.right();
    for gate in &mut gates {
        let width = if gate.more {
            more_width(gate)
        } else {
            gate.label = if chars < 2 {
                String::new()
            } else {
                clip(&gate.title, chars)
            };
            gate.label.chars().count() as f32 * PILL_CHAR_WIDTH + GATE_PADDING
        };
        gate.rect = WorldRect {
            x: right - width,
            y: rect.y - GATE_HEIGHT + GATE_OVERLAP,
            width,
            height: GATE_HEIGHT,
        };
        right -= width + GATE_GAP;
    }
    gates
}

fn title_height(title: &str) -> f32 {
    if title.is_empty() {
        0.0
    } else {
        LANE_TITLE_HEIGHT
    }
}

/// What arrow lines keep clear of (boxes, badges, lane titles) and where they may run.
struct Runs<'a> {
    blocked: &'a [WorldRect],
    /// Each lane with the height of its title.
    lanes: &'a [(WorldRect, f32)],
    width: f32,
}

impl Runs<'_> {
    /// The heights a horizontal line at `y` may move within: its lane below the title, or
    /// the gap between two lanes, in both cases clear of the lane edges.
    fn band(&self, y: f32) -> (f32, f32) {
        for (lane, title) in self.lanes {
            if y >= lane.y && y <= lane.bottom() {
                return (lane.y + title + RUN_MARGIN, lane.bottom() - RUN_MARGIN);
            }
        }
        for pair in self.lanes.windows(2) {
            let (upper, lower) = (pair[0].0, pair[1].0);
            if y > upper.bottom() && y < lower.y {
                return (upper.bottom() + RUN_MARGIN, lower.y - RUN_MARGIN);
            }
        }
        (y, y)
    }
}

/// The side of a box an arrow starts or ends on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// An arrow's line and the box sides it starts and ends on.
struct Route {
    points: Vec<(f32, f32)>,
    from: (WorldRect, Side),
    to: (WorldRect, Side),
}

/// Whether a line's end at `point` sits on `side` of `rect`, clear of its corners.
fn on_side(point: (f32, f32), rect: &WorldRect, side: Side) -> bool {
    match side {
        Side::Top | Side::Bottom => {
            point.0 >= rect.x + PORT_INSET && point.0 <= rect.right() - PORT_INSET
        }
        Side::Left | Side::Right => {
            point.1 >= rect.y + PORT_INSET && point.1 <= rect.bottom() - PORT_INSET
        }
    }
}

/// Length the segment `a`-`b` shares with lines of `earlier` arrows that run beside it,
/// closer than `RUN_SPACING`, and end to end with less than `RUN_TOLERANCE` between.
fn shared(a: (f32, f32), b: (f32, f32), earlier: &[Edge]) -> f32 {
    let horizontal = a.1 == b.1;
    let (level, start, end) = if horizontal {
        (a.1, a.0.min(b.0), a.0.max(b.0))
    } else {
        (a.0, a.1.min(b.1), a.1.max(b.1))
    };
    let mut total = 0.0;
    for pair in earlier.iter().flat_map(|edge| edge.points.windows(2)) {
        let (c, d) = (pair[0], pair[1]);
        if c == d || (c.1 == d.1) != horizontal {
            continue;
        }
        let (other, other_start, other_end) = if horizontal {
            (c.1, c.0.min(d.0), c.0.max(d.0))
        } else {
            (c.0, c.1.min(d.1), c.1.max(d.1))
        };
        let overlap = end.min(other_end) - start.max(other_start);
        if (other - level).abs() < RUN_SPACING && overlap > -RUN_TOLERANCE {
            total += overlap + RUN_TOLERANCE;
        }
    }
    total
}

/// Moves each segment of an arrow off the lines of earlier arrows it would run on or right
/// beside, and off boxes, badges and lane titles. A segment between two bends moves within
/// its band (see `Runs::band`; vertical ones within the lanes' width), the first and last
/// along their box side, which spreads several arrows over one side. Where no place is free,
/// the segment takes the place it shares least, so a shared line is a last resort.
fn separate_runs(route: &mut Route, earlier: &[Edge], runs: &Runs) {
    let count = route.points.len();
    for index in 0..count.saturating_sub(1) {
        let (a, b) = (route.points[index], route.points[index + 1]);
        if a == b {
            continue;
        }
        let horizontal = a.1 == b.1;
        let band = if horizontal {
            runs.band(a.1)
        } else {
            (RUN_MARGIN, runs.width - RUN_MARGIN)
        };
        let mut ends = Vec::new();
        if index == 0 {
            ends.push((0, route.from));
        }
        if index + 2 == count {
            ends.push((1, route.to));
        }
        let shift = |offset: f32| {
            if horizontal {
                ((a.0, a.1 + offset), (b.0, b.1 + offset))
            } else {
                ((a.0 + offset, a.1), (b.0 + offset, b.1))
            }
        };
        let fits = |offset: f32| {
            let (c, d) = shift(offset);
            let level = if horizontal { c.1 } else { c.0 };
            let placed = if ends.is_empty() {
                level >= band.0 && level <= band.1
            } else {
                ends.iter()
                    .all(|(end, (rect, side))| on_side(if *end == 0 { c } else { d }, rect, *side))
            };
            let line = WorldRect {
                x: c.0.min(d.0),
                y: c.1.min(d.1),
                width: (c.0 - d.0).abs(),
                height: (c.1 - d.1).abs(),
            };
            let clear = runs.blocked.iter().all(|other| {
                ends.iter().any(|(_, (rect, _))| rect == other)
                    || !overlaps(&line, other, RUN_MARGIN)
            });
            placed && clear
        };
        if fits(0.0) && shared(a, b, earlier) == 0.0 {
            continue;
        }
        let offsets =
            (1..=RUN_STEPS).flat_map(|step| [-(step as f32) * RUN_STEP, step as f32 * RUN_STEP]);
        let best = std::iter::once(0.0)
            .chain(offsets)
            .filter(|offset| fits(*offset))
            .map(|offset| {
                let (c, d) = shift(offset);
                (offset, shared(c, d, earlier))
            })
            .min_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((offset, _)) = best {
            let (c, d) = shift(offset);
            route.points[index] = c;
            route.points[index + 1] = d;
        }
    }
}

/// Puts the pill on the arrow where it covers no box, badge or earlier pill: on its longest
/// segment if it can, in the middle before nearer one end. Where the whole label fits
/// nowhere (beside boxes the gap is narrow) the pill shows only its number; the whole label
/// stays in its accessible name.
fn place_pill(
    points: &[(f32, f32)],
    label: &str,
    blocked: &[WorldRect],
    earlier: &[Edge],
) -> (String, WorldRect) {
    let mut segments: Vec<_> = points.windows(2).map(|pair| (pair[0], pair[1])).collect();
    segments.sort_by(|a, b| length(b).total_cmp(&length(a)));
    let number = label.split_whitespace().next().unwrap_or("").to_owned();
    let free = |pill: &WorldRect| {
        blocked
            .iter()
            .chain(earlier.iter().map(|edge| &edge.pill))
            .all(|other| !overlaps(pill, other, PILL_MARGIN))
    };
    for text in [label, number.as_str()] {
        for ((x1, y1), (x2, y2)) in &segments {
            for position in PILL_POSITIONS {
                let center = (x1 + (x2 - x1) * position, y1 + (y2 - y1) * position);
                let pill = pill_rect(center, text);
                if free(&pill) {
                    return (text.to_owned(), pill);
                }
            }
        }
    }
    let ((x1, y1), (x2, y2)) = segments.first().copied().unwrap_or_default();
    (
        number.clone(),
        pill_rect(((x1 + x2) / 2.0, (y1 + y2) / 2.0), &number),
    )
}

fn length(segment: &((f32, f32), (f32, f32))) -> f32 {
    let ((x1, y1), (x2, y2)) = *segment;
    (x2 - x1).abs() + (y2 - y1).abs()
}

fn pill_rect(center: (f32, f32), text: &str) -> WorldRect {
    let width = text.chars().count() as f32 * PILL_CHAR_WIDTH + PILL_PADDING;
    WorldRect {
        x: center.0 - width / 2.0,
        y: center.1 - PILL_HEIGHT / 2.0,
        width,
        height: PILL_HEIGHT,
    }
}

/// Whether `a`, grown by `margin` on every side, overlaps `b`.
fn overlaps(a: &WorldRect, b: &WorldRect, margin: f32) -> bool {
    a.x - margin < b.right()
        && b.x < a.right() + margin
        && a.y - margin < b.bottom()
        && b.y < a.bottom() + margin
}

/// The boxes and lanes an arrow joins, for choosing its color.
struct ArrowEnds {
    boxes: (Id, Id),
    lanes: (usize, usize),
}

impl ArrowEnds {
    /// Arrows inside one lane, between the same two lanes, or touching the same box are
    /// told apart by color.
    fn neighbors(&self, other: &ArrowEnds) -> bool {
        let same_lanes = self.lanes == other.lanes || self.lanes == (other.lanes.1, other.lanes.0);
        let (a, b) = self.boxes;
        let shares_box = [other.boxes.0, other.boxes.1]
            .iter()
            .any(|id| *id == a || *id == b);
        same_lanes || shares_box
    }
}

/// Color indices into `ARROW_COLORS`, in `ids()` order: each arrow takes the first color none
/// of its already colored neighbors has; when all are taken, the least used among them (the
/// first of those on a tie).
fn arrow_colors(ends: &[ArrowEnds]) -> Vec<usize> {
    let mut colors: Vec<usize> = Vec::with_capacity(ends.len());
    for (index, arrow) in ends.iter().enumerate() {
        let used: Vec<usize> = ends[..index]
            .iter()
            .zip(&colors)
            .filter(|(other, _)| arrow.neighbors(other))
            .map(|(_, color)| *color)
            .collect();
        let palette = 0..ARROW_COLORS.len();
        let color = palette
            .clone()
            .find(|color| !used.contains(color))
            .unwrap_or_else(|| {
                palette
                    .min_by_key(|color| used.iter().filter(|taken| *taken == color).count())
                    .expect("the palette is not empty")
            });
        colors.push(color);
    }
    colors
}

/// Arrow and pill color, and the arrowhead marker to match.
fn edge_paint(edge: &Edge, focused: bool) -> (&'static str, String) {
    if focused {
        (FOCUS, HEAD_FOCUS.to_owned())
    } else {
        (ARROW_COLORS[edge.color], head_id(edge.color))
    }
}

fn head_id(color: usize) -> String {
    format!("{HEAD}-{COLOR_MARK}{color}")
}

/// `manual` for a dashed arrow, then its color.
fn edge_modifiers(edge: &Edge) -> Vec<String> {
    edge.dashed
        .then(|| MODIFIER_MANUAL.to_owned())
        .into_iter()
        .chain(std::iter::once(format!(
            "{KIND_ARROW}-{COLOR_MARK}{}",
            edge.color
        )))
        .collect()
}

/// Ends the accessible name of dropped content.
fn dropped_suffix(dropped: bool) -> String {
    if dropped {
        format!(" ({DROPPED_LABEL})")
    } else {
        String::new()
    }
}

fn kind_name(kind: &ElementKind) -> &'static str {
    match kind {
        ElementKind::Text => "text",
        ElementKind::Step => "step",
        ElementKind::Finding => "finding",
        ElementKind::Question => "question",
    }
}

fn kind_label(kind: &ElementKind) -> &'static str {
    match kind {
        ElementKind::Text => "Text",
        ElementKind::Step => "Step",
        ElementKind::Finding => "Finding",
        ElementKind::Question => "Question",
    }
}

/// Opens the `<g>` around one clickable shape; the caller closes it. CSS themes the shape
/// through its classes: `prism-<kind>`, plus `prism-<modifier>` for each modifier. With a
/// `label` it is a keyboard stop named by it; without, it answers pointers only. A `dropped`
/// shape is faded and also carries `prism-dropped`.
fn open_clickable(
    svg: &mut String,
    id: Id,
    kind: &str,
    modifiers: &[impl AsRef<str>],
    label: Option<&str>,
    dropped: bool,
) {
    let mut modifier: String = modifiers
        .iter()
        .map(|name| format!(" prism-{}", name.as_ref()))
        .collect();
    let mut opacity = String::new();
    if dropped {
        modifier.push_str(&format!(" prism-{MODIFIER_DROPPED}"));
        opacity = format!(r#" opacity="{DROPPED_OPACITY}""#);
    }
    let focus = label.map_or(r#"tabindex="-1" aria-hidden="true""#.to_owned(), |label| {
        format!(
            r#"tabindex="0" role="button" aria-label="{}""#,
            escape(label)
        )
    });
    let _ = write!(
        svg,
        r#"<g class="prism-{kind}{modifier}" data-id="{id}" data-kind="{kind}"{opacity} {focus}>"#
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

/// Orthogonal route between two boxes. To another row it leaves the bottom (or top) and
/// turns once: in the gap below the row inside one lane, or in the middle of the gap between
/// two lanes, never on a lane's edge; a run that would cross another box detours through the
/// gap beside the column. Beside the target it runs from side to side; past a box in between
/// it runs below the row, clear of the badges on top. Below a row means below its tallest box:
/// with the full text, boxes of one row differ in height.
fn route(
    from: &WorldRect,
    to: &WorldRect,
    lanes: (&WorldRect, &WorldRect),
    lane_width: f32,
    obstacles: &[WorldRect],
) -> Route {
    let (from_lane, to_lane) = lanes;
    let blocked = |x: f32, top: f32, bottom: f32| {
        obstacles.iter().any(|other| {
            other != from
                && other != to
                && other.x < x
                && x < other.right()
                && other.bottom() > top
                && other.y < bottom
        })
    };
    let vertical = |downwards: bool| {
        let (leave, enter, sign, sides) = if downwards {
            (from.bottom(), to.y, 1.0, (Side::Bottom, Side::Top))
        } else {
            (from.y, to.bottom(), -1.0, (Side::Top, Side::Bottom))
        };
        let turn = if from_lane == to_lane && downwards {
            row_bottom(from, obstacles) + BOX_GAP_Y / 2.0
        } else if from_lane == to_lane {
            leave - BOX_GAP_Y / 2.0
        } else if downwards {
            from_lane.bottom() + LANE_GAP / 2.0
        } else {
            from_lane.y - LANE_GAP / 2.0
        };
        let (start_x, end_x) = (from.center_x(), to.center_x());
        let points = if !blocked(start_x, leave.min(turn), leave.max(turn))
            && !blocked(end_x, turn.min(enter), turn.max(enter))
        {
            vec![
                (start_x, leave),
                (start_x, turn),
                (end_x, turn),
                (end_x, enter),
            ]
        } else {
            let step = sign * BOX_GAP_Y / 4.0;
            let channel = if from.right() + BOX_GAP_X < lane_width {
                from.right() + BOX_GAP_X / 2.0
            } else {
                from.x - BOX_GAP_X / 2.0
            };
            vec![
                (start_x, leave),
                (start_x, leave + step),
                (channel, leave + step),
                (channel, enter - step),
                (end_x, enter - step),
                (end_x, enter),
            ]
        };
        Route {
            points,
            from: (*from, sides.0),
            to: (*to, sides.1),
        }
    };
    if to.y >= from.bottom() {
        return vertical(true);
    }
    if to.bottom() <= from.y {
        return vertical(false);
    }
    let (left, right) = if to.x >= from.right() {
        (from, to)
    } else {
        (to, from)
    };
    let between = obstacles.iter().any(|other| {
        other.x >= left.right()
            && other.right() <= right.x
            && other.y < from.bottom().max(to.bottom())
            && other.bottom() > from.y.min(to.y)
    });
    if between {
        let below = row_bottom(from, obstacles) + BOX_GAP_Y / 4.0;
        return Route {
            points: vec![
                (from.center_x(), from.bottom()),
                (from.center_x(), below),
                (to.center_x(), below),
                (to.center_x(), to.bottom()),
            ],
            from: (*from, Side::Bottom),
            to: (*to, Side::Bottom),
        };
    }
    // Boxes of one row share their top edge: the line runs level, through the lower one's middle.
    let y = from.y.max(to.y) + from.height.min(to.height) / 2.0;
    if to.x >= from.right() {
        Route {
            points: vec![(from.right(), y), (to.x, y)],
            from: (*from, Side::Right),
            to: (*to, Side::Left),
        }
    } else {
        Route {
            points: vec![(from.x, y), (to.right(), y)],
            from: (*from, Side::Left),
            to: (*to, Side::Right),
        }
    }
}

/// The bottom of the row of `rect`: boxes of one row share their top edge, the tallest one
/// sets the bottom.
fn row_bottom(rect: &WorldRect, obstacles: &[WorldRect]) -> f32 {
    obstacles
        .iter()
        .filter(|other| other.y == rect.y)
        .map(WorldRect::bottom)
        .fold(rect.bottom(), f32::max)
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
    fn wrapping_breaks_a_word_longer_than_a_line() {
        let lines = wrap_words("see abcdefghij", 4, 5);
        assert_eq!(lines, vec!["see", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn segment_distance_measures_to_the_closest_point() {
        assert_eq!(segment_distance((0.0, 0.0), (10.0, 0.0), (5.0, 3.0)), 3.0);
        assert_eq!(segment_distance((0.0, 0.0), (10.0, 0.0), (14.0, 0.0)), 4.0);
    }
}
