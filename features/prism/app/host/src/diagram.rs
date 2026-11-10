//! The diagram of one document as a tab shows it. `prism_core::graph` lays it out with names
//! and subtitles and writes the SVG; the UI only places, themes and zooms it.
use prism_core::graph::{Detail, GraphLayout};
use prism_core::model::{Document, Id};
use serde::Serialize;
use std::collections::BTreeSet;

pub const SVG_EXTENSION: &str = "svg";
/// Longest file name the export suggests, without the extension.
const FILE_STEM_CHARS: usize = 48;
const DEFAULT_FILE_STEM: &str = "diagram";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diagram {
    pub svg: String,
    pub width: f32,
    pub height: f32,
    /// Boxes in lane order, each followed by its badges, then arrows: the order J and K
    /// follow. Lanes are not in it; the SVG's Tab order puts them first.
    pub order: Vec<Id>,
}

/// Lays out what the tab lists: `visible` leaves out hidden and filtered-out items; dropped
/// (archived) ones stay in their place, drawn faded.
pub fn diagram(document: &Document, visible: &BTreeSet<Id>) -> Diagram {
    let layout = GraphLayout::new(document, visible, Detail::Subtitles);
    Diagram {
        svg: layout.svg(None, &BTreeSet::new()),
        width: layout.bounds.width,
        height: layout.bounds.height,
        order: layout.ids(),
    }
}

/// `<document title>.svg` in lower case, with dashes for anything but letters and digits
/// (accented ones included).
pub fn export_file_name(title: &str) -> String {
    let slug: String = title
        .chars()
        .flat_map(|character| {
            let kept = if character.is_alphanumeric() {
                character
            } else {
                '-'
            };
            kept.to_lowercase()
        })
        .collect();
    let stem: String = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(FILE_STEM_CHARS)
        .collect();
    let stem = stem.trim_end_matches('-');
    let stem = if stem.is_empty() {
        DEFAULT_FILE_STEM
    } else {
        stem
    };
    format!("{stem}.{SVG_EXTENSION}")
}
