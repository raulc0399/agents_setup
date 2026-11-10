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
    /// Publication position; display order is (order, id), not ID order.
    #[serde(default)]
    pub order: u64,
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
    /// A manual or review step; diagrams draw it dashed.
    #[serde(default)]
    pub manual: bool,
    pub semantic_revision: u64,
    pub accepted_revision: Option<u64>,
    pub archived: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: Id,
    pub title: String,
    pub members: BTreeSet<Id>,
    pub semantic_revision: u64,
    pub accepted_revision: Option<u64>,
    pub archived: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: Id,
    pub title: String,
    pub revision: u64,
    pub elements: BTreeMap<Id, Element>,
    pub relationships: BTreeMap<Id, Relationship>,
    pub groups: BTreeMap<Id, Group>,
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
