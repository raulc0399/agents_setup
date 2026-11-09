//! Portable content types live in `prism_core::model`; view state stays with the terminal UI.
pub use prism_core::model::*;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
