//! Graphic diagram: lanes, boxes, arrows and gates laid out in world units and written as
//! SVG. The same geometry answers clicks; every clickable shape in the SVG carries `data-id`
//! and `data-kind` (one of the `KIND_*` values). Shapes and texts carry `prism-*` classes, so
//! a stylesheet can theme what the presentation attributes paint by default. Archived
//! (dropped) content keeps its place and number: it is drawn faded, with `prism-dropped`.
use crate::model::{Direction, Document, ElementKind, Id, Relationship, single_line};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
/// Free space a pill keeps to every other pill, so that each reads as its own.
const PILL_GAP: f32 = 6.0;
const GATE_HEIGHT: f32 = 18.0;
/// Room around a badge's "! " and label; a "+N" badge has no "! ".
const GATE_PADDING: f32 = 26.0;
const MORE_PADDING: f32 = 20.0;
const GATE_GAP: f32 = 6.0;
/// How far a badge reaches down into its box: it stays clear of the kind label, and of the
/// title of a box without head line.
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
/// Room below a lane's last row: an arrow past a box in that row runs `ARROW_END_RUN` below
/// it, and a second one `RUN_SPACING` further, both clear of the lane's edge.
const LANE_BOTTOM_PADDING: f32 = ARROW_END_RUN + RUN_SPACING + RUN_MARGIN;
/// Room an arrow's end keeps to the corners of its box side.
const PORT_INSET: f32 = 12.0;
/// A pill slides along its segment from the middle outwards in steps of this length, at most
/// `PILL_SLIDE_STEPS` to either side: it stays near the middle, where it reads as its arrow's.
const PILL_SLIDE_STEP: f32 = 6.0;
const PILL_SLIDE_STEPS: usize = 64;
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
/// Width the dropped label takes at the right end of a title line, with room before it: a
/// box without head line keeps it free on all its title lines.
const DROPPED_LABEL_ROOM: f32 = 64.0;
/// Arrowhead markers: one per arrow color (see `COLOR_MARK`), and the focused one.
const HEAD: &str = "prism-head";
const HEAD_FOCUS: &str = "prism-head-focus";
/// Width and height of an arrowhead in world units, whatever the line's stroke width
/// (`markerUnits="userSpaceOnUse"`), so a viewer can size it on screen.
const ARROW_HEAD_SIZE: f32 = 11.5;
/// An arrow runs straight into its head, and out of a start head, for at least this long: a
/// bend never sits inside the head.
pub const ARROW_END_RUN: f32 = ARROW_HEAD_SIZE + HEAD_CLEARANCE;
const HEAD_CLEARANCE: f32 = 8.0;

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

/// A straight piece of line, from one point to the next.
type Segment = ((f32, f32), (f32, f32));

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
    /// The number the list shows for the relationship.
    number: usize,
    /// The number the pill always shows, from `arrow_steps`.
    step: usize,
    points: Vec<(f32, f32)>,
    /// The relationship's label on one line.
    text: String,
    /// What fits on the pill of its step number and label.
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
    /// The list's number of every visible record.
    numbers: BTreeMap<Id, usize>,
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

