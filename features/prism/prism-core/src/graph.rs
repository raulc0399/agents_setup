//! Graphic diagram: lanes, boxes, arrows and gates laid out in world units and written as
//! SVG. The same geometry answers clicks; every clickable shape in the SVG carries `data-id`
//! and `data-kind` (one of the `KIND_*` values). Shapes and texts carry `prism-*` classes, so
//! a stylesheet can theme what the presentation attributes paint by default. Archived
//! (dropped) content keeps its place and number: it is drawn faded, with `prism-dropped`.
//! Lanes are rows or columns (see `GraphLayout::orientation`); arrows run orthogonal, drawn
//! with rounded bends (see `rounded_path`).
use crate::model::{
    Direction, Document, Element, ElementKind, Id, Lanes, Relationship, single_line,
};
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
/// Room between lanes; an arrow from one lane to another turns in its middle. Column lanes
/// leave more where pills need it (see `ColumnRoom`), up to `MAX_COLUMN_GAP`.
const LANE_GAP: f32 = 48.0;
const MAX_COLUMN_GAP: f32 = 240.0;
/// Room left and right of a column lane's boxes at most (see `ColumnRoom::side`).
const MAX_COLUMN_SIDE: f32 = 100.0;
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
/// Boxes per row of a row lane at most; a column lane stacks up to `MAX_STACK` boxes before it
/// starts another column, as its boxes are wide and low.
const MAX_COLUMNS: usize = 4;
const MAX_STACK: usize = 6;
/// A lane shape further off 16:10 than this factor takes another number of boxes a line.
const MAX_MISFIT: f32 = 2.0;
/// The automatic lane orientation (see `GraphLayout::orientation`) prefers the shape closer to
/// a canvas of this width over height, and weighs the orientation against the flow by
/// `FLOW_PREFERENCE`.
const TARGET_ASPECT: f32 = 16.0 / 10.0;
const FLOW_PREFERENCE: f32 = 1.5;
/// All labels lost would add half the cost.
const LOST_LABEL_WEIGHT: f32 = 0.5;
const PILL_HEIGHT: f32 = 18.0;
const PILL_CHAR_WIDTH: f32 = 6.6;
const PILL_PADDING: f32 = 14.0;
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
/// Arrowhead markers: one per arrow color (see `COLOR_MARK`), and the focused one. Each name
/// is the marker's id and its class: a viewer showing several diagrams in one page renames the
/// ids and themes the markers by class.
const HEAD: &str = "prism-head";
const HEAD_FOCUS: &str = "prism-head-focus";
/// Width and height of an arrowhead in world units, whatever the line's stroke width
/// (`markerUnits="userSpaceOnUse"`), so a viewer can size it on screen.
pub const ARROW_HEAD_SIZE: f32 = 10.0;
/// Width of an arrow's line, and of the focused one.
const ARROW_STROKE: f32 = 1.5;
const ARROW_FOCUS_STROKE: f32 = 3.0;
/// Radius of an arrow's rounded bends, less where a segment is short (see `rounded_path`).
const CORNER_RADIUS: f32 = 8.0;
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

    /// The rectangle mirrored on the diagonal: x for y, width for height.
    fn transposed(&self) -> WorldRect {
        WorldRect {
            x: self.y,
            y: self.x,
            width: self.height,
            height: self.width,
        }
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
    /// What all lanes share across them: the width of rows, the height of columns.
    lane_length: f32,
    gaps: Gaps,
}

/// The room between boxes and between lanes, as routing sees it: for column lanes transposed.
#[derive(Clone, Copy)]
struct Gaps {
    /// Between boxes side by side in a row.
    beside: f32,
    /// Between rows of boxes.
    rows: f32,
    lanes: f32,
}

