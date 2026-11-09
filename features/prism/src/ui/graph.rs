//! Terminal half of the diagram: rasterizes the SVG from `prism_core::graph` and draws it
//! with an image protocol. The same geometry answers clicks.
use crate::model::{Document, Id, ViewState};
use crate::ui::list;
use prism_core::graph::LANE_PADDING;
pub use prism_core::graph::{Detail, GraphLayout, WorldRect};
use ratatui::buffer::CellDiffOption;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::num::NonZeroU16;
use std::sync::Arc;

pub const ZOOM_STEP: f32 = 1.5;
pub const MAX_ZOOM: f32 = 8.0;

/// Lays out what the list shows: archived, hidden and filtered-out content stays out.
pub fn visible_layout(document: &Document, view: &ViewState, detail: Detail) -> GraphLayout {
    let visible: BTreeSet<Id> = list::rows(document, view)
        .iter()
        .map(|row| row.id)
        .collect();
    GraphLayout::new(document, &visible, detail)
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

/// Rasterizes the world-unit SVG at `scale` with the top-left corner at `pan`, as PNG.
pub fn rasterize(
    svg: &str,
    width: u32,
    height: u32,
    scale: f32,
    pan: (f32, f32),
    fonts: &Arc<fontdb::Database>,
) -> Option<Vec<u8>> {
    render_pixmap(svg, width, height, scale, pan, fonts)?
        .encode_png()
        .ok()
}

fn render_pixmap(
    svg: &str,
    width: u32,
    height: u32,
    scale: f32,
    pan: (f32, f32),
    fonts: &Arc<fontdb::Database>,
) -> Option<tiny_skia::Pixmap> {
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
    Some(pixmap)
}

/// Cuts the raster into one PNG per terminal row, top to bottom.
fn row_strips(pixmap: &tiny_skia::Pixmap, rows: u16) -> Option<Vec<Vec<u8>>> {
    let rows = u32::from(rows.max(1));
    let strip_height = pixmap.height() / rows;
    let size = tiny_skia::IntSize::from_wh(pixmap.width(), strip_height)?;
    let strip_bytes = (pixmap.width() * strip_height * tiny_skia::BYTES_PER_PIXEL as u32) as usize;
    pixmap
        .data()
        .chunks_exact(strip_bytes)
        .take(rows as usize)
        .map(|data| {
            tiny_skia::Pixmap::from_vec(data.to_vec(), size)?
                .encode_png()
                .ok()
        })
        .collect()
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                let sextet = (group >> (18 - 6 * index)) & 0x3f;
                encoded.push(BASE64_ALPHABET[sextet as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

/// Where the image escape draws. A tmux passthrough reaches the terminal at its real cursor,
/// which follows the active pane rather than the pane that wrote it, so inside tmux the
/// escape carries the absolute screen cell (1-based) and restores the cursor afterwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    AtCursor,
    TmuxScreen { column: u16, row: u16 },
}

/// The pane's top-left cell on the terminal screen (0-based), from tmux's own layout.
fn tmux_pane_origin() -> Option<(u16, u16)> {
    let pane = std::env::var("TMUX_PANE").ok()?;
    let output = std::process::Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "-t",
            &pane,
            "#{pane_left} #{pane_top} #{status} #{status-position}",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    pane_origin(&String::from_utf8_lossy(&output.stdout))
}

/// A status line at the top pushes every pane down by its height.
fn pane_origin(fields: &str) -> Option<(u16, u16)> {
    let mut fields = fields.split_whitespace();
    let left: u16 = fields.next()?.parse().ok()?;
    let top: u16 = fields.next()?.parse().ok()?;
    let status = fields.next()?;
    let status_lines = match status {
        "off" => 0,
        "on" => 1,
        lines => lines.parse().ok()?,
    };
    let offset = if fields.next()? == "top" {
        status_lines
    } else {
        0
    };
    Some((left, top + offset))
}

/// iTerm2 inline images sized in cells and stretched to exactly their area, so placement and
/// click mapping never depend on the terminal's real font size.
///
/// The picture goes out as one image per terminal row. A taller image makes the terminal
/// feed lines while it draws, and a line feed at the bottom of whatever scroll region tmux
/// last set scrolls the screen. A one-row image with `doNotMoveCursor` moves nothing. Each
/// row is erased first so an earlier image cannot show through.
fn inline_image(strips: &[Vec<u8>], columns: u16, placement: Placement) -> String {
    let (start, escape, end) = match placement {
        Placement::TmuxScreen { .. } => ("\x1bPtmux;", "\x1b\x1b", "\x1b\\"),
        Placement::AtCursor => ("", "\x1b", ""),
    };
    let mut sequence = String::new();
    for (index, png) in strips.iter().enumerate() {
        sequence.push_str(start);
        let _ = write!(sequence, "{escape}7");
        match placement {
            Placement::TmuxScreen { column, row } => {
                let _ = write!(sequence, "{escape}[{};{column}H", usize::from(row) + index);
            }
            Placement::AtCursor if index > 0 => {
                let _ = write!(sequence, "{escape}[{index}B");
            }
            Placement::AtCursor => {}
        }
        let _ = write!(
            sequence,
            "{escape}[{columns}X{escape}]1337;File=inline=1;size={};width={columns};height=1;preserveAspectRatio=0;doNotMoveCursor=1:",
            png.len()
        );
        sequence.push_str(&base64(png));
        let _ = write!(sequence, "\x07{escape}8");
        sequence.push_str(end);
    }
    sequence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_short_groups() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn inline_image_sends_one_row_strips_that_never_move_the_cursor_down() {
        let strips = vec![b"png".to_vec(), b"png".to_vec(), b"png".to_vec()];
        let plain = inline_image(&strips, 78, Placement::AtCursor);
        assert_eq!(plain.matches("height=1;").count(), 3);
        assert_eq!(
            plain
                .matches("]1337;File=inline=1;size=3;width=78;height=1;preserveAspectRatio=0;doNotMoveCursor=1:cG5n\x07")
                .count(),
            3
        );
        // Every strip starts from the saved position and returns to it.
        assert!(plain.starts_with("\x1b7\x1b[78X"));
        assert!(plain.contains("\x1b7\x1b[2B\x1b[78X"));
        assert!(plain.ends_with("\x07\x1b8"));

        let wrapped = inline_image(&strips, 78, Placement::TmuxScreen { column: 99, row: 3 });
        // Each strip is its own passthrough with an absolute row and a restored cursor.
        assert_eq!(wrapped.matches("\x1bPtmux;").count(), 3);
        assert!(wrapped.starts_with("\x1bPtmux;\x1b\x1b7\x1b\x1b[3;99H\x1b\x1b[78X\x1b\x1b]1337;"));
        assert!(wrapped.contains("\x1b\x1b[5;99H"));
        assert!(wrapped.ends_with("\x07\x1b\x1b8\x1b\\"));
        assert!(!wrapped.contains("height=3"));
    }

    #[test]
    fn row_strips_cut_the_raster_into_one_png_per_row() {
        let pixmap = tiny_skia::Pixmap::new(20, 60).unwrap();
        let strips = row_strips(&pixmap, 3).unwrap();
        assert_eq!(strips.len(), 3);
        assert!(strips.iter().all(|png| png.starts_with(b"\x89PNG")));
    }

    #[test]
    fn pane_origin_accounts_for_a_top_status_line() {
        assert_eq!(pane_origin("97 0 on bottom"), Some((97, 0)));
        assert_eq!(pane_origin("97 0 on top"), Some((97, 1)));
        assert_eq!(pane_origin("0 5 2 top"), Some((0, 7)));
        assert_eq!(pane_origin("0 5 off top"), Some((0, 5)));
        assert_eq!(pane_origin("garbage"), None);
    }
}

/// Viewport state and image cache for one diagram pane. Zoom 1 fits the whole diagram.
pub struct GraphView {
    /// Inside tmux the image escape travels in a passthrough wrapper.
    tmux: bool,
    /// Pixels rasterized per terminal cell. The image is sized in cells, so this only sets
    /// resolution and aspect; it need not match the real font.
    cell: (u16, u16),
    /// The pane's screen position, read from tmux when the pane size changes or on repaint.
    origin: Option<(u16, u16)>,
    fonts: Arc<fontdb::Database>,
    pub zoom: f32,
    pub pan: (f32, f32),
    cells: (u16, u16),
    scale: f32,
    layout: GraphLayout,
    cache: Option<(CacheKey, String)>,
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
    origin: Option<(u16, u16)>,
    area: (u16, u16),
}

/// A tiny diagram is not blown up beyond this many pixels per world unit when fitting.
const MAX_FIT_SCALE: f32 = 1.5;
const WHEEL_ZOOM_STEP: f32 = 1.25;

impl GraphView {
    pub fn new(tmux: bool, cell: (u16, u16), fonts: Arc<fontdb::Database>) -> Self {
        Self {
            tmux,
            cell: (cell.0.max(1), cell.1.max(1)),
            origin: None,
            fonts,
            zoom: 1.0,
            pan: (0.0, 0.0),
            cells: (1, 1),
            scale: 1.0,
            layout: GraphLayout::default(),
            cache: None,
        }
    }

    pub fn layout(&self) -> &GraphLayout {
        &self.layout
    }

    pub fn wheel_zoom_step(&self) -> f32 {
        WHEEL_ZOOM_STEP
    }

    fn pixels(&self) -> (f32, f32) {
        (
            f32::from(self.cells.0) * f32::from(self.cell.0),
            f32::from(self.cells.1) * f32::from(self.cell.1),
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
        let cells = (cells.0.max(1), cells.1.max(1));
        if self.tmux && (self.origin.is_none() || cells != self.cells) {
            self.origin = tmux_pane_origin();
        }
        self.cells = cells;
        let mut chosen = visible_layout(document, view, Detail::Names);
        let mut fit = self.fit_scale(chosen.bounds);
        for detail in [Detail::Full, Detail::Subtitles] {
            let candidate = visible_layout(document, view, detail);
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
            chosen = visible_layout(document, view, detail);
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

    /// Forgets the cached image and pane position; the next frame reads both again.
    pub fn invalidate(&mut self) {
        self.cache = None;
        self.origin = None;
    }

    pub fn reset_zoom(&mut self) {
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
    }

    pub fn pan_by_cells(&mut self, dx: i32, dy: i32) {
        self.pan.0 += dx as f32 * f32::from(self.cell.0) / self.scale;
        self.pan.1 += dy as f32 * f32::from(self.cell.1) / self.scale;
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
        let x =
            (f32::from(column - inner.x) + 0.5) * f32::from(self.cell.0) / self.scale + self.pan.0;
        let y = (f32::from(row - inner.y) + 0.5) * f32::from(self.cell.1) / self.scale + self.pan.1;
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
            nodes: self.layout.node_count(),
            edges: self.layout.edge_count(),
            origin: self.origin,
            area: (inner.x, inner.y),
        };
        if self.cache.as_ref().is_none_or(|(cached, _)| *cached != key) {
            let (width, height) = self.pixels();
            let svg = self.layout.svg(focus, selected);
            let Some(strips) = render_pixmap(
                &svg,
                width as u32,
                height as u32,
                self.scale,
                self.pan,
                &self.fonts,
            )
            .and_then(|pixmap| row_strips(&pixmap, inner.height)) else {
                return;
            };
            let placement = match (self.tmux, self.origin) {
                (false, _) => Placement::AtCursor,
                (true, Some((left, top))) => Placement::TmuxScreen {
                    column: left + inner.x + 1,
                    row: top + inner.y + 1,
                },
                // Without the pane position the image would land in the active pane.
                (true, None) => return,
            };
            self.cache = Some((key, inline_image(&strips, inner.width, placement)));
        }
        let Some((_, sequence)) = &self.cache else {
            return;
        };
        // The first cell carries the whole escape; the rest are left alone so the diff
        // never prints over the image.
        let buffer = frame.buffer_mut();
        for y in inner.top()..inner.bottom() {
            for x in inner.left()..inner.right() {
                let cell = &mut buffer[(x, y)];
                if (x, y) == (inner.x, inner.y) {
                    cell.set_symbol(sequence)
                        .set_diff_option(CellDiffOption::ForcedWidth(NonZeroU16::MIN));
                } else {
                    cell.set_diff_option(CellDiffOption::Skip);
                }
            }
        }
    }

    /// Writes the whole diagram with full text as an SVG file the browser can open.
    pub fn export_svg(
        &self,
        document: &Document,
        view: &ViewState,
        directory: &std::path::Path,
    ) -> std::io::Result<std::path::PathBuf> {
        let layout = visible_layout(document, view, Detail::Full);
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