/// The lines of title the box shows, of at most `chars` characters: the whole title wrapped
/// with the full detail, else one clipped line.
fn title_lines(title: &str, detail: Detail, chars: usize) -> Vec<String> {
    let lines = if detail == Detail::Full {
        wrap_words(title, chars, TITLE_MAX_LINES)
    } else {
        vec![clip(title, chars)]
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
/// subtitles, so that their boxes line up. A box without head line (see `has_head_line`) is
/// that line shorter.
fn box_height(detail: Detail, kind: &ElementKind, title_lines: usize, text_lines: usize) -> f32 {
    let text_lines = match detail {
        Detail::Names => 0,
        Detail::Subtitles => 1,
        Detail::Full => text_lines,
    };
    let head_line = if has_head_line(kind) {
        HEAD_LINE_HEIGHT
    } else {
        0.0
    };
    BOX_PADDING_Y * 2.0
        + head_line
        + title_lines as f32 * TITLE_LINE_HEIGHT
        + text_lines as f32 * TEXT_LINE_HEIGHT
}

/// Whether a box has the head line with its number and kind label. A text box stands for an
/// actor or a store, which its title names well enough: its number sits in the title's first
/// line.
fn has_head_line(kind: &ElementKind) -> bool {
    *kind != ElementKind::Text
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
        let placement = Self::place(document, visible, detail);
        let arrows = arrow_steps(document, &placement);
        let Placement {
            mut layout,
            lane_of,
            numbers,
            lane_width,
            ..
        } = placement;
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
            boxes: &obstacles,
            blocked_grid: Grid::of(&blocked),
            box_grid: Grid::of(&obstacles),
            lanes: &lane_bands,
            width: lane_width,
        };
        // All routes first: straight arrows on one line spread apart before any is separated.
        let mut routes = Vec::new();
        for (relationship, step) in arrows {
            let (Some(from), Some(to)) =
                (rects.get(&relationship.from), rects.get(&relationship.to))
            else {
                continue;
            };
            let lane = |id: &Id| layout.lanes[lane_of[id]].rect;
            let lanes = (lane(&relationship.from), lane(&relationship.to));
            let route = route(from, to, (&lanes.0, &lanes.1), lane_width, &obstacles);
            routes.push((relationship, step, route));
        }
        spread_straight_runs(&mut routes, &obstacles);
        let mut ends = Vec::new();
        for (relationship, step, mut route) in routes {
            let heads = match relationship.direction {
                Direction::Directed => (false, true),
                Direction::Bidirectional => (true, true),
                Direction::Undirected => (false, false),
            };
            separate_runs(&mut route, heads, &layout.edges, &runs);
            let points = route.points;
            let text = single_line(&relationship.label);
            let name = |id: &Id| single_line(&document.elements[id].title);
            ends.push(ArrowEnds {
                boxes: (relationship.from, relationship.to),
                lanes: (lane_of[&relationship.from], lane_of[&relationship.to]),
            });
            layout.edges.push(Edge {
                id: relationship.id,
                number: numbers[&relationship.id],
                step,
                points,
                text,
                // Pill and color are set below, once all arrows are known.
                shown: String::new(),
                ends: format!("{} to {}", name(&relationship.from), name(&relationship.to)),
                dashed: relationship.manual,
                direction: relationship.direction.clone(),
                pill: WorldRect::default(),
                color: 0,
                dropped: relationship.archived,
            });
        }
        // In step order: each pill keeps clear of the pills before it, and where it can of
        // every line and lane border.
        let borders: Vec<_> = layout
            .lanes
            .iter()
            .flat_map(|lane| {
                let r = lane.rect;
                let corners = [
                    (r.x, r.y),
                    (r.right(), r.y),
                    (r.right(), r.bottom()),
                    (r.x, r.bottom()),
                ];
                (0..corners.len()).map(move |side| (corners[side], corners[(side + 1) % 4]))
            })
            .collect();
        let mut room = PillRoom::new(&layout.edges, &blocked, borders);
        for index in 0..layout.edges.len() {
            let (shown, pill) = place_pill(index, &layout.edges, &room);
            room.pill_grid.insert(index, &pill);
            let edge = &mut layout.edges[index];
            edge.shown = shown;
            edge.pill = pill;
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

    /// `bounds_for`, with the step number each arrow's pill shows, by relationship ID: it is
    /// the same at every detail. Relationships drawn as badges have none.
    pub fn bounds_and_steps(
        document: &Document,
        visible: &BTreeSet<Id>,
        detail: Detail,
    ) -> (WorldRect, BTreeMap<Id, usize>) {
        let placement = Self::place(document, visible, detail);
        let steps = arrow_steps(document, &placement)
            .into_iter()
            .map(|(relationship, step)| (relationship.id, step))
            .collect();
        (placement.layout.bounds, steps)
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
                // Without a head line, the dropped label shares the title's lines.
                let chars = if element.archived && !has_head_line(&element.kind) {
                    let room = (DROPPED_LABEL_ROOM / (TITLE_SIZE * TITLE_CHAR_EM)).ceil() as usize;
                    detail.title_chars().saturating_sub(room)
                } else {
                    detail.title_chars()
                };
                let title = title_lines(&name, detail, chars);
                let text = text_lines(&element.body, detail);
                let height = box_height(detail, &element.kind, title.len(), text.len());
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
                + LANE_PADDING
                + LANE_BOTTOM_PADDING
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
            numbers,
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
                r#"<marker id="{id}" viewBox="0 0 10 10" refX="9" refY="5" markerUnits="userSpaceOnUse" markerWidth="{ARROW_HEAD_SIZE}" markerHeight="{ARROW_HEAD_SIZE}" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{color}"/></marker>"#
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
            // Number and the dropped label sit in the head line, or without one on the
            // baseline of the title's first line.
            let (title, number_line, label_line) = if has_head_line(&node.kind) {
                (
                    head + HEAD_LINE_HEIGHT,
                    baseline(head, HEAD_LINE_HEIGHT, NUMBER_SIZE),
                    baseline(head, HEAD_LINE_HEIGHT, KIND_SIZE),
                )
            } else {
                let line = baseline(head, TITLE_LINE_HEIGHT, TITLE_SIZE);
                (head, line, line)
            };
            let _ = write!(
                svg,
                r#"<text class="prism-number" x="{}" y="{}" text-anchor="end" fill="{FAINT}" font-size="{NUMBER_SIZE}">{}</text>"#,
                r.x + NUMBER_RIGHT,
                number_line,
                node.number
            );
            if has_head_line(&node.kind) {
                let _ = write!(
                    svg,
                    r#"<text class="prism-kind" x="{}" y="{}" fill="{}" font-size="{KIND_SIZE}" font-weight="650" letter-spacing="{KIND_LETTER_SPACING}">{}</text>"#,
                    r.x + TEXT_X,
                    baseline(head, HEAD_LINE_HEIGHT, KIND_SIZE),
                    node.color,
                    kind_label(&node.kind).to_uppercase()
                );
            }
            // Right-aligned in the head line (else the title's first line), as the card's badge
            // sits at its right end.
            if node.dropped {
                let _ = write!(
                    svg,
                    r#"<text class="prism-dropped-label" x="{}" y="{}" text-anchor="end" fill="{DIM}" font-size="{KIND_SIZE}" font-weight="650" letter-spacing="{KIND_LETTER_SPACING}">{}</text>"#,
                    r.right() - BOX_PADDING_RIGHT,
                    label_line,
                    DROPPED_LABEL.to_uppercase()
                );
            }
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
            // Named by its step and its list number, as the list and the panel name it.
            let mut name = format!("Step {}, link {}", edge.step, edge.number);
            if !edge.text.trim().is_empty() {
                let _ = write!(name, ", {}", edge.text.trim());
            }
            open_clickable(
                &mut svg,
                edge.id,
                KIND_PILL,
                &edge_modifiers(edge),
                Some(&format!(
                    "{name}: {}{}",
                    edge.ends,
                    dropped_suffix(edge.dropped)
                )),
                edge.dropped,
            );
            let _ = write!(
                svg,
                r#"<title>{}</title><rect class="prism-shape" x="{}" y="{}" width="{}" height="{}" rx="10" fill="{stroke}"/><text class="prism-label" x="{}" y="{}" text-anchor="middle" fill="{PILL_TEXT}" font-size="11" font-weight="700">{}</text></g>"#,
                escape(&name),
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
    /// No line runs through a box, not even through one of its own ends.
    boxes: &'a [WorldRect],
    /// `blocked` and `boxes` by place.
    blocked_grid: Grid,
    box_grid: Grid,
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

/// Spreads straight arrows that run on one line apart, symmetric around it: both ways between
/// side-by-side boxes, or several between boxes of one column. Their pills have nowhere else
/// to go, so the lines keep `PILL_HEIGHT` and `PILL_GAP` apart when level, the widest number
/// pill and `PILL_GAP` when upright. Where side-by-side boxes leave room for fewer level
/// lines, the later arrows run below the row (`below_row`). Where the upright lines' boxes
/// leave less room, they share it evenly; closer than `RUN_SPACING`, `separate_runs` places
/// them instead.
fn spread_straight_runs(routes: &mut [(&Relationship, usize, Route)], obstacles: &[WorldRect]) {
    // Level or not, the line's level, and the stretch it covers.
    let straight = |route: &Route| -> Option<(bool, f32, (f32, f32))> {
        let points = &route.points;
        let (first, last) = (points[0], points[points.len() - 1]);
        let stretch = |a: f32, b: f32| (a.min(b), a.max(b));
        if points.iter().all(|point| point.1 == first.1) {
            Some((true, first.1, stretch(first.0, last.0)))
        } else if points.iter().all(|point| point.0 == first.0) {
            Some((false, first.0, stretch(first.1, last.1)))
        } else {
            None
        }
    };
    let mut spread = vec![false; routes.len()];
    for index in 0..routes.len() {
        if spread[index] {
            continue;
        }
        let Some((level_line, level, stretch)) = straight(&routes[index].2) else {
            continue;
        };
        let group: Vec<usize> = (index..routes.len())
            .filter(|other| {
                !spread[*other]
                    && straight(&routes[*other].2).is_some_and(|(line, at, other)| {
                        line == level_line
                            && at == level
                            && other.0 < stretch.1
                            && stretch.0 < other.1
                    })
            })
            .collect();
        for member in &group {
            spread[*member] = true;
        }
        if group.len() < 2 {
            continue;
        }

        // The room on the box sides that all their ends sit on.
        let room = group
            .iter()
            .flat_map(|member| [routes[*member].2.from.0, routes[*member].2.to.0])
            .fold((f32::MIN, f32::MAX), |(low, high), rect| {
                if level_line {
                    (
                        low.max(rect.y + PORT_INSET),
                        high.min(rect.bottom() - PORT_INSET),
                    )
                } else {
                    (
                        low.max(rect.x + PORT_INSET),
                        high.min(rect.right() - PORT_INSET),
                    )
                }
            });
        let wanted = if level_line {
            PILL_HEIGHT + PILL_GAP
        } else {
            group
                .iter()
                .map(|member| pill_rect((0.0, 0.0), &routes[*member].1.to_string()).width)
                .fold(0.0, f32::max)
                + PILL_GAP
        };
        let mut group = group;
        if level_line {
            let fit = ((room.1 - room.0).max(0.0) / wanted).floor() as usize + 1;
            for member in group.split_off(fit.min(group.len())) {
                let (from, to) = (routes[member].2.from.0, routes[member].2.to.0);
                routes[member].2 = below_row(&from, &to, obstacles);
            }
            if group.len() < 2 {
                continue;
            }
        }
        let gaps = (group.len() - 1) as f32;
        let spacing = wanted.min((room.1 - room.0) / gaps);
        if spacing < RUN_SPACING {
            continue;
        }
        let half = spacing * gaps / 2.0;
        let middle = level.clamp(room.0 + half, room.1 - half);
        for (position, member) in group.iter().enumerate() {
            let offset = middle - half + position as f32 * spacing - level;
            for point in &mut routes[*member].2.points {
                if level_line {
                    point.1 += offset;
                } else {
                    point.0 += offset;
                }
            }
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
/// the segment takes the place it shares least, so a shared line is a last resort. A bend
/// next to an end with a head (`heads`: at the start, at the end) keeps the arrow's straight
/// run into that head.
fn separate_runs(route: &mut Route, heads: (bool, bool), earlier: &[Edge], runs: &Runs) {
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
        // What the segment and its neighbors may meet as it moves.
        let moved = &route.points[index.saturating_sub(1)..(index + 3).min(count)];
        let reach = RUN_STEPS as f32 * RUN_STEP + RUN_MARGIN;
        let region = moved.iter().fold(span(a, b), |region, point| {
            let x = region.x.min(point.0 - reach);
            let y = region.y.min(point.1 - reach);
            WorldRect {
                x,
                y,
                width: region.right().max(point.0 + reach) - x,
                height: region.bottom().max(point.1 + reach) - y,
            }
        });
        let near = |grid: &Grid, rects: &[WorldRect]| -> Vec<WorldRect> {
            grid.near(&region)
                .into_iter()
                .map(|index| rects[index])
                .collect()
        };
        let (blocked, boxes) = (
            near(&runs.blocked_grid, runs.blocked),
            near(&runs.box_grid, runs.boxes),
        );
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
            let line = span(c, d);
            let clear = blocked.iter().all(|other| {
                ends.iter().any(|(_, (rect, _))| rect == other)
                    || !overlaps(&line, other, RUN_MARGIN)
            });
            // Moving the segment stretches its neighbors: neither may cross a box.
            let before = (index > 0).then(|| span(route.points[index - 1], c));
            let after = (index + 2 < count).then(|| span(d, route.points[index + 2]));
            let through = std::iter::once(line)
                .chain(before)
                .chain(after)
                .any(|stretch| boxes.iter().any(|rect| overlaps(&stretch, rect, 0.0)));
            let kept = (!heads.0 || index != 1 || keeps_end_run(route.points[0], a, c))
                && (!heads.1 || index + 3 != count || keeps_end_run(route.points[count - 1], b, d));
            placed && clear && !through && kept
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

/// The rectangle a level or upright line from `a` to `b` covers: zero high or wide.
fn span(a: (f32, f32), b: (f32, f32)) -> WorldRect {
    WorldRect {
        x: a.0.min(b.0),
        y: a.1.min(b.1),
        width: (a.0 - b.0).abs(),
        height: (a.1 - b.1).abs(),
    }
}

/// Whether the straight run from an arrow's `end` to its `bend` stays at least
/// `ARROW_END_RUN` long, in its direction, with the bend `moved`.
fn keeps_end_run(end: (f32, f32), bend: (f32, f32), moved: (f32, f32)) -> bool {
    let length = (bend.0 - end.0).abs() + (bend.1 - end.1).abs();
    length > 0.0
        && ((moved.0 - end.0) * (bend.0 - end.0) + (moved.1 - end.1) * (bend.1 - end.1)) / length
            >= ARROW_END_RUN
}

/// The relationships drawn as arrows, each with its step number: pills count them from 1 in
/// publication order, so a revised arrow keeps its number. A label's own digits are text.
fn arrow_steps<'a>(
    document: &Document,
    placement: &Placement<'a>,
) -> Vec<(&'a Relationship, usize)> {
    let positions = document.positions();
    let mut sorted: Vec<&'a Relationship> = placement
        .relationships
        .iter()
        .copied()
        .filter(|item| !placement.consumed_edges.contains(&item.id))
        .collect();
    sorted.sort_by_key(|item| (positions[&item.id], item.id));
    sorted
        .into_iter()
        .enumerate()
        .filter(|(_, item)| {
            placement.lane_of.contains_key(&item.from) && placement.lane_of.contains_key(&item.to)
        })
        .map(|(index, relationship)| (relationship, index + 1))
        .collect()
}

/// Puts the pill of arrow `index` on its line, clear of boxes, badges and lane titles by
/// `PILL_MARGIN` and of the pills of earlier steps by `PILL_GAP`: with its step number and
/// label, else with its number only (beside boxes the gap is narrow); the whole label stays in
/// its accessible name. It tries the arrow's segments longest first, each from its middle
/// outwards (see `slide_positions`), and of the free places takes the first that covers the
/// least of the other arrows' lines, of its own arrowheads and of the lane `borders`. Where
/// no place is free, it takes the one that overlaps least, so that pills stack only as a last
/// resort.
fn place_pill(index: usize, edges: &[Edge], room: &PillRoom) -> (String, WorldRect) {
    let edge = &edges[index];
    let label = format!("{} {}", edge.step, edge.text);
    let number = edge.step.to_string();
    let mut segments: Vec<_> = edge
        .points
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .collect();
    segments.sort_by(|a, b| length(b).total_cmp(&length(a)));
    let heads = head_runs(edge);
    let mut best: Option<((f32, usize, f32), String, WorldRect)> = None;
    for (form, text) in [label.as_str(), number.as_str()].into_iter().enumerate() {
        // A free place for the whole label comes before any for the number.
        if form > 0 && best.as_ref().is_some_and(|(least, _, _)| least.0 == 0.0) {
            break;
        }
        let size = pill_rect((0.0, 0.0), text);
        for segment in &segments {
            // What is near, by the window of `GRID_CELL` along the segment the pill is in.
            let mut windows: HashMap<i32, Near> = HashMap::new();
            for center in slide_positions(segment, (size.width, size.height)) {
                let along = if segment.0.1 == segment.1.1 {
                    center.0
                } else {
                    center.1
                };
                let window = (along / GRID_CELL).floor() as i32;
                let near = windows
                    .entry(window)
                    .or_insert_with(|| room.near(segment, window, &size, index));
                let pill = pill_rect(center, text);
                let area = near.overlap(&pill, room, edges);
                if best.as_ref().is_some_and(|(least, _, _)| area > least.0) {
                    continue;
                }
                let cost = (area, form, near.covered(&pill, room, &heads));
                // Free and on no other line: nothing comes before it.
                if cost == (0.0, 0, 0.0) {
                    return (text.to_owned(), pill);
                }
                // Overlap first, then the label before the number, then the lines covered.
                let better = best.as_ref().is_none_or(|(least, _, _)| {
                    let order = cost.0.total_cmp(&least.0);
                    let order = order.then(cost.1.cmp(&least.1));
                    order.then(cost.2.total_cmp(&least.2)).is_lt()
                });
                if better {
                    best = Some((cost, text.to_owned(), pill));
                }
            }
        }
    }
    best.map_or_else(
        || (number.clone(), pill_rect((0.0, 0.0), &number)),
        |(_, text, pill)| (text, pill),
    )
}

/// Side of the square cells `Grid` sorts rectangles into.
const GRID_CELL: f32 = 128.0;

/// Indices of rectangles by the square cells of `GRID_CELL` they touch, to find the ones near
/// a place without going through all of them.
#[derive(Default)]
struct Grid {
    cells: HashMap<(i32, i32), Vec<usize>>,
}

impl Grid {
    fn cells(rect: &WorldRect) -> impl Iterator<Item = (i32, i32)> {
        let cell = |value: f32| (value / GRID_CELL).floor() as i32;
        let (columns, rows) = (
            cell(rect.x)..=cell(rect.right()),
            cell(rect.y)..=cell(rect.bottom()),
        );
        columns.flat_map(move |column| rows.clone().map(move |row| (column, row)))
    }

    fn of(rects: &[WorldRect]) -> Self {
        let mut grid = Grid::default();
        for (index, rect) in rects.iter().enumerate() {
            grid.insert(index, rect);
        }
        grid
    }

    fn insert(&mut self, index: usize, rect: &WorldRect) {
        for cell in Self::cells(rect) {
            self.cells.entry(cell).or_default().push(index);
        }
    }

    /// The indices of the rectangles in the cells `rect` touches, each once, in increasing
    /// order: sums over them add up as over all of them in order.
    fn near(&self, rect: &WorldRect) -> Vec<usize> {
        let mut found: Vec<usize> = Self::cells(rect)
            .filter_map(|cell| self.cells.get(&cell))
            .flatten()
            .copied()
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }
}

/// What pills keep clear of, by place: boxes, badges and lane titles (`blocked`) and the
/// pills placed so far, and where they can every arrow's line and the lane borders.
struct PillRoom<'a> {
    blocked: &'a [WorldRect],
    blocked_grid: Grid,
    /// Every arrow's segments with the index of its arrow, then the lane borders.
    lines: Vec<(Option<usize>, Segment)>,
    line_grid: Grid,
    /// The pills placed so far, by the index of their arrow.
    pill_grid: Grid,
}

impl<'a> PillRoom<'a> {
    fn new(edges: &[Edge], blocked: &'a [WorldRect], borders: Vec<Segment>) -> Self {
        let blocked_grid = Grid::of(blocked);
        let lines: Vec<(Option<usize>, Segment)> = edges
            .iter()
            .enumerate()
            .flat_map(|(index, edge)| {
                edge.points
                    .windows(2)
                    .map(move |pair| (Some(index), (pair[0], pair[1])))
            })
            .chain(borders.into_iter().map(|border| (None, border)))
            .collect();
        let mut line_grid = Grid::default();
        for (index, (_, (a, b))) in lines.iter().enumerate() {
            line_grid.insert(index, &span(*a, *b));
        }
        PillRoom {
            blocked,
            blocked_grid,
            lines,
            line_grid,
            pill_grid: Grid::default(),
        }
    }

    /// What a pill of `size` on `segment` of arrow `index` may touch while its center is in
    /// `window`, the stretch of `GRID_CELL` along the segment that starts at `window` times it.
    fn near(&self, segment: &Segment, window: i32, size: &WorldRect, index: usize) -> Near {
        let reach = size.width.max(size.height) / 2.0 + PILL_GAP.max(PILL_MARGIN);
        let mut line = span(segment.0, segment.1);
        let (start, end) = (window as f32 * GRID_CELL, (window + 1) as f32 * GRID_CELL);
        if segment.0.1 == segment.1.1 {
            let x = line.x.max(start);
            line.width = line.right().min(end) - x;
            line.x = x;
        } else {
            let y = line.y.max(start);
            line.height = line.bottom().min(end) - y;
            line.y = y;
        }
        let sweep = WorldRect {
            x: line.x - reach,
            y: line.y - reach,
            width: line.width + 2.0 * reach,
            height: line.height + 2.0 * reach,
        };
        let (lines, borders) = self
            .line_grid
            .near(&sweep)
            .into_iter()
            .filter(|line| self.lines[*line].0 != Some(index))
            .partition(|line| self.lines[*line].0.is_some());
        Near {
            blocked: self.blocked_grid.near(&sweep),
            pills: self.pill_grid.near(&sweep),
            lines,
            borders,
        }
    }
}

/// Indices into a `PillRoom` of what is near one segment.
struct Near {
    blocked: Vec<usize>,
    pills: Vec<usize>,
    lines: Vec<usize>,
    borders: Vec<usize>,
}

impl Near {
    /// The area `pill` shares with boxes, badges and lane titles grown by `PILL_MARGIN`, and
    /// with the pills placed so far grown by `PILL_GAP`.
    fn overlap(&self, pill: &WorldRect, room: &PillRoom, edges: &[Edge]) -> f32 {
        let boxes: f32 = self
            .blocked
            .iter()
            .map(|other| overlap_area(pill, &room.blocked[*other], PILL_MARGIN))
            .sum();
        let pills: f32 = self
            .pills
            .iter()
            .map(|other| overlap_area(pill, &edges[*other].pill, PILL_GAP))
            .sum();
        boxes + pills
    }

    /// The length of other arrows' lines, of the arrow's own `heads` and of lane borders
    /// within `PILL_MARGIN` that `pill` covers.
    fn covered(&self, pill: &WorldRect, room: &PillRoom, heads: &[Segment]) -> f32 {
        let on = |rect: &WorldRect, lines: &[usize]| -> f32 {
            lines
                .iter()
                .map(|line| {
                    let (a, b) = room.lines[*line].1;
                    covered_length(rect, a, b)
                })
                .sum()
        };
        let grown = WorldRect {
            x: pill.x - PILL_MARGIN,
            y: pill.y - PILL_MARGIN,
            width: pill.width + 2.0 * PILL_MARGIN,
            height: pill.height + 2.0 * PILL_MARGIN,
        };
        let own: f32 = heads
            .iter()
            .map(|(a, b)| covered_length(pill, *a, *b))
            .sum();
        on(pill, &self.lines) + own + on(&grown, &self.borders)
    }
}

/// The stretches of an arrow's line its heads cover: the last `ARROW_HEAD_SIZE` before each
/// end with a head.
fn head_runs(edge: &Edge) -> Vec<Segment> {
    let points = &edge.points;
    let (start, end) = match edge.direction {
        Direction::Directed => (false, true),
        Direction::Bidirectional => (true, true),
        Direction::Undirected => (false, false),
    };
    let reversed: Vec<(f32, f32)> = points.iter().rev().copied().collect();
    [(start, &reversed), (end, points)]
        .into_iter()
        .filter(|(head, line)| *head && line.len() >= 2)
        .map(|(_, line)| {
            let (tip, before) = (line[line.len() - 1], line[line.len() - 2]);
            let total = (tip.0 - before.0).abs() + (tip.1 - before.1).abs();
            let share = if total > 0.0 {
                (ARROW_HEAD_SIZE / total).min(1.0)
            } else {
                0.0
            };
            let base = (
                tip.0 + (before.0 - tip.0) * share,
                tip.1 + (before.1 - tip.1) * share,
            );
            (base, tip)
        })
        .collect()
}

/// The centers a pill of `size` (width, height) may take on a segment: its middle, then
/// outwards in steps of `PILL_SLIDE_STEP` to either side, nearer the start first, at most
/// `PILL_SLIDE_STEPS` and as far as the pill stays within the segment's ends.
fn slide_positions(segment: &Segment, size: (f32, f32)) -> Vec<(f32, f32)> {
    let ((x1, y1), (x2, y2)) = *segment;
    let total = length(segment);
    let extent = if y1 == y2 { size.0 } else { size.1 };
    let reach = ((total - extent) / 2.0).max(0.0);
    let steps = ((reach / PILL_SLIDE_STEP).floor() as usize).min(PILL_SLIDE_STEPS);
    let at = |offset: f32| {
        let share = if total > 0.0 {
            0.5 + offset / total
        } else {
            0.5
        };
        (x1 + (x2 - x1) * share, y1 + (y2 - y1) * share)
    };
    std::iter::once(at(0.0))
        .chain((1..=steps).flat_map(|step| {
            let offset = step as f32 * PILL_SLIDE_STEP;
            [at(-offset), at(offset)]
        }))
        .collect()
}

fn length(segment: &((f32, f32), (f32, f32))) -> f32 {
    let ((x1, y1), (x2, y2)) = *segment;
    (x2 - x1).abs() + (y2 - y1).abs()
}

/// The area `a`, grown by `margin` on every side, shares with `b`.
fn overlap_area(a: &WorldRect, b: &WorldRect, margin: f32) -> f32 {
    let width = (a.right() + margin).min(b.right()) - (a.x - margin).max(b.x);
    let height = (a.bottom() + margin).min(b.bottom()) - (a.y - margin).max(b.y);
    width.max(0.0) * height.max(0.0)
}

/// The length of the line `a`-`b` that runs inside `rect`. Arrows run level or upright.
fn covered_length(rect: &WorldRect, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (level, across, start, end, along) = if a.1 == b.1 {
        (
            a.1,
            (rect.y, rect.bottom()),
            a.0,
            b.0,
            (rect.x, rect.right()),
        )
    } else {
        (
            a.0,
            (rect.x, rect.right()),
            a.1,
            b.1,
            (rect.y, rect.bottom()),
        )
    };
    if level <= across.0 || level >= across.1 {
        return 0.0;
    }
    (start.max(end).min(along.1) - start.min(end).max(along.0)).max(0.0)
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
            let step = sign * ARROW_END_RUN;
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
        return below_row(from, to, obstacles);
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

/// From the bottom of `from` down below its row, along it and up into the bottom of `to`, a box
/// of the same row.
fn below_row(from: &WorldRect, to: &WorldRect, obstacles: &[WorldRect]) -> Route {
    let below = row_bottom(from, obstacles) + ARROW_END_RUN;
    Route {
        points: vec![
            (from.center_x(), from.bottom()),
            (from.center_x(), below),
            (to.center_x(), below),
            (to.center_x(), to.bottom()),
        ],
        from: (*from, Side::Bottom),
        to: (*to, Side::Bottom),
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