#[derive(Default)]
pub struct GraphLayout {
    lanes: Vec<Lane>,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    pub bounds: WorldRect,
    pub detail: Detail,
    /// Lanes as rows or columns (see `GraphLayout::orientation`).
    pub orientation: Lanes,
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
        Self::new_as(document, visible, detail, Self::orientation(document))
    }

    /// `new` with lanes as `lanes`.
    fn new_as(document: &Document, visible: &BTreeSet<Id>, detail: Detail, lanes: Lanes) -> Self {
        let placement = Self::place_as(document, visible, detail, lanes);
        let arrows = arrow_steps(document, &placement);
        let Placement {
            mut layout,
            lane_of,
            numbers,
            lane_length,
            gaps,
            ..
        } = placement;
        // Arrows are routed as between row lanes: column lanes, their mirror, are routed
        // transposed (x for y) and their lines turned back before the pills are placed.
        let columns = layout.orientation == Lanes::Columns;
        let routed = |rect: WorldRect| if columns { rect.transposed() } else { rect };
        let rects: BTreeMap<Id, WorldRect> = layout
            .nodes
            .iter()
            .map(|node| (node.id, routed(node.rect)))
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
        let blocked_routed: Vec<WorldRect> = blocked.iter().copied().map(routed).collect();
        let obstacles: Vec<WorldRect> = rects.values().copied().collect();
        // A column lane's title is on top, not where its level (transposed) lines run.
        let lane_bands: Vec<(WorldRect, f32)> = layout
            .lanes
            .iter()
            .map(|lane| {
                let title = if columns {
                    0.0
                } else {
                    title_height(&lane.title)
                };
                (routed(lane.rect), title)
            })
            .collect();
        let runs = Runs {
            blocked: &blocked_routed,
            boxes: &obstacles,
            blocked_grid: Grid::of(&blocked_routed),
            box_grid: Grid::of(&obstacles),
            lanes: &lane_bands,
            // Column lanes keep their title band at the top free, as rows keep their left edge.
            across: (
                if columns {
                    lane_bands_top(&layout.lanes) + RUN_MARGIN
                } else {
                    RUN_MARGIN
                },
                lane_length - RUN_MARGIN,
            ),
        };
        // All routes first: straight arrows on one line spread apart before any is separated.
        let mut routes = Vec::new();
        for (relationship, step) in arrows {
            let (Some(from), Some(to)) =
                (rects.get(&relationship.from), rects.get(&relationship.to))
            else {
                continue;
            };
            let lane = |id: &Id| routed(layout.lanes[lane_of[id]].rect);
            let lanes = (lane(&relationship.from), lane(&relationship.to));
            let route = route(
                from,
                to,
                (&lanes.0, &lanes.1),
                lane_length,
                gaps,
                &obstacles,
            );
            routes.push((relationship, step, route));
        }
        spread_straight_runs(&mut routes, &obstacles, columns);
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
        if columns {
            for edge in &mut layout.edges {
                for point in &mut edge.points {
                    *point = (point.1, point.0);
                }
            }
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

    /// Lanes and boxes with their badges, and the bounds, in the lane orientation of
    /// `orientation`; the arrows are left to `new`.
    fn place<'a>(document: &'a Document, visible: &BTreeSet<Id>, detail: Detail) -> Placement<'a> {
        Self::place_as(document, visible, detail, Self::orientation(document))
    }

    /// The lane orientation of the diagram: the one stored with the document (the bridge
    /// stores `automatic_lanes` at the first publication with two lanes), else, for a document
    /// stored before that, `automatic_lanes` now, else rows. It never depends on what is shown.
    pub fn orientation(document: &Document) -> Lanes {
        document
            .lanes
            .or_else(|| Self::automatic_lanes(document))
            .unwrap_or_default()
    }

    /// The automatic lane orientation of the whole document, dropped content included; `None`
    /// while it has fewer than two lanes. It lays the document out both ways with subtitles and
    /// takes the orientation of the lower cost, rows on a tie. The cost is the factor by which
    /// the bounds' width over height is off `TARGET_ASPECT` (one or more), times
    /// `FLOW_PREFERENCE` for the orientation against the flow, times one plus
    /// `LOST_LABEL_WEIGHT` times the share of labelled arrows whose pill shows only its number. Columns go with the flow when more of
    /// the arrows drawn run from one lane to another than stay inside one (hand-overs between
    /// places), rows when fewer; on a tie neither does. So the flow decides unless the other
    /// orientation's shape is more than `FLOW_PREFERENCE` times closer to 16:10, as for lanes
    /// of two boxes, which make flat columns, or it keeps more labels.
    pub fn automatic_lanes(document: &Document) -> Option<Lanes> {
        let all: BTreeSet<Id> = document
            .elements
            .keys()
            .chain(document.relationships.keys())
            .chain(document.groups.keys())
            .copied()
            .collect();
        let rows = Self::place_as(document, &all, Detail::Subtitles, Lanes::Rows);
        if rows.layout.lanes.len() < 2 {
            return None;
        }
        let (crossing, inside): (Vec<_>, Vec<_>) = arrow_steps(document, &rows)
            .into_iter()
            .partition(|(item, _)| rows.lane_of[&item.from] != rows.lane_of[&item.to]);
        let flow = match crossing.len().cmp(&inside.len()) {
            std::cmp::Ordering::Greater => Some(Lanes::Columns),
            std::cmp::Ordering::Less => Some(Lanes::Rows),
            std::cmp::Ordering::Equal => None,
        };
        let cost = |lanes: Lanes| {
            let layout = Self::new_as(document, &all, Detail::Subtitles, lanes);
            let ratio = layout.bounds.width / layout.bounds.height / TARGET_ASPECT;
            let against = flow.is_some_and(|flow| flow != lanes);
            let (lost, labelled) = layout.lost_labels();
            ratio.max(1.0 / ratio)
                * if against { FLOW_PREFERENCE } else { 1.0 }
                * (1.0 + LOST_LABEL_WEIGHT * lost as f32 / labelled.max(1) as f32)
        };
        if cost(Lanes::Columns) < cost(Lanes::Rows) {
            Some(Lanes::Columns)
        } else {
            Some(Lanes::Rows)
        }
    }

    /// Arrows whose pill shows only its number though they have a label, and arrows with one.
    fn lost_labels(&self) -> (usize, usize) {
        let labelled: Vec<&Edge> = self
            .edges
            .iter()
            .filter(|edge| !edge.text.trim().is_empty())
            .collect();
        let lost = labelled
            .iter()
            .filter(|edge| edge.shown == edge.step.to_string())
            .count();
        (lost, labelled.len())
    }

    /// Lanes and boxes with their badges, and the bounds, with lanes as `lanes`.
    fn place_as<'a>(
        document: &'a Document,
        visible: &BTreeSet<Id>,
        detail: Detail,
        lanes: Lanes,
    ) -> Placement<'a> {
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

        // Every box shaped first: column lanes line their rows up by the tallest box of each.
        let shaped: Vec<(Option<Id>, String, Vec<Shaped>)> = lane_members
            .into_iter()
            .map(|(group, title, members)| {
                let members = members
                    .into_iter()
                    .map(|id| shape(&document.elements[&id], detail))
                    .collect();
                (group, title, members)
            })
            .collect();
        let most = shaped
            .iter()
            .map(|(_, _, members)| members.len())
            .max()
            .unwrap_or(1)
            .max(1);
        let box_width = detail.box_width();
        let mut layout = GraphLayout {
            detail,
            orientation: lanes,
            ..Default::default()
        };
        let heights: Vec<Vec<f32>> = shaped
            .iter()
            .map(|(_, _, members)| members.iter().map(|item| item.height).collect())
            .collect();
        let titles: Vec<f32> = shaped
            .iter()
            .map(|(_, title, _)| title_height(title))
            .collect();
        let room = column_room(&shaped, &relationships, &consumed_edges);
        let places_with = |heights: &[Vec<f32>], per_line: usize, box_width: f32| match lanes {
            Lanes::Rows => row_places(heights, &titles, per_line, box_width),
            Lanes::Columns => column_places(heights, &titles, per_line, box_width, room),
        };
        // Boxes per line along the lane (per row of a row lane, per column of a column lane):
        // a square-ish block of at most `MAX_COLUMNS` a row, or a stack of at most `MAX_STACK`.
        // Where that cap makes long lanes a strip more than `MAX_MISFIT` off 16:10, the number
        // closest to 16:10 instead. Chosen with subtitles, so the same at every detail.
        let usual = match lanes {
            Lanes::Rows => ((most as f32).sqrt().ceil() as usize).clamp(1, MAX_COLUMNS),
            Lanes::Columns => most.div_ceil(most.div_ceil(MAX_STACK)),
        };
        let reference: Vec<Vec<f32>> = shaped
            .iter()
            .map(|(_, _, members)| {
                members
                    .iter()
                    .map(|item| {
                        box_height(Detail::Subtitles, &document.elements[&item.id].kind, 1, 1)
                    })
                    .collect()
            })
            .collect();
        let shape_of = |per_line: usize| {
            misfit(&places_with(
                &reference,
                per_line,
                Detail::Subtitles.box_width(),
            ))
        };
        let capped = match lanes {
            Lanes::Rows => most > MAX_COLUMNS * MAX_COLUMNS,
            Lanes::Columns => most > MAX_STACK,
        };
        let per_line = if !capped || shape_of(usual) <= MAX_MISFIT {
            usual
        } else {
            (1..=most)
                .min_by(|a, b| shape_of(*a).total_cmp(&shape_of(*b)))
                .unwrap_or(usual)
        };
        let places = places_with(&heights, per_line, box_width);
        let mut lane_of: BTreeMap<Id, usize> = BTreeMap::new();
        let lanes_and_boxes = places.lanes.iter().zip(&places.boxes);
        for (index, ((group, title, members), (lane_rect, rects))) in
            shaped.into_iter().zip(lanes_and_boxes).enumerate()
        {
            for (item, rect) in members.into_iter().zip(rects) {
                let element = &document.elements[&item.id];
                lane_of.insert(item.id, index);
                layout.nodes.push(Node {
                    id: item.id,
                    number: numbers[&item.id],
                    rect: *rect,
                    name: item.name,
                    title: item.title,
                    text: item.text,
                    kind: element.kind.clone(),
                    color: kind_color(&element.kind),
                    gates: place_gates(gates.remove(&item.id).unwrap_or_default(), rect),
                    dropped: element.archived,
                });
            }
            layout
                .lanes
                .push(lane(document, index, group, title, *lane_rect));
        }

        let (width, height) = layout.lanes.iter().fold((0.0_f32, 0.0_f32), |size, lane| {
            (
                size.0.max(lane.rect.right()),
                size.1.max(lane.rect.bottom()),
            )
        });
        layout.bounds = WorldRect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        Placement {
            layout,
            lane_of,
            numbers,
            relationships,
            consumed_edges,
            lane_length: places.length,
            gaps: places.gaps,
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
    /// An arrow answers along its polyline (`Edge::points`), not the rounded line drawn: a
    /// rounded bend stays within `CORNER_RADIUS` of its corner, well inside the hit distance.
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
                r#"<marker id="{id}" class="{id}" viewBox="0 0 10 10" refX="9" refY="5" markerUnits="userSpaceOnUse" markerWidth="{ARROW_HEAD_SIZE}" markerHeight="{ARROW_HEAD_SIZE}" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{color}"/></marker>"#
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
            let path = rounded_path(&edge.points);
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
            // An invisible wide stroke along the same rounded line makes the thin arrow easy to
            // hit with a pointer.
            let _ = write!(
                svg,
                r#"<path class="prism-hit" d="{path}" fill="none" stroke="{BACKGROUND}" stroke-opacity="0" stroke-width="{}" pointer-events="stroke"/>"#,
                ARROW_HIT_DISTANCE * 2.0
            );
            let _ = write!(
                svg,
                r#"<path class="prism-line" d="{path}" fill="none" stroke="{stroke}" stroke-width="{}"{}{markers}/></g>"#,
                if focused {
                    ARROW_FOCUS_STROKE
                } else {
                    ARROW_STROKE
                },
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
                r#"<title>{}</title><rect class="prism-shape" x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{stroke}"/><text class="prism-label" x="{}" y="{}" text-anchor="middle" fill="{PILL_TEXT}" font-size="11" font-weight="700">{}</text></g>"#,
                escape(&name),
                p.x,
                p.y,
                p.width,
                p.height,
                PILL_HEIGHT / 2.0,
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

/// A box's texts and height, before it has its place.
struct Shaped {
    id: Id,
    name: String,
    title: Vec<String>,
    text: Vec<String>,
    height: f32,
}

fn shape(element: &Element, detail: Detail) -> Shaped {
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
    Shaped {
        id: element.id,
        name,
        title,
        text,
        height,
    }
}

/// The lane at `index` of the group `group` (none for the lane of ungrouped boxes).
fn lane(
    document: &Document,
    index: usize,
    group: Option<Id>,
    title: String,
    rect: WorldRect,
) -> Lane {
    let dropped = group.is_some_and(|id| document.groups[&id].archived);
    let name = group.map_or(String::new(), |id| {
        let title = single_line(&document.groups[&id].title);
        format!("Group: {title}{}", dropped_suffix(dropped))
    });
    Lane {
        group,
        title,
        name,
        color: LANE_COLORS[index % LANE_COLORS.len()],
        rect,
        dropped,
    }
}

/// Where the lanes and their boxes go: each lane's rectangle and its boxes' in order, what all
/// lanes share across them (`Placement::lane_length`) and the gaps as routing sees them.
struct Places {
    lanes: Vec<WorldRect>,
    boxes: Vec<Vec<WorldRect>>,
    length: f32,
    gaps: Gaps,
}

/// Row lanes stacked top to bottom, of boxes as high as `heights` (per lane) and titles as
/// high as `titles`: `per_line` boxes a row, each row as high as its tallest box.
fn row_places(heights: &[Vec<f32>], titles: &[f32], per_line: usize, box_width: f32) -> Places {
    let width =
        LANE_PADDING * 2.0 + per_line as f32 * box_width + (per_line - 1) as f32 * BOX_GAP_X;
    let mut places = Places {
        lanes: Vec::new(),
        boxes: Vec::new(),
        length: width,
        gaps: Gaps {
            beside: BOX_GAP_X,
            rows: BOX_GAP_Y,
            lanes: LANE_GAP,
        },
    };
    let mut y = 0.0;
    for (members, title_height) in heights.iter().zip(titles) {
        let lane_top = y;
        let mut row_y = lane_top + title_height + LANE_PADDING;
        let mut row_height: f32 = 0.0;
        let mut boxes = Vec::new();
        for (position, height) in members.iter().enumerate() {
            let column = position % per_line;
            if column == 0 && position > 0 {
                row_y += row_height + BOX_GAP_Y;
                row_height = 0.0;
            }
            row_height = row_height.max(*height);
            boxes.push(WorldRect {
                x: LANE_PADDING + column as f32 * (box_width + BOX_GAP_X),
                y: row_y,
                width: box_width,
                height: *height,
            });
        }
        let lane_height = title_height
            + LANE_PADDING
            + LANE_BOTTOM_PADDING
            + (row_y - lane_top - title_height - LANE_PADDING)
            + row_height;
        places.lanes.push(WorldRect {
            x: 0.0,
            y: lane_top,
            width,
            height: lane_height,
        });
        places.boxes.push(boxes);
        y = lane_top + lane_height + LANE_GAP;
    }
    places
}

/// Column lanes side by side, the mirror of `row_places`, so that routing can run on them
/// transposed (see `GraphLayout::new`): boxes fill each column of a lane top to bottom,
/// `per_line` a column, and columns follow left to right, with `room` beside them and between
/// lanes. The gaps between boxes stay as in rows (between columns at least as wide as the
/// room beside them), so that pills fit; the title stays on top;
/// the rows of all lanes line up, each as tall as its tallest box, so that a line from lane
/// to lane can run straight.
fn column_places(
    heights: &[Vec<f32>],
    titles: &[f32],
    per_line: usize,
    box_width: f32,
    room: ColumnRoom,
) -> Places {
    let mut row_heights = vec![0.0_f32; per_line];
    for members in heights {
        for (position, height) in members.iter().enumerate() {
            let row = position % per_line;
            row_heights[row] = row_heights[row].max(*height);
        }
    }
    let top = titles.iter().copied().fold(0.0, f32::max) + LANE_PADDING;
    let side = room.side;
    // Between the columns of a lane, arrows past a box run as beside the lane's last one.
    let between = side.max(BOX_GAP_X);
    let row_tops: Vec<f32> = row_heights
        .iter()
        .scan(top, |next, height| {
            let row_top = *next;
            *next += height + BOX_GAP_Y;
            Some(row_top)
        })
        .collect();
    let lane_height = row_tops[per_line - 1] + row_heights[per_line - 1] + LANE_PADDING;
    // As routing sees them, transposed.
    let mut places = Places {
        lanes: Vec::new(),
        boxes: Vec::new(),
        length: lane_height,
        gaps: Gaps {
            beside: BOX_GAP_Y,
            rows: between,
            lanes: room.gap,
        },
    };
    let mut x = 0.0;
    for members in heights {
        let columns = members.len().div_ceil(per_line).max(1);
        let boxes = members
            .iter()
            .enumerate()
            .map(|(position, height)| WorldRect {
                x: x + side + (position / per_line) as f32 * (box_width + between),
                y: row_tops[position % per_line],
                width: box_width,
                height: *height,
            })
            .collect();
        let lane_width = 2.0 * side + columns as f32 * box_width + (columns - 1) as f32 * between;
        places.lanes.push(WorldRect {
            x,
            y: 0.0,
            width: lane_width,
            height: lane_height,
        });
        places.boxes.push(boxes);
        x += lane_width + room.gap;
    }
    places
}

/// Room around column lanes, from the labels of the arrows that run there (step numbers are
/// not known yet: a pill counts with the number of arrows).
#[derive(Clone, Copy)]
struct ColumnRoom {
    /// Between lanes, where arrows from lane to lane turn: the widest pill of such an arrow
    /// fits on its upright line, clear of both lanes by `PILL_MARGIN` also with the line
    /// `RUN_SPACING` off the middle (beside another one), between `LANE_GAP` and
    /// `MAX_COLUMN_GAP`.
    gap: f32,
    /// Left and right of a lane's boxes, where an arrow past a box of its column runs (the
    /// mirror of `below_row`), `ARROW_END_RUN` and a second `RUN_SPACING` further out: the
    /// widest pill of an arrow inside a lane that skips a box fits there off the lane's edge,
    /// between `LANE_BOTTOM_PADDING` and `MAX_COLUMN_SIDE`.
    side: f32,
}

fn column_room(
    lanes: &[(Option<Id>, String, Vec<Shaped>)],
    relationships: &[&Relationship],
    consumed: &BTreeSet<Id>,
) -> ColumnRoom {
    let place: HashMap<Id, (usize, usize)> = lanes
        .iter()
        .enumerate()
        .flat_map(|(lane, (_, _, members))| {
            members
                .iter()
                .enumerate()
                .map(move |(position, item)| (item.id, (lane, position)))
        })
        .collect();
    let number = relationships.len();
    let pill = |item: &Relationship| {
        pill_rect(
            (0.0, 0.0),
            &format!("{number} {}", single_line(&item.label)),
        )
        .width
    };
    let mut room = ColumnRoom {
        gap: LANE_GAP,
        side: LANE_BOTTOM_PADDING,
    };
    for item in relationships
        .iter()
        .filter(|item| !consumed.contains(&item.id))
    {
        let (Some(from), Some(to)) = (place.get(&item.from), place.get(&item.to)) else {
            continue;
        };
        if from.0 != to.0 {
            room.gap = room.gap.max(pill(item) + 2.0 * (PILL_MARGIN + RUN_SPACING));
        } else if from.1.abs_diff(to.1) > 1 {
            room.side = room
                .side
                .max(ARROW_END_RUN + RUN_SPACING + pill(item) / 2.0 + PILL_MARGIN);
        }
    }
    ColumnRoom {
        gap: room.gap.min(MAX_COLUMN_GAP),
        side: room.side.min(MAX_COLUMN_SIDE),
    }
}

/// The height of the tallest lane title: the band on top of column lanes.
fn lane_bands_top(lanes: &[Lane]) -> f32 {
    lanes
        .iter()
        .map(|lane| title_height(&lane.title))
        .fold(0.0, f32::max)
}

/// How far the lanes' extent is off `TARGET_ASPECT`, as a factor of one or more.
fn misfit(places: &Places) -> f32 {
    let (width, height) = places.lanes.iter().fold((0.0_f32, 0.0_f32), |size, lane| {
        (size.0.max(lane.right()), size.1.max(lane.bottom()))
    });
    let ratio = width / height / TARGET_ASPECT;
    ratio.max(1.0 / ratio)
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
    /// Where a line across the lanes (upright, as routing sees them) may run: within what
    /// all lanes share across them (`Placement::lane_length`), clear of their edges.
    across: (f32, f32),
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
/// them instead. Routes of column lanes are `transposed`: their level lines are upright on
/// screen, so they keep the pills' width apart, and the upright ones their height.
fn spread_straight_runs(
    routes: &mut [(&Relationship, usize, Route)],
    obstacles: &[WorldRect],
    transposed: bool,
) {
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
        let wanted = if level_line != transposed {
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
            runs.across
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

/// What a pill's place costs, compared in order: the area it overlaps, whether it is on a lane
/// border (1) or not (0), the label (0) or the number (1), and the lines it covers.
type PillCost = (f32, usize, usize, f32);

/// Puts the pill of arrow `index` on its line, clear of boxes, badges and lane titles by
/// `PILL_MARGIN` and of the pills of earlier steps by `PILL_GAP`: with its step number and
/// label, else with its number only (beside boxes the gap is narrow, and a place off the lane
/// borders comes first); the whole label stays in its accessible name. It tries the arrow's
/// segments longest first, each from its middle outwards (see `slide_positions`), and of the
/// free places takes the first that covers the least of the other arrows' lines, of its own
/// arrowheads and of the lane `borders`. Where
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
    let mut best: Option<(PillCost, String, WorldRect)> = None;
    for (form, text) in [label.as_str(), number.as_str()].into_iter().enumerate() {
        // A free place for the whole label, off the lane borders, comes before any for the
        // number.
        if form > 0
            && best
                .as_ref()
                .is_some_and(|(least, _, _)| least.0 == 0.0 && least.1 == 0)
        {
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
                let (lines, borders) = near.covered(&pill, room, &heads);
                let cost = (area, usize::from(borders > 0.0), form, lines + borders);
                // Free and on no other line: nothing comes before it.
                if cost == (0.0, 0, 0, 0.0) {
                    return (text.to_owned(), pill);
                }
                // Overlap first, then off the lane borders, then the label before the number,
                // then the lines covered.
                let better = best.as_ref().is_none_or(|(least, _, _)| {
                    let order = cost.0.total_cmp(&least.0);
                    let order = order.then(cost.1.cmp(&least.1));
                    let order = order.then(cost.2.cmp(&least.2));
                    order.then(cost.3.total_cmp(&least.3)).is_lt()
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

    /// The length of other arrows' lines and of the arrow's own `heads` that `pill` covers,
    /// and of lane borders within `PILL_MARGIN`.
    fn covered(&self, pill: &WorldRect, room: &PillRoom, heads: &[Segment]) -> (f32, f32) {
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
        (on(pill, &self.lines) + own, on(&grown, &self.borders))
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
/// with the full text, boxes of one row differ in height. Boxes side by side across the turn
/// (column lanes, routed transposed) are joined by one straight line where they overlap.
fn route(
    from: &WorldRect,
    to: &WorldRect,
    lanes: (&WorldRect, &WorldRect),
    lane_width: f32,
    gaps: Gaps,
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
            row_bottom(from, obstacles) + gaps.rows / 2.0
        } else if from_lane == to_lane {
            leave - gaps.rows / 2.0
        } else if downwards {
            from_lane.bottom() + gaps.lanes / 2.0
        } else {
            from_lane.y - gaps.lanes / 2.0
        };
        let (start_x, end_x) = upright_ends(from, to);
        let free = !blocked(start_x, leave.min(turn), leave.max(turn))
            && !blocked(end_x, turn.min(enter), turn.max(enter));
        // One straight line moves as a whole (see `separate_runs`), never into a jog.
        let points = if free && start_x == end_x {
            vec![(start_x, leave), (end_x, enter)]
        } else if free {
            vec![
                (start_x, leave),
                (start_x, turn),
                (end_x, turn),
                (end_x, enter),
            ]
        } else {
            let step = sign * ARROW_END_RUN;
            // The gap right of the column, else left of it, else (the lane's only column)
            // the middle of the room right of it, off the lane's edge.
            let right = column_right(from, obstacles);
            let channel = if right + gaps.beside < lane_width {
                right + gaps.beside / 2.0
            } else if from.x > gaps.beside {
                from.x - gaps.beside / 2.0
            } else {
                (right + lane_width) / 2.0
            };
            // From lane to lane it turns in the gaps between lanes, where it has room to
            // keep apart from others, unless a box of the lane is in the way there.
            let cross = from_lane != to_lane;
            let first = if cross && !blocked(start_x, leave.min(turn), leave.max(turn)) {
                turn
            } else {
                leave + step
            };
            let near_to = if downwards {
                to_lane.y - gaps.lanes / 2.0
            } else {
                to_lane.bottom() + gaps.lanes / 2.0
            };
            let last = if cross && !blocked(end_x, near_to.min(enter), near_to.max(enter)) {
                near_to
            } else {
                enter - step
            };
            vec![
                (start_x, leave),
                (start_x, first),
                (channel, first),
                (channel, last),
                (end_x, last),
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

/// Where an arrow from `from` to `to` above or below it leaves and enters: the middle of the
/// stretch both boxes cover side by side, so the line runs straight, else each box's middle.
fn upright_ends(from: &WorldRect, to: &WorldRect) -> (f32, f32) {
    let (left, right) = (from.x.max(to.x), from.right().min(to.right()));
    if right - left >= 2.0 * PORT_INSET {
        let middle = (left + right) / 2.0;
        (middle, middle)
    } else {
        (from.center_x(), to.center_x())
    }
}

/// The right edge of the column of `rect`: boxes of one column share their left edge, the
/// widest one sets it. Row lanes' boxes are all as wide; those of column lanes, routed
/// transposed, differ in height.
fn column_right(rect: &WorldRect, obstacles: &[WorldRect]) -> f32 {
    obstacles
        .iter()
        .filter(|other| other.x == rect.x)
        .map(WorldRect::right)
        .fold(rect.right(), f32::max)
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

/// The SVG path of an arrow's polyline with rounded bends: each bend becomes a quadratic curve
/// from `CORNER_RADIUS` before it to as far after it, with the bend as its control point. The
/// radius is less where a segment is short: an inner segment gives each of its two bends at
/// most half of it, an end segment keeps `ARROW_HEAD_SIZE` straight for a head, so a head
/// always sits on a straight run. Every bend is a `Q`, one of radius 0 too, so the polyline
/// reads back as the `M` point, the control point of every `Q` and the last point. Points on
/// one straight line are no bends and are left out.
fn rounded_path(points: &[(f32, f32)]) -> String {
    let mut corners: Vec<(f32, f32)> = Vec::with_capacity(points.len());
    for point in points {
        if corners.last() == Some(point) {
            continue;
        }
        if corners.len() >= 2 {
            let (a, b) = (corners[corners.len() - 2], corners[corners.len() - 1]);
            if direction(a, b) == direction(b, *point) {
                corners.pop();
            }
        }
        corners.push(*point);
    }
    let mut path = String::new();
    let Some(first) = corners.first() else {
        return path;
    };
    let _ = write!(path, "M{},{}", first.0, first.1);
    let last = corners.len() - 1;
    for index in 1..last {
        let (a, b, c) = (corners[index - 1], corners[index], corners[index + 1]);
        let room = |length: f32, end: bool| {
            if end {
                length - ARROW_HEAD_SIZE
            } else {
                length / 2.0
            }
        };
        let radius = CORNER_RADIUS
            .min(room(length(&(a, b)), index == 1))
            .min(room(length(&(b, c)), index + 1 == last))
            .max(0.0);
        let (into, out) = (direction(a, b), direction(b, c));
        let _ = write!(
            path,
            " L{},{} Q{},{} {},{}",
            b.0 - into.0 * radius,
            b.1 - into.1 * radius,
            b.0,
            b.1,
            b.0 + out.0 * radius,
            b.1 + out.1 * radius
        );
    }
    if last > 0 {
        let _ = write!(path, " L{},{}", corners[last].0, corners[last].1);
    }
    path
}

/// The unit step from `a` towards `b` along a level or upright line: each part -1, 0 or 1.
fn direction(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let sign = |value: f32| {
        if value > 0.0 {
            1.0
        } else if value < 0.0 {
            -1.0
        } else {
            0.0
        }
    };
    (sign(b.0 - a.0), sign(b.1 - a.1))
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
