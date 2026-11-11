//! The diagram the app shows for a document and the file name its export suggests.
mod common;

use common::document;
use prism_app_host::diagram::{Diagram, Layout, Size, diagram, export_file_name};
use prism_core::graph::{Detail, GraphLayout};
use prism_core::model::{Direction, Document, Id, Relationship};
use std::collections::{BTreeMap, BTreeSet};

/// The layout of `diagram` at `detail`.
fn laid_out(document: &Document, visible: &BTreeSet<Id>, detail: Detail) -> (Diagram, Layout) {
    let diagram = diagram(document, visible, Some(detail));
    let layout = diagram.layout.clone().expect("a detail was asked for");
    (diagram, layout)
}

#[test]
fn the_diagram_shows_named_boxes_with_subtitles_in_keyboard_order() {
    let document = document(1, 1, &["Collect", "Store"]);
    let visible: BTreeSet<Id> = document.elements.keys().copied().collect();
    let (diagram, layout) = laid_out(&document, &visible, Detail::Subtitles);
    assert_eq!(layout.order, vec![Id::from_u128(100), Id::from_u128(101)]);
    assert!(layout.svg.contains(">Collect<") && layout.svg.contains(">Collect body<"));
    let size = diagram.sizes[&Detail::Subtitles];
    assert!(size.width > 0.0 && size.height > 0.0);
}

#[test]
fn the_diagram_is_laid_out_at_the_detail_asked_for_with_the_size_of_every_detail() {
    let mut document = document(1, 1, &["Collect", "Store"]);
    document.elements.get_mut(&Id::from_u128(100)).unwrap().body = "word ".repeat(30);
    let visible: BTreeSet<Id> = document.elements.keys().copied().collect();
    let (sized, overview) = laid_out(&document, &visible, Detail::Names);
    let (_, full) = laid_out(&document, &visible, Detail::Full);
    assert_eq!(
        (overview.detail, full.detail),
        (Detail::Names, Detail::Full)
    );
    assert!(
        !overview.svg.contains(">word"),
        "the overview shows titles only"
    );
    assert_eq!(
        full.svg.matches("word").count(),
        30,
        "the full detail shows the whole text"
    );
    assert_eq!(overview.order, full.order, "J and K keep their order");
    let sizes: Vec<Size> = Detail::ALL
        .iter()
        .map(|detail| sized.sizes[detail])
        .collect();
    for (detail, size) in Detail::ALL.iter().zip(&sizes) {
        let bounds = GraphLayout::new(&document, &visible, *detail).bounds;
        assert_eq!(
            (size.width, size.height),
            (bounds.width, bounds.height),
            "{detail:?}"
        );
    }
    assert!(
        sizes
            .windows(2)
            .all(|pair| pair[0].width < pair[1].width && pair[0].height < pair[1].height)
    );
    let json = serde_json::to_value(diagram(&document, &visible, Some(Detail::Full))).unwrap();
    assert_eq!(json["layout"]["detail"], "full");
    assert!(
        json["sizes"]["names"]["width"].is_number()
            && json["sizes"]["subtitles"]["height"].is_number()
    );
}

#[test]
fn without_a_detail_only_the_sizes_are_given() {
    let document = document(1, 1, &["Collect", "Store"]);
    let visible: BTreeSet<Id> = document.elements.keys().copied().collect();
    let sizes = diagram(&document, &visible, None);
    assert_eq!(sizes.layout, None);
    assert_eq!(
        sizes.sizes,
        diagram(&document, &visible, Some(Detail::Full)).sizes
    );
    assert_eq!(
        serde_json::to_value(&sizes).unwrap()["layout"],
        serde_json::Value::Null
    );
}

#[test]
fn the_arrows_steps_come_with_the_sizes_alone_and_at_every_detail() {
    let mut document = document(1, 1, &["Collect", "Store"]);
    let arrow = Id::from_u128(150);
    let relationship = Relationship {
        id: arrow,
        from: Id::from_u128(100),
        to: Id::from_u128(101),
        label: "hands over".into(),
        direction: Direction::Directed,
        manual: false,
        order: None,
        semantic_revision: 1,
        accepted_revision: None,
        archived: false,
    };
    document.relationships.insert(arrow, relationship);
    let visible: BTreeSet<Id> = [100, 101, 150].into_iter().map(Id::from_u128).collect();
    let sizes = diagram(&document, &visible, None);
    assert_eq!(sizes.steps, BTreeMap::from([(arrow, 1)]));
    for detail in Detail::ALL {
        assert_eq!(
            diagram(&document, &visible, Some(detail)).steps,
            sizes.steps
        );
    }
    assert_eq!(
        serde_json::to_value(&sizes).unwrap()["steps"][arrow.to_string()],
        1
    );
    let without_arrow = BTreeSet::from([Id::from_u128(100), Id::from_u128(101)]);
    assert!(diagram(&document, &without_arrow, None).steps.is_empty());
}

#[test]
fn items_the_tab_does_not_list_stay_out_of_the_diagram() {
    let document = document(1, 1, &["Collect", "Store"]);
    let visible = BTreeSet::from([Id::from_u128(101)]);
    let (_, layout) = laid_out(&document, &visible, Detail::Subtitles);
    assert_eq!(layout.order, vec![Id::from_u128(101)]);
    assert!(!layout.svg.contains(&Id::from_u128(100).to_string()));
    assert!(!layout.svg.contains("Collect"));
}

#[test]
fn nothing_visible_gives_an_empty_diagram() {
    let document = document(1, 1, &["Collect"]);
    let (diagram, layout) = laid_out(&document, &BTreeSet::new(), Detail::Subtitles);
    assert!(layout.order.is_empty());
    assert!(
        diagram
            .sizes
            .values()
            .all(|size| (size.width, size.height) == (0.0, 0.0))
    );
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
