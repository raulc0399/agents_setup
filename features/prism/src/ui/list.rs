use crate::model::{Direction, Document, ElementKind, Id, ViewState, terminal_safe};
use crate::ui::theme;
use ratatui::style::Color;

/// Presentation shared by list navigation, diagram cards and selected-content details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: Id,
    pub kind: &'static str,
    pub color: Color,
    pub title: String,
    pub label: String,
    /// One-line summary shown under the title.
    pub excerpt: String,
    /// Full text without the title; rendering adds title, kind and identifiers.
    pub details: String,
    pub accepted: bool,
    pub stale_acceptance: bool,
    /// Revised by the latest publication while older content exists in the document.
    pub changed: bool,
}

struct Record {
    id: Id,
    kind: &'static str,
    color: Color,
    title: String,
    excerpt: String,
    details: String,
    revision: u64,
    accepted: Option<u64>,
}

pub fn single_line(text: &str) -> String {
    terminal_safe(text).replace(['\n', '\t'], " ")
}

fn kind_label(kind: &ElementKind) -> (&'static str, Color) {
    match kind {
        ElementKind::Text => ("Text", theme::TEXT),
        ElementKind::Step => ("Step", theme::STEP),
        ElementKind::Finding => ("Finding", theme::FINDING),
        ElementKind::Question => ("Question", theme::QUESTION),
    }
}

fn direction_label(direction: &Direction) -> &'static str {
    match direction {
        Direction::Directed => "directed",
        Direction::Undirected => "undirected",
        Direction::Bidirectional => "bidirectional",
    }
}

fn records(document: &Document) -> Vec<Record> {
    let title_of = |id: &Id| {
        document
            .elements
            .get(id)
            .map(|value| value.title.as_str())
            .unwrap_or("missing")
    };
    let mut ordered: Vec<_> = document
        .elements
        .values()
        .filter(|item| !item.archived)
        .collect();
    ordered.sort_by_key(|item| (item.order, item.id));
    let elements = ordered.into_iter().map(|item| {
        let (kind, color) = kind_label(&item.kind);
        let mut details = item.body.clone();
        if let Some(source) = &item.source {
            details.push_str(&format!("\nSource: {source}"));
        }
        Record {
            id: item.id,
            kind,
            color,
            title: item.title.clone(),
            excerpt: item
                .body
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_owned(),
            details,
            revision: item.semantic_revision,
            accepted: item.accepted_revision,
        }
    });
    let relationships = document
        .relationships
        .values()
        .filter(|item| !item.archived)
        .map(|item| {
            let endpoints = format!("{} -> {}", title_of(&item.from), title_of(&item.to));
            Record {
                id: item.id,
                kind: "Link",
                color: theme::LINK,
                title: item.label.clone(),
                excerpt: endpoints.clone(),
                details: format!(
                    "{endpoints}\nDirection: {}",
                    direction_label(&item.direction)
                ),
                revision: item.semantic_revision,
                accepted: item.accepted_revision,
            }
        });
    let groups = document
        .groups
        .values()
        .filter(|item| !item.archived)
        .map(|item| {
            let members = item
                .members
                .iter()
                .map(|id| format!("- {}", title_of(id)))
                .collect::<Vec<_>>()
                .join("\n");
            Record {
                id: item.id,
                kind: "Group",
                color: theme::GROUP,
                title: item.title.clone(),
                excerpt: format!("{} members", item.members.len()),
                details: format!("Members:\n{members}"),
                revision: item.semantic_revision,
                accepted: item.accepted_revision,
            }
        });
    elements.chain(relationships).chain(groups).collect()
}

pub fn rows(document: &Document, view: &ViewState) -> Vec<Row> {
    let records = records(document);
    let latest = records.iter().map(|record| record.revision).max();
    let earliest = records.iter().map(|record| record.revision).min();
    let has_older_content = latest > earliest;
    let filter = view.filter.to_lowercase();
    records
        .into_iter()
        .map(|record| Row {
            id: record.id,
            kind: record.kind,
            color: record.color,
            title: single_line(&record.title),
            label: format!("{}: {}", record.kind, single_line(&record.title)),
            excerpt: single_line(&record.excerpt),
            details: terminal_safe(&record.details),
            accepted: record.accepted == Some(record.revision),
            stale_acceptance: record
                .accepted
                .is_some_and(|value| value != record.revision),
            changed: has_older_content
                && Some(record.revision) == latest
                && record.accepted != Some(record.revision),
        })
        .filter(|row| {
            !view.hidden.contains(&row.id)
                && (filter.is_empty()
                    || row.label.to_lowercase().contains(&filter)
                    || row.details.to_lowercase().contains(&filter)
                    || row.id.to_string().contains(&filter))
        })
        .collect()
}
