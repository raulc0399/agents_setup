//! Native conversation capabilities and delivery observations.

pub mod claude;
pub mod codex;

use crate::model::{DeliveryOutcome, MAX_PAYLOAD_BYTES, MAX_TEXT_BYTES, Question};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The content identity of a delivery attempt, or None when it exceeds the native bounds.
pub fn delivery_fingerprint(question: &Question) -> Option<String> {
    serde_json::to_string(&(
        question.workspace_id,
        question.generation,
        &question.snapshot,
        &question.text,
        &question.mode,
    ))
    .ok()
    .filter(|fingerprint| {
        fingerprint.len() <= MAX_PAYLOAD_BYTES && question.text.len() <= MAX_TEXT_BYTES
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub start: bool,
    pub steer: bool,
    pub queue: bool,
    pub share: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum NativeEvent {
    TurnStarted {
        turn_id: String,
    },
    TurnCompleted {
        turn_id: String,
        status: String,
    },
    InputAccepted {
        request_id: Uuid,
        turn_id: String,
    },
    ApprovalPending {
        method: String,
        turn_id: Option<String>,
    },
    Retired {
        reason: String,
    },
}

pub trait AgentAdapter {
    fn capabilities(&self) -> Capabilities;
    fn submit(&mut self, question: &Question) -> DeliveryOutcome;
    fn poll_events(&mut self) -> anyhow::Result<Vec<NativeEvent>>;
    fn retire(&mut self, reason: &str);
}
