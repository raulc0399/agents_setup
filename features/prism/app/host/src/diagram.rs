//! The diagram of one document as a tab shows it. `prism_core::graph` lays it out at the
//! detail the UI's zoom asks for and writes the SVG; the UI only places, themes and zooms it.
use prism_core::graph::{Detail, GraphLayout};
use prism_core::model::{Document, Id};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub const SVG_EXTENSION: &str = "svg";
/// Longest file name the export suggests, without the extension.
const FILE_STEM_CHARS: usize = 48;
const DEFAULT_FILE_STEM: &str = "diagram";

/// What the shell's `diagram_layout` returns: the size of the whole diagram at every detail,
/// from placing its boxes alone, the step number of every arrow, and the layout of the
/// detail asked for. The UI's fit picks the most detailed one that fits its canvas from the
/// sizes before it asks for a layout; the list shows the steps without a layout.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diagram {
    pub sizes: BTreeMap<Detail, Size>,
    /// The number each arrow's pill shows, by relationship ID, the same at every detail;
    /// relationships drawn as badges have none.
    pub steps: BTreeMap<Id, usize>,
    /// None when no detail was asked for.
    pub layout: Option<Layout>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// The diagram laid out at one detail; its size is in `Diagram::sizes`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layout {
    pub detail: Detail,
    pub svg: String,
    /// Boxes in lane order, each followed by its badges, then arrows: the order J and K
    /// follow. Lanes are not in it; the SVG's Tab order puts them first.
    pub order: Vec<Id>,
}

/// What the tab lists, sized at every detail and laid out at `detail`: `visible` leaves out
/// hidden and filtered-out items; dropped (archived) ones stay in their place, drawn faded.
pub fn diagram(document: &Document, visible: &BTreeSet<Id>, detail: Option<Detail>) -> Diagram {
    // The same at every detail: any placement gives them.
    let mut steps = BTreeMap::new();
    let sizes = Detail::ALL
        .into_iter()
        .map(|level| {
            let (bounds, level_steps) = GraphLayout::bounds_and_steps(document, visible, level);
            steps = level_steps;
            let size = Size {
                width: bounds.width,
                height: bounds.height,
            };
            (level, size)
        })
        .collect();
    let layout = detail.map(|detail| {
        let layout = GraphLayout::new(document, visible, detail);
        Layout {
            detail,
            svg: layout.svg(None, &BTreeSet::new()),
            order: layout.ids(),
        }
    });
    Diagram {
        sizes,
        steps,
        layout,
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
