use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub type Id = Uuid;
pub const MAX_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_TITLE_BYTES: usize = 512;
pub const MAX_ELEMENTS: usize = 2_000;
pub const MAX_RELATIONSHIPS: usize = 4_000;
pub const MAX_GROUPS: usize = 500;
pub const MAX_SELECTION: usize = MAX_ELEMENTS + MAX_RELATIONSHIPS + MAX_GROUPS;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Codex,
    Claude,
}

impl AgentKind {
    pub fn label(&self) -> &'static str {
        match self {
            AgentKind::Codex => "Codex",
            AgentKind::Claude => "Claude",
        }
    }

    pub fn cli_name(&self) -> &'static str {
        match self {
            AgentKind::Codex => "codex",
            AgentKind::Claude => "claude",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceKey {
    pub project: String,
    pub agent: AgentKind,
    pub conversation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Id,
    pub key: WorkspaceKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Text,
    Step,
    Finding,
    Question,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Directed,
    Undirected,
    Bidirectional,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    pub id: Id,
    pub title: String,
    pub body: String,
    pub kind: ElementKind,
    pub source: Option<String>,
    /// Position in the document's publication sequence (see `Document::positions`), given at
    /// the element's first publication and kept by every later one.
    #[serde(default)]
    pub order: u64,
    /// Revision that first published it, kept by every later change; `None` when stored before
    /// records had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_revision: Option<u64>,
    pub semantic_revision: u64,
    pub accepted_revision: Option<u64>,
    pub archived: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relationship {
    pub id: Id,
    pub from: Id,
    pub to: Id,
    pub label: String,
    pub direction: Direction,
    /// Diagrams draw the arrow dashed: use it for manual, review, asynchronous or optional flows.
    #[serde(default)]
    pub manual: bool,
    /// Position in the publication sequence, as `Element::order`; `None` when stored before
    /// relationships had one (see `Document::positions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u64>,
    /// Revision that first published it, kept by every later change; `None` when stored before
    /// records had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_revision: Option<u64>,
    pub semantic_revision: u64,
    pub accepted_revision: Option<u64>,
    pub archived: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: Id,
    pub title: String,
    pub members: BTreeSet<Id>,
    /// Position in the publication sequence, as `Element::order`; `None` when stored before
    /// groups had one (see `Document::positions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u64>,
    /// Revision that first published it, kept by every later change; `None` when stored before
    /// records had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_revision: Option<u64>,
    pub semantic_revision: u64,
    pub accepted_revision: Option<u64>,
    pub archived: bool,
}

/// How a diagram lays out its lanes: as rows stacked top to bottom, or as columns side by side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lanes {
    #[default]
    Rows,
    Columns,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: Id,
    pub title: String,
    pub revision: u64,
    pub elements: BTreeMap<Id, Element>,
    pub relationships: BTreeMap<Id, Relationship>,
    pub groups: BTreeMap<Id, Group>,
    /// The diagram's lane orientation: the agent's choice, or the automatic one the bridge
    /// stored at the first publication with two lanes (see `graph::GraphLayout::orientation`);
    /// `None` before that and in documents stored before it. Optional and left out when
    /// `None`, so such documents load and older peers read documents. An older bridge drops
    /// it on publish, and an older store loses it when it rewrites the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lanes: Option<Lanes>,
}

impl Document {
    /// Every record's position in the document's one publication sequence: elements, then
    /// relationships, then groups of a first publication, each in request order; whatever a
    /// later publication adds goes after the highest position. A relationship or group stored
    /// before it had a position gets one after the highest stored position, relationships
    /// before groups, each by ID, so such a document keeps the numbering it always showed.
    pub fn positions(&self) -> BTreeMap<Id, u64> {
        let stored = self
            .elements
            .values()
            .map(|item| Some(item.order))
            .chain(self.relationships.values().map(|item| item.order))
            .chain(self.groups.values().map(|item| item.order))
            .flatten()
            .max();
        let mut sequence = Sequence::after(stored);

        let mut positions: BTreeMap<Id, u64> = self
            .elements
            .values()
            .map(|item| (item.id, item.order))
            .collect();
        for item in self.relationships.values() {
            let position = item.order.unwrap_or_else(|| sequence.take());
            positions.insert(item.id, position);
        }
        for item in self.groups.values() {
            let position = item.order.unwrap_or_else(|| sequence.take());
            positions.insert(item.id, position);
        }
        positions
    }

    /// The positions a publication gives out next: after every record's position.
    pub fn sequence(&self) -> Sequence {
        Sequence::after(self.positions().into_values().max())
    }

    /// Every record in the order the list numbers them from 1: by position, then elements
    /// before relationships before groups, then by ID. The app's `ui/js/rows.js` agrees.
    pub fn list_order(&self) -> Vec<Id> {
        let positions = self.positions();
        let mut records: Vec<(u64, u8, Id)> = self
            .elements
            .keys()
            .map(|id| (positions[id], 0, *id))
            .chain(self.relationships.keys().map(|id| (positions[id], 1, *id)))
            .chain(self.groups.keys().map(|id| (positions[id], 2, *id)))
            .collect();
        records.sort();
        records.into_iter().map(|(_, _, id)| id).collect()
    }
}

/// Hands out the next positions of a document's publication sequence, one by one.
pub struct Sequence {
    next: u64,
}

impl Sequence {
    /// Starts after `highest`, or at 0 when there is no position yet.
    pub fn after(highest: Option<u64>) -> Self {
        Sequence {
            next: highest.map_or(0, |highest| highest + 1),
        }
    }

    pub fn take(&mut self) -> u64 {
        let position = self.next;
        self.next += 1;
        position
    }
}

/// The shared content actions; drop is archive under the name users see. Unknown fields are
/// refused, so `ids` on a document action is an error instead of being ignored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentAction {
    Accept {
        ids: Vec<Id>,
    },
    Archive {
        ids: Vec<Id>,
    },
    /// Takes the whole document out of the app's document strip, for the agent and every app
    /// view; its content, questions and answers stay in the store.
    Close {},
    /// Brings a closed document back.
    Reopen {},
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub document_id: Id,
    pub revision: u64,
    pub ids: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Start,
    Steer,
    Queue,
    Share,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeliveryOutcome {
    Pending,
    Submitted,
    Queued { reason: Option<String> },
    Started { turn_id: String },
    Failed { reason: String },
    Refused { reason: String },
    Dropped { reason: String },
    Unknown { reason: String },
}

/// What the user asked the agent to do with a shared question, beyond reading the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Explain the items or the quoted text; the agent records the explanation through the
    /// answer tool, and the app shows it on each item.
    Explain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionSnapshot {
    pub selection: Selection,
    pub document_title: String,
    pub elements: BTreeMap<Id, Element>,
    pub relationships: BTreeMap<Id, Relationship>,
    pub groups: BTreeMap<Id, Group>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub request_id: Id,
    pub workspace_id: Id,
    pub generation: u64,
    pub snapshot: SelectionSnapshot,
    pub text: String,
    pub mode: DeliveryMode,
    pub outcome: DeliveryOutcome,
    /// Recorded by the agent through the answer tool after replying in its conversation.
    #[serde(default)]
    pub answer: Option<String>,
    /// `None` for an ordinary ask. Optional and left out when `None`, so questions stored
    /// before it load and ordinary questions serialize as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    /// The item whose quote in `text` is a part the user selected, not its whole text; `None`
    /// when every item is quoted in full. Optional and left out when `None`, as `intent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted: Option<Id>,
}

/// Escape terminal controls at rendering boundaries, including persisted content.
pub fn terminal_safe(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            if character.is_control() && character != '\n' && character != '\t' {
                character.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

/// Persisted text on one line, with terminal controls escaped.
pub fn single_line(text: &str) -> String {
    terminal_safe(text).replace(['\n', '\t'], " ")
}
