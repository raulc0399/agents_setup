use crate::model::{Document, Id, ViewState, terminal_safe};

/// Presentation shared by list navigation and selected-content details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: Id,
    pub label: String,
    pub details: String,
    pub accepted: bool,
    pub stale_acceptance: bool,
}

fn row(id: Id, label: String, details: String, revision: u64, accepted: Option<u64>) -> Row {
    Row {
        id,
        label: terminal_safe(&label).replace(['\n', '\t'], " "),
        details: terminal_safe(&details),
        accepted: accepted == Some(revision),
        stale_acceptance: accepted.is_some_and(|value| value != revision),
    }
}

pub fn rows(document: &Document, view: &ViewState) -> Vec<Row> {
    let elements = document
        .elements
        .values()
        .filter(|item| !item.archived)
        .map(|item| {
            row(
                item.id,
                format!("{:?}: {}", item.kind, item.title),
                format!(
                    "{}\n{}\n{}\nSource: {}",
                    item.title,
                    item.id,
                    item.body,
                    item.source.as_deref().unwrap_or("none")
                ),
                item.semantic_revision,
                item.accepted_revision,
            )
        });
    let relationships = document
        .relationships
        .values()
        .filter(|item| !item.archived)
        .map(|item| {
            let from = document
                .elements
                .get(&item.from)
                .map(|value| value.title.as_str())
                .unwrap_or("missing");
            let to = document
                .elements
                .get(&item.to)
                .map(|value| value.title.as_str())
                .unwrap_or("missing");
            row(
                item.id,
                format!("Link: {}", item.label),
                format!(
                    "{}\n{}\n{from} -> {to}\nDirection: {:?}\nFrom: {}\nTo: {}",
                    item.label, item.id, item.direction, item.from, item.to
                ),
                item.semantic_revision,
                item.accepted_revision,
            )
        });
    let groups = document
        .groups
        .values()
        .filter(|item| !item.archived)
        .map(|item| {
            let members = item
                .members
                .iter()
                .map(|id| {
                    let name = document
                        .elements
                        .get(id)
                        .map(|value| value.title.as_str())
                        .unwrap_or("missing");
                    format!("{name} ({id})")
                })
                .collect::<Vec<_>>()
                .join("\n");
            row(
                item.id,
                format!("Group: {}", item.title),
                format!("{}\n{}\nMembers:\n{members}", item.title, item.id),
                item.semantic_revision,
                item.accepted_revision,
            )
        });
    let filter = view.filter.to_lowercase();
    elements
        .chain(relationships)
        .chain(groups)
        .filter(|item| {
            !view.hidden.contains(&item.id)
                && (filter.is_empty()
                    || item.details.to_lowercase().contains(&filter)
                    || item.label.to_lowercase().contains(&filter))
        })
        .collect()
}
