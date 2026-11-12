use crate::model::*;
use prism_core::graph::GraphLayout;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid content: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("revision conflict; current revision is {current}")]
    Conflict { current: u64 },
    #[error("request ID was already used with a different payload")]
    RequestConflict,
    #[error("undo conflicts with subsequent changes")]
    UndoConflict,
    #[error("unsupported database schema version {0}")]
    SchemaVersion(i64),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementInput {
    pub id: Id,
    pub title: String,
    pub body: String,
    pub kind: ElementKind,
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationshipInput {
    pub id: Id,
    pub from: Id,
    pub to: Id,
    pub label: String,
    pub direction: Direction,
    #[serde(default)]
    pub manual: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInput {
    pub id: Id,
    pub title: String,
    pub members: BTreeSet<Id>,
}

/// Updates are explicit upserts; omitted records remain unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishRequest {
    pub request_id: Id,
    pub document_id: Option<Id>,
    pub expected_revision: Option<u64>,
    pub title: String,
    pub elements: Vec<ElementInput>,
    pub relationships: Vec<RelationshipInput>,
    pub groups: Vec<GroupInput>,
    /// What this publish does to the diagram's lane orientation (`Document::lanes`); absent
    /// (or null) keeps it. Left out when absent, so a retried request from before it hashes
    /// the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<LaneChoice>,
}

/// A publish's choice of lane orientation: `auto` chooses again (`automatic_lanes`) and stores
/// the result, `rows` and `columns` store themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneChoice {
    Auto,
    Rows,
    Columns,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishResult {
    pub document_id: Id,
    pub revision: u64,
    pub element_ids: Vec<Id>,
    pub relationship_ids: Vec<Id>,
    pub group_ids: Vec<Id>,
}

pub use crate::model::ContentAction;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationResult {
    pub revision: u64,
    pub undo_id: Id,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareQuestion {
    pub request_id: Id,
    pub generation: u64,
    pub selection: Selection,
    pub text: String,
    pub mode: DeliveryMode,
    /// Left out of the payload hash when `None`, so an ordinary ask hashes as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    /// As `intent`, left out of the hash when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted: Option<Id>,
}

pub(crate) fn bounded(text: &str, maximum: usize, field: &str) -> Result<()> {
    if text.len() > maximum {
        return Err(Error::Invalid(format!("{field} exceeds {maximum} bytes")));
    }
    Ok(())
}

pub(crate) fn serialized<T: Serialize>(value: &T) -> Result<String> {
    let json = serde_json::to_string(value)?;
    bounded(&json, MAX_PAYLOAD_BYTES, "payload")?;
    Ok(json)
}

pub(crate) fn check_revision(document: &Document, expected: u64) -> Result<()> {
    if document.revision != expected {
        return Err(Error::Conflict {
            current: document.revision,
        });
    }
    Ok(())
}

pub(crate) fn validate_document(document: &Document) -> Result<()> {
    bounded(&document.title, MAX_TITLE_BYTES, "document title")?;
    if document.elements.len() > MAX_ELEMENTS
        || document.relationships.len() > MAX_RELATIONSHIPS
        || document.groups.len() > MAX_GROUPS
    {
        return Err(Error::Invalid("document record limit exceeded".into()));
    }
    if document.bookmarks.len() > MAX_BOOKMARKS {
        return Err(Error::Invalid(format!(
            "a document holds at most {MAX_BOOKMARKS} bookmarks"
        )));
    }
    let mut ids = BTreeSet::new();
    for item in document.elements.values() {
        if !ids.insert(item.id) || item.id.is_nil() {
            return Err(Error::Invalid("duplicate or nil ID".into()));
        }
        bounded(&item.title, MAX_TITLE_BYTES, "element title")?;
        bounded(&item.body, MAX_TEXT_BYTES, "element body")?;
        if let Some(source) = &item.source {
            bounded(source, MAX_TEXT_BYTES, "source")?;
        }
    }
    for edge in document.relationships.values() {
        if !ids.insert(edge.id) || edge.id.is_nil() {
            return Err(Error::Invalid("duplicate or nil ID".into()));
        }
        bounded(&edge.label, MAX_TITLE_BYTES, "relationship label")?;
        for endpoint in [edge.from, edge.to] {
            let element = document
                .elements
                .get(&endpoint)
                .ok_or_else(|| Error::Invalid("relationship endpoint missing".into()))?;
            if !edge.archived && element.archived {
                return Err(Error::Invalid(
                    "relationship references archived element".into(),
                ));
            }
        }
    }
    for group in document.groups.values() {
        if !ids.insert(group.id) || group.id.is_nil() {
            return Err(Error::Invalid("duplicate or nil ID".into()));
        }
        bounded(&group.title, MAX_TITLE_BYTES, "group title")?;
        for member in &group.members {
            if !document.elements.contains_key(member) {
                return Err(Error::Invalid("group member missing".into()));
            }
        }
    }
    serialized(document)?;
    Ok(())
}

pub(crate) fn publish_into(mut document: Document, request: &PublishRequest) -> Result<Document> {
    let revision = document.revision + 1;
    let mut seen = BTreeSet::new();
    for id in request
        .elements
        .iter()
        .map(|v| v.id)
        .chain(request.relationships.iter().map(|v| v.id))
        .chain(request.groups.iter().map(|v| v.id))
    {
        if !seen.insert(id) {
            return Err(Error::Invalid("duplicate input ID".into()));
        }
    }
    document.title = request.title.clone();
    // Every record keeps the position it was first published at, so a publish never
    // renumbers the list. Relationships and groups stored before they had one get the
    // position they are shown at, so what this publish adds goes after them: new elements,
    // then new relationships, then new groups, each in request order.
    let positions = document.positions();
    for item in document.relationships.values_mut() {
        item.order = Some(positions[&item.id]);
    }
    for item in document.groups.values_mut() {
        item.order = Some(positions[&item.id]);
    }
    let mut sequence = document.sequence();
    for input in &request.elements {
        let previous = document.elements.get(&input.id);
        reject_archived(previous.is_some_and(|item| item.archived))?;
        let order = previous.map_or_else(|| sequence.take(), |item| item.order);
        let mut item = Element {
            id: input.id,
            title: input.title.clone(),
            body: input.body.clone(),
            kind: input.kind.clone(),
            source: input.source.clone(),
            order,
            created_revision: previous.map_or(Some(revision), |item| item.created_revision),
            semantic_revision: revision,
            accepted_revision: previous.and_then(|item| item.accepted_revision),
            archived: false,
        };
        if let Some(previous) = previous {
            item.semantic_revision = previous.semantic_revision;
            if item != *previous {
                item.semantic_revision = revision;
            }
        }
        document.elements.insert(item.id, item);
    }
    for input in &request.relationships {
        let previous = document.relationships.get(&input.id);
        reject_archived(previous.is_some_and(|item| item.archived))?;
        let mut item = Relationship {
            id: input.id,
            from: input.from,
            to: input.to,
            label: input.label.clone(),
            direction: input.direction.clone(),
            manual: input.manual,
            order: Some(
                previous
                    .and_then(|item| item.order)
                    .unwrap_or_else(|| sequence.take()),
            ),
            created_revision: previous.map_or(Some(revision), |item| item.created_revision),
            semantic_revision: revision,
            accepted_revision: previous.and_then(|item| item.accepted_revision),
            archived: false,
        };
        if let Some(previous) = previous {
            item.semantic_revision = previous.semantic_revision;
            if item != *previous {
                item.semantic_revision = revision;
            }
        }
        document.relationships.insert(item.id, item);
    }
    for input in &request.groups {
        let previous = document.groups.get(&input.id);
        reject_archived(previous.is_some_and(|item| item.archived))?;
        for id in &input.members {
            if document.elements.get(id).is_none_or(|item| item.archived) {
                return Err(Error::Invalid(
                    "group references missing or archived element".into(),
                ));
            }
        }
        let mut item = Group {
            id: input.id,
            title: input.title.clone(),
            members: input.members.clone(),
            order: Some(
                previous
                    .and_then(|item| item.order)
                    .unwrap_or_else(|| sequence.take()),
            ),
            created_revision: previous.map_or(Some(revision), |item| item.created_revision),
            semantic_revision: revision,
            accepted_revision: previous.and_then(|item| item.accepted_revision),
            archived: false,
        };
        if let Some(previous) = previous {
            item.semantic_revision = previous.semantic_revision;
            if item != *previous {
                item.semantic_revision = revision;
            }
        }
        document.groups.insert(item.id, item);
    }
    document.revision = revision;
    validate_document(&document)?;
    // Sticky, so the diagram does not flip as it grows: a document without an orientation
    // gets the automatic one at its first publication with two lanes, and keeps it.
    document.lanes = match request.lanes {
        Some(LaneChoice::Rows) => Some(Lanes::Rows),
        Some(LaneChoice::Columns) => Some(Lanes::Columns),
        Some(LaneChoice::Auto) => GraphLayout::automatic_lanes(&document),
        None => document
            .lanes
            .or_else(|| GraphLayout::automatic_lanes(&document)),
    };
    Ok(document)
}

fn reject_archived(archived: bool) -> Result<()> {
    if archived {
        return Err(Error::Invalid(
            "archived content cannot be republished; use undo".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_selection(document: &Document, selection: &Selection) -> Result<()> {
    if selection.document_id != document.id {
        return Err(Error::Invalid(
            "selection belongs to another document".into(),
        ));
    }
    check_revision(document, selection.revision)?;
    validate_ids(document, &selection.ids)
}

fn validate_ids(document: &Document, ids: &[Id]) -> Result<()> {
    validate_unique(ids)?;
    for id in ids {
        if active(document, id) != Some(true) {
            return Err(Error::Invalid(
                "selection contains missing or archived content".into(),
            ));
        }
    }
    Ok(())
}

fn validate_unique(ids: &[Id]) -> Result<()> {
    if ids.is_empty() || ids.len() > MAX_SELECTION {
        return Err(Error::Invalid(
            "selection must contain bounded content IDs".into(),
        ));
    }
    let mut unique = BTreeSet::new();
    for id in ids {
        if !unique.insert(id) {
            return Err(Error::Invalid("duplicate selection ID".into()));
        }
    }
    Ok(())
}

/// Whether the element, relationship or group `id` is not archived; `None` when there is none.
fn active(document: &Document, id: &Id) -> Option<bool> {
    document
        .elements
        .get(id)
        .map(|v| !v.archived)
        .or_else(|| document.relationships.get(id).map(|v| !v.archived))
        .or_else(|| document.groups.get(id).map(|v| !v.archived))
}

/// The closed state a document action sets; `None` for the item actions.
pub(crate) fn closed_by(action: &ContentAction) -> Option<bool> {
    match action {
        ContentAction::Close {} => Some(true),
        ContentAction::Reopen {} => Some(false),
        ContentAction::Accept { .. }
        | ContentAction::Archive { .. }
        | ContentAction::Bookmark { .. }
        | ContentAction::Unbookmark { .. } => None,
    }
}

/// Applies `action` to `document`; bookmarks it adds take their positions from `bookmarks`.
pub(crate) fn apply_action(
    document: &mut Document,
    action: &ContentAction,
    bookmarks: Sequence,
) -> Result<()> {
    let (ids, archive) = match action {
        ContentAction::Accept { ids } => (ids, false),
        ContentAction::Archive { ids } => (ids, true),
        // No item changes; the store keeps the closed state beside the document.
        ContentAction::Close {} | ContentAction::Reopen {} => {
            document.revision += 1;
            return Ok(());
        }
        // Content state like acceptance: a new revision, so every view and the agent see it.
        ContentAction::Bookmark { ids, quote } => {
            add_bookmarks(document, ids, quote.as_deref(), bookmarks)?;
            document.revision += 1;
            return validate_document(document);
        }
        ContentAction::Unbookmark { ids } => {
            remove_bookmarks(document, ids)?;
            document.revision += 1;
            return validate_document(document);
        }
    };
    validate_ids(document, ids)?;
    for id in ids {
        if let Some(item) = document.elements.get_mut(id) {
            if archive {
                item.archived = true;
            } else {
                item.accepted_revision = Some(item.semantic_revision);
            }
        }
        if let Some(item) = document.relationships.get_mut(id) {
            if archive {
                item.archived = true;
            } else {
                item.accepted_revision = Some(item.semantic_revision);
            }
        }
        if let Some(item) = document.groups.get_mut(id) {
            if archive {
                item.archived = true;
            } else {
                item.accepted_revision = Some(item.semantic_revision);
            }
        }
    }
    if archive {
        for edge in document.relationships.values_mut() {
            if document.elements[&edge.from].archived || document.elements[&edge.to].archived {
                edge.archived = true;
            }
        }
    }
    document.revision += 1;
    validate_document(document)
}

/// Bookmarks each item of `ids` as a whole or, with `quote`, the text selected in the one item
/// of `ids`. Dropped items can be bookmarked; a bookmark that exists already is refused.
fn add_bookmarks(
    document: &mut Document,
    ids: &[Id],
    quote: Option<&str>,
    mut sequence: Sequence,
) -> Result<()> {
    validate_unique(ids)?;
    if let Some(quote) = quote {
        bounded(quote, MAX_QUOTE_BYTES, "bookmark quote")?;
        if ids.len() != 1 || quote.trim().is_empty() {
            return Err(Error::Invalid(
                "a bookmark with a quote marks one item and needs the selected text".into(),
            ));
        }
    }
    for id in ids {
        if active(document, id).is_none() {
            return Err(Error::Invalid("bookmark of missing content".into()));
        }
        // The same text, however its whitespace was selected.
        let taken = document.bookmarks.values().any(|bookmark| {
            bookmark.item == *id && bookmark.quote.as_deref().map(collapsed) == quote.map(collapsed)
        });
        if taken {
            let what = if quote.is_some() { "text" } else { "item" };
            return Err(Error::Invalid(format!("{what} is already bookmarked")));
        }
        let bookmark = Bookmark {
            id: Id::new_v4(),
            item: *id,
            quote: quote.map(str::to_owned),
            order: sequence.take(),
        };
        document.bookmarks.insert(bookmark.id, bookmark);
    }
    Ok(())
}

/// Text with every run of whitespace as one space, trimmed; the app compares quotes so too.
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Removes the bookmark of each ID of `ids`, or the bookmark on the whole item of that ID.
fn remove_bookmarks(document: &mut Document, ids: &[Id]) -> Result<()> {
    validate_unique(ids)?;
    for id in ids {
        let bookmark = if document.bookmarks.contains_key(id) {
            Some(*id)
        } else {
            document
                .bookmarks
                .values()
                .find(|bookmark| bookmark.item == *id && bookmark.quote.is_none())
                .map(|bookmark| bookmark.id)
        };
        let bookmark = bookmark.ok_or_else(|| {
            Error::Invalid("no bookmark with this ID, nor on the whole item".into())
        })?;
        document.bookmarks.remove(&bookmark);
    }
    Ok(())
}

pub(crate) fn snapshot(document: &Document, selection: &Selection) -> Result<SelectionSnapshot> {
    validate_selection(document, selection)?;
    let mut element_ids: BTreeSet<Id> = selection.ids.iter().copied().collect();
    let mut relationships = BTreeMap::new();
    let mut groups = BTreeMap::new();
    for id in &selection.ids {
        if let Some(edge) = document.relationships.get(id) {
            element_ids.extend([edge.from, edge.to]);
            relationships.insert(*id, edge.clone());
        }
        if let Some(group) = document.groups.get(id) {
            element_ids.extend(group.members.iter().copied());
            groups.insert(*id, group.clone());
        }
    }
    let elements = element_ids
        .into_iter()
        .filter_map(|id| {
            document
                .elements
                .get(&id)
                .filter(|v| !v.archived)
                .map(|v| (id, v.clone()))
        })
        .collect();
    Ok(SelectionSnapshot {
        selection: selection.clone(),
        document_title: document.title.clone(),
        elements,
        relationships,
        groups,
    })
}
