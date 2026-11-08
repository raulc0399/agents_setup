use crate::model::{Direction, Document, Id, Position, ViewState, terminal_safe};
use crate::ui::list;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Widget};
use std::collections::{BTreeMap, BTreeSet};

const CARD_WIDTH: i32 = 30;
const CARD_HEIGHT: i32 = 5;
const ROW_SPACING: i32 = 7;
const LANE_SPACING: i32 = 42;
const POSITION_LIMIT: i32 = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Bounds {
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

#[derive(Clone, Debug)]
struct Card {
    id: Id,
    bounds: Bounds,
    label: String,
    caption: String,
    accepted: bool,
    stale_acceptance: bool,
    color: Color,
}

#[derive(Clone, Debug)]
struct Connection {
    id: Id,
    from: Bounds,
    to: Bounds,
    card: Bounds,
    direction: Direction,
}

/// One card per logical visible record; relationship cards require both visible endpoints.
#[derive(Clone, Debug, Default)]
pub struct DiagramLayout {
    cards: Vec<Card>,
    connections: Vec<Connection>,
    pub bounds: Bounds,
}

impl DiagramLayout {
    pub fn new(document: &Document, view: &ViewState) -> Self {
        let rows = list::rows(document, view);
        let elements: BTreeSet<_> = rows
            .iter()
            .filter(|row| document.elements.contains_key(&row.id))
            .map(|row| row.id)
            .collect();
        let mut layout = Self::default();
        let mut lane_rows = [0; 3];
        for row in rows {
            let (lane, caption, color) = if document.elements.contains_key(&row.id) {
                (0, String::new(), Color::Cyan)
            } else if let Some(relationship) = document.relationships.get(&row.id) {
                if !elements.contains(&relationship.from) || !elements.contains(&relationship.to) {
                    continue;
                }
                let arrow = match relationship.direction {
                    Direction::Directed => "->",
                    Direction::Undirected => "--",
                    Direction::Bidirectional => "<->",
                };
                // Endpoint captions supplement crowded or cyclic routes.
                let caption = format!(
                    "{} {arrow} {}",
                    short_title(&document.elements[&relationship.from].title),
                    short_title(&document.elements[&relationship.to].title),
                );
                (1, caption, Color::Blue)
            } else if let Some(group) = document.groups.get(&row.id) {
                (
                    2,
                    format!("{} members · details", group.members.len()),
                    Color::Yellow,
                )
            } else {
                continue;
            };
            let position = view.positions.get(&row.id);
            let bounds = Bounds {
                x: position.map_or(lane as i32 * LANE_SPACING, |value| {
                    value.x.clamp(-POSITION_LIMIT, POSITION_LIMIT)
                }),
                y: position.map_or(lane_rows[lane] * ROW_SPACING, |value| {
                    value.y.clamp(-POSITION_LIMIT, POSITION_LIMIT)
                }),
                width: CARD_WIDTH,
                height: CARD_HEIGHT,
            };
            lane_rows[lane] += 1;
            layout.cards.push(Card {
                id: row.id,
                bounds,
                label: row.label,
                caption,
                accepted: row.accepted,
                stale_acceptance: row.stale_acceptance,
                color,
            });
        }
        let positions: BTreeMap<_, _> = layout
            .cards
            .iter()
            .map(|card| (card.id, card.bounds))
            .collect();
        for relationship in document.relationships.values() {
            let Some(&card) = positions.get(&relationship.id) else {
                continue;
            };
            layout.connections.push(Connection {
                id: relationship.id,
                from: positions[&relationship.from],
                to: positions[&relationship.to],
                card,
                direction: relationship.direction.clone(),
            });
        }
        if let Some(first) = layout.cards.first() {
            let mut min_x = layout
                .cards
                .iter()
                .map(|card| card.bounds.x)
                .min()
                .unwrap_or(first.bounds.x);
            let min_y = layout
                .cards
                .iter()
                .map(|card| card.bounds.y)
                .min()
                .unwrap_or(first.bounds.y);
            let mut max_x = layout
                .cards
                .iter()
                .map(|card| card.bounds.x + card.bounds.width)
                .max()
                .unwrap_or(0);
            let max_y = layout
                .cards
                .iter()
                .map(|card| card.bounds.y + card.bounds.height)
                .max()
                .unwrap_or(0);
            for connection in &layout.connections {
                min_x = min_x.min(connection.card.x - 1);
                max_x = max_x
                    .max(connection.from.x + CARD_WIDTH + 1)
                    .max(connection.to.x + CARD_WIDTH + 1);
            }
            layout.bounds = Bounds {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            };
        }
        layout
    }

    /// IDs in the filtered logical diagram, including records outside the viewport.
    pub fn visible_ids(&self) -> Vec<Id> {
        self.cards.iter().map(|card| card.id).collect()
    }

    pub fn position(&self, id: Id) -> Option<Position> {
        self.cards
            .iter()
            .find(|card| card.id == id)
            .map(|card| Position {
                x: card.bounds.x,
                y: card.bounds.y,
            })
    }

    pub fn focus_navigation(&self, focus: Option<Id>, delta: isize) -> Option<Id> {
        if self.cards.is_empty() {
            return None;
        }
        let index = self
            .cards
            .iter()
            .position(|card| Some(card.id) == focus)
            .unwrap_or(0);
        Some(self.cards[index.saturating_add_signed(delta).min(self.cards.len() - 1)].id)
    }
}

fn short_title(title: &str) -> String {
    terminal_safe(title)
        .replace(['\n', '\t'], " ")
        .chars()
        .take(10)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagramState {
    pub pan_x: i32,
    pub pan_y: i32,
}

impl DiagramState {
    fn clamp(&mut self, layout: &DiagramLayout, area: Rect) {
        let inner = Block::bordered().inner(area);
        self.pan_x = self.pan_x.clamp(
            layout.bounds.x,
            (layout.bounds.x + layout.bounds.width - i32::from(inner.width)).max(layout.bounds.x),
        );
        self.pan_y = self.pan_y.clamp(
            layout.bounds.y,
            (layout.bounds.y + layout.bounds.height - i32::from(inner.height)).max(layout.bounds.y),
        );
    }

    pub fn pan(&mut self, dx: i32, dy: i32, layout: &DiagramLayout, area: Rect) {
        self.pan_x = self.pan_x.saturating_add(dx);
        self.pan_y = self.pan_y.saturating_add(dy);
        self.clamp(layout, area);
    }

    pub fn reveal(&mut self, id: Id, layout: &DiagramLayout, area: Rect) {
        let Some(card) = layout.cards.iter().find(|card| card.id == id) else {
            return;
        };
        let inner = Block::bordered().inner(area);
        if card.bounds.x < self.pan_x
            || card.bounds.x + card.bounds.width > self.pan_x + i32::from(inner.width)
        {
            self.pan_x = card.bounds.x;
        }
        if card.bounds.y < self.pan_y
            || card.bounds.y + card.bounds.height > self.pan_y + i32::from(inner.height)
        {
            self.pan_y = card.bounds.y;
        }
        self.clamp(layout, area);
    }

    pub fn hit_test(&self, x: u16, y: u16, area: Rect, layout: &DiagramLayout) -> Option<Id> {
        let inner = Block::bordered().inner(area);
        if !inner.contains((x, y).into()) {
            return None;
        }
        let mut state = self.clone();
        state.clamp(layout, area);
        let world_x = state.pan_x + i32::from(x - inner.x);
        let world_y = state.pan_y + i32::from(y - inner.y);
        // Later cards paint over earlier cards when explicit private positions overlap.
        layout
            .cards
            .iter()
            .rev()
            .find(|card| card.bounds.contains(world_x, world_y))
            .map(|card| card.id)
    }

    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        layout: &DiagramLayout,
        focus: Option<Id>,
        selected: &BTreeSet<Id>,
    ) {
        self.clamp(layout, area);
        let block = Block::bordered().title(format!(
            "Diagram · {} records · pan {},{}",
            layout.cards.len(),
            self.pan_x,
            self.pan_y
        ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.is_empty() {
            return;
        }
        let viewport = Bounds {
            x: self.pan_x,
            y: self.pan_y,
            width: i32::from(inner.width),
            height: i32::from(inner.height),
        };
        let buffer = frame.buffer_mut();
        for connection in &layout.connections {
            let style = if focus == Some(connection.id) || selected.contains(&connection.id) {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let start = (connection.from.x + CARD_WIDTH, connection.from.y + 1);
            let end = (connection.to.x + CARD_WIDTH, connection.to.y + 3);
            let incoming = (connection.card.x - 1, connection.card.y + 1);
            let outgoing = (connection.card.x - 1, connection.card.y + 3);
            route(buffer, inner, viewport, start, incoming, style);
            route(buffer, inner, viewport, end, outgoing, style);
            match connection.direction {
                Direction::Directed => {
                    put(buffer, inner, viewport, incoming.0, incoming.1, ">", style);
                    put(buffer, inner, viewport, end.0, end.1, "<", style);
                }
                Direction::Bidirectional => {
                    for point in [start, end] {
                        put(buffer, inner, viewport, point.0, point.1, "<", style);
                    }
                    for point in [incoming, outgoing] {
                        put(buffer, inner, viewport, point.0, point.1, ">", style);
                    }
                }
                Direction::Undirected => {}
            }
        }
        for card in &layout.cards {
            if card.bounds.x >= viewport.x + viewport.width
                || card.bounds.y >= viewport.y + viewport.height
                || card.bounds.x + CARD_WIDTH <= viewport.x
                || card.bounds.y + CARD_HEIGHT <= viewport.y
            {
                continue;
            }
            let mut style = Style::default().fg(card.color);
            if focus == Some(card.id) {
                style = style.bg(Color::DarkGray);
            }
            let acceptance = if card.stale_acceptance {
                "accepted stale"
            } else if card.accepted {
                "accepted"
            } else {
                ""
            };
            let text = format!(
                "{} {}\n{}\n{acceptance}",
                if selected.contains(&card.id) {
                    "[x]"
                } else {
                    "[ ]"
                },
                card.label,
                card.caption
            );
            let card_area = Rect::new(0, 0, CARD_WIDTH as u16, CARD_HEIGHT as u16);
            let mut card_buffer = Buffer::empty(card_area);
            Paragraph::new(text)
                .style(style)
                .block(Block::bordered())
                .render(card_area, &mut card_buffer);
            for y in 0..CARD_HEIGHT {
                for x in 0..CARD_WIDTH {
                    let world_x = card.bounds.x + x;
                    let world_y = card.bounds.y + y;
                    if !viewport.contains(world_x, world_y) {
                        continue;
                    }
                    let cell = &card_buffer[(x as u16, y as u16)];
                    let width = Line::raw(cell.symbol()).width() as i32;
                    if world_x + width > viewport.x + viewport.width {
                        continue;
                    }
                    buffer[(
                        (i32::from(inner.x) + world_x - viewport.x) as u16,
                        (i32::from(inner.y) + world_y - viewport.y) as u16,
                    )] = cell.clone();
                }
            }
        }
    }
}

fn put(
    buffer: &mut Buffer,
    area: Rect,
    viewport: Bounds,
    x: i32,
    y: i32,
    symbol: &str,
    style: Style,
) {
    if viewport.contains(x, y) {
        buffer[(
            (i32::from(area.x) + x - viewport.x) as u16,
            (i32::from(area.y) + y - viewport.y) as u16,
        )]
            .set_symbol(symbol)
            .set_style(style);
    }
}

/// Clip segments before walking their cells, even for distant private coordinates.
fn route(
    buffer: &mut Buffer,
    area: Rect,
    viewport: Bounds,
    start: (i32, i32),
    end: (i32, i32),
    style: Style,
) {
    let turn = start.0 + (end.0 - start.0) / 2;
    for (left, right, y) in [
        (start.0.min(turn), start.0.max(turn), start.1),
        (turn.min(end.0), turn.max(end.0), end.1),
    ] {
        if y >= viewport.y && y < viewport.y + viewport.height {
            for x in left.max(viewport.x)..=right.min(viewport.x + viewport.width - 1) {
                put(buffer, area, viewport, x, y, "─", style);
            }
        }
    }
    if turn >= viewport.x && turn < viewport.x + viewport.width {
        for y in start.1.min(end.1).max(viewport.y)
            ..=start.1.max(end.1).min(viewport.y + viewport.height - 1)
        {
            put(buffer, area, viewport, turn, y, "│", style);
        }
    }
}
