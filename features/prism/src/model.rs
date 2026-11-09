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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub document_id: Id,
    pub revision: u64,
    pub ids: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    List,
    Diagram,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

/// Local UI state. Agent inspection returns Document, never ViewState.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewState {
    pub id: Id,
    pub document_id: Id,
    pub kind: ViewKind,
    pub selection: Option<Selection>,
    pub positions: BTreeMap<Id, Position>,
    pub hidden: BTreeSet<Id>,
    pub filter: String,
    pub draft: String,
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
