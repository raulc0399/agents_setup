//! The diagram the app shows for a document and the file name its export suggests.
mod common;

use common::document;
use prism_app_host::diagram::{diagram, export_file_name};
use prism_core::model::Id;
use std::collections::BTreeSet;

#[test]
fn the_diagram_shows_named_boxes_with_subtitles_in_keyboard_order() {
    let document = document(1, 1, &["Collect", "Store"]);
    let visible: BTreeSet<Id> = document.elements.keys().copied().collect();
    let diagram = diagram(&document, &visible);
    assert_eq!(diagram.order, vec![Id::from_u128(100), Id::from_u128(101)]);
    assert!(diagram.svg.contains(">Collect<") && diagram.svg.contains(">Collect body<"));
    assert!(diagram.width > 0.0 && diagram.height > 0.0);
}

#[test]
fn items_the_tab_does_not_list_stay_out_of_the_diagram() {
    let document = document(1, 1, &["Collect", "Store"]);
    let visible = BTreeSet::from([Id::from_u128(101)]);
    let diagram = diagram(&document, &visible);
    assert_eq!(diagram.order, vec![Id::from_u128(101)]);
    assert!(!diagram.svg.contains(&Id::from_u128(100).to_string()));
    assert!(!diagram.svg.contains("Collect"));
}

#[test]
fn nothing_visible_gives_an_empty_diagram() {
    let document = document(1, 1, &["Collect"]);
    let diagram = diagram(&document, &BTreeSet::new());
    assert!(diagram.order.is_empty());
    assert_eq!((diagram.width, diagram.height), (0.0, 0.0));
}

#[test]
fn the_export_is_named_after_the_document() {
    assert_eq!(
        export_file_name("App plan: Batch 5!"),
        "app-plan-batch-5.svg"
    );
    assert_eq!(export_file_name("  ***  "), "diagram.svg");
    assert_eq!(export_file_name("Über Größe"), "über-größe.svg");
    assert_eq!(export_file_name(&"x".repeat(80)).len(), 48 + ".svg".len());
}
