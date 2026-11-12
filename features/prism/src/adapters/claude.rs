//! Exact-session Claude lifecycle observer. Native sends stay disabled; the mod reports
//! `session.end` through the authenticated bridge and this adapter retires the binding.

use super::{AgentAdapter, Capabilities, NativeEvent, delivery_fingerprint};
use crate::model::{DeliveryOutcome, Question};
use anyhow::{Result, bail};
use uuid::Uuid;

pub const SEND_GATE_REASON: &str = "Native sends are disabled: Claude's prompt receipt carries no request-to-turn correlation, so a submission can be neither confirmed as started nor cancelled. Share the question, then ask the bound Claude conversation to retrieve it with the Prism questions tool.";

pub struct ClaudeAdapter {
    session_id: String,
    generation: u64,
    events: Vec<NativeEvent>,
    retired: Option<String>,
    capabilities: Capabilities,
}

impl ClaudeAdapter {
    /// The caller must authenticate the binding before observing its session.
    pub fn new(session_id: &str, generation: u64) -> Result<Self> {
        let id = Uuid::parse_str(session_id);
        if generation == 0 || !id.is_ok_and(|id| !id.is_nil() && id.to_string() == session_id) {
            bail!(
                "invalid Claude binding: session ID must be a canonical UUID with a live generation"
            );
        }
        Ok(Self {
            session_id: session_id.to_owned(),
            generation,
            events: Vec::new(),
            retired: None,
            capabilities: Capabilities {
                start: false,
                steer: false,
                queue: false,
                share: true,
                reason: Some(SEND_GATE_REASON.to_owned()),
            },
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn retired(&self) -> Option<&str> {
        self.retired.as_deref()
    }

    /// `clear`, `resume` and `logout` continue the process under another session ID;
    /// every other reason means the bound CLI process is ending.
    pub fn observe_session_end(&mut self, reason: &str) {
        let message = match reason {
            "clear" | "resume" | "logout" => {
                format!("Claude conversation changed ({reason}); explicitly resume")
            }
            _ => format!("Claude CLI exited ({reason})"),
        };
        self.retire(&message);
    }
}

impl AgentAdapter for ClaudeAdapter {
    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    /// Every refusal is decided locally; no native request is ever sent.
    fn submit(&mut self, question: &Question) -> DeliveryOutcome {
        if question.generation != self.generation {
            return DeliveryOutcome::Refused {
                reason: "Question belongs to a stale binding generation".to_owned(),
            };
        }
        if delivery_fingerprint(question).is_none() {
            return DeliveryOutcome::Refused {
                reason: "Question exceeds the native delivery bounds".to_owned(),
            };
        }
        if let Some(reason) = &self.retired {
            return DeliveryOutcome::Refused {
                reason: reason.clone(),
            };
        }
        DeliveryOutcome::Refused {
            reason: self
                .capabilities
                .reason
                .clone()
                .unwrap_or_else(|| "This native delivery mode is unavailable".to_owned()),
        }
    }

    fn poll_events(&mut self) -> Result<Vec<NativeEvent>> {
        Ok(std::mem::take(&mut self.events))
    }

    fn retire(&mut self, reason: &str) {
        if self.retired.is_some() {
            return;
        }
        self.retired = Some(reason.to_owned());
        self.capabilities.start = false;
        self.capabilities.steer = false;
        self.capabilities.queue = false;
        self.capabilities.reason = Some(reason.to_owned());
        self.events.push(NativeEvent::Retired {
            reason: reason.to_owned(),
        });
    }
}

#[cfg(test)]
mod contract {
    use crate::adapters::claude::{ClaudeAdapter, SEND_GATE_REASON};
    use crate::adapters::{AgentAdapter, NativeEvent};
    use crate::model::{
        DeliveryMode, DeliveryOutcome, MAX_TEXT_BYTES, Question, Selection, SelectionSnapshot,
    };
    use std::collections::BTreeMap;
    use uuid::Uuid;

    const SESSION: &str = "4f4e0ac8-2d3e-4a6b-9b7a-0b1c2d3e4f50";

    fn adapter() -> ClaudeAdapter {
        ClaudeAdapter::new(SESSION, 7).unwrap()
    }

    fn question(mode: DeliveryMode) -> Question {
        Question {
            request_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            generation: 7,
            snapshot: SelectionSnapshot {
                selection: Selection {
                    document_id: Uuid::new_v4(),
                    revision: 1,
                    ids: Vec::new(),
                },
                document_title: "Plan".into(),
                elements: BTreeMap::new(),
                relationships: BTreeMap::new(),
                groups: BTreeMap::new(),
            },
            text: "Explain the selected step".into(),
            mode,
            outcome: DeliveryOutcome::Pending,
            answer: None,
            intent: None,
            quoted: None,
        }
    }

    #[test]
    fn requires_canonical_session_and_live_generation() {
        assert_eq!(adapter().session_id(), SESSION);
        assert!(ClaudeAdapter::new(SESSION, 0).is_err());
        assert!(ClaudeAdapter::new(&SESSION.to_uppercase(), 1).is_err());
        assert!(ClaudeAdapter::new(&Uuid::nil().to_string(), 1).is_err());
        assert!(ClaudeAdapter::new("not-a-uuid", 1).is_err());
    }

    #[test]
    fn capabilities_report_share_only_with_the_gate_reason() {
        let capabilities = adapter().capabilities();
        assert!(!capabilities.start && !capabilities.steer && !capabilities.queue);
        assert!(capabilities.share);
        assert_eq!(capabilities.reason.as_deref(), Some(SEND_GATE_REASON));
    }

    #[test]
    fn every_delivery_mode_is_refused_with_the_gate_reason() {
        let mut adapter = adapter();
        for mode in [
            DeliveryMode::Start,
            DeliveryMode::Steer,
            DeliveryMode::Queue,
            DeliveryMode::Share,
        ] {
            assert_eq!(
                adapter.submit(&question(mode)),
                DeliveryOutcome::Refused {
                    reason: SEND_GATE_REASON.to_owned()
                }
            );
        }
        assert!(adapter.poll_events().unwrap().is_empty());
    }

    #[test]
    fn stale_generation_and_oversized_text_are_refused() {
        let mut adapter = adapter();
        let mut question = question(DeliveryMode::Start);
        question.generation = 6;
        assert!(matches!(
            adapter.submit(&question),
            DeliveryOutcome::Refused { ref reason } if reason.contains("stale binding generation")
        ));
        question.generation = 7;
        question.text = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(matches!(
            adapter.submit(&question),
            DeliveryOutcome::Refused { ref reason } if reason.contains("delivery bounds")
        ));
    }

    #[test]
    fn request_replay_returns_the_same_outcome() {
        let mut adapter = adapter();
        let question = question(DeliveryMode::Start);
        let outcome = adapter.submit(&question);
        assert_eq!(adapter.submit(&question), outcome);
        assert!(matches!(outcome, DeliveryOutcome::Refused { .. }));
    }

    #[test]
    fn session_end_reasons_map_to_retirement_messages() {
        for (reason, expected) in [
            (
                "clear",
                "Claude conversation changed (clear); explicitly resume",
            ),
            (
                "resume",
                "Claude conversation changed (resume); explicitly resume",
            ),
            (
                "logout",
                "Claude conversation changed (logout); explicitly resume",
            ),
            ("prompt_input_exit", "Claude CLI exited (prompt_input_exit)"),
            ("other", "Claude CLI exited (other)"),
        ] {
            let mut adapter = adapter();
            adapter.observe_session_end(reason);
            assert_eq!(
                adapter.poll_events().unwrap(),
                vec![NativeEvent::Retired {
                    reason: expected.to_owned()
                }]
            );
            assert_eq!(adapter.capabilities().reason.as_deref(), Some(expected));
            assert_eq!(
                adapter.submit(&question(DeliveryMode::Share)),
                DeliveryOutcome::Refused {
                    reason: expected.to_owned()
                }
            );
        }
    }

    #[test]
    fn retire_is_idempotent_and_poll_drains() {
        let mut adapter = adapter();
        adapter.retire("first");
        adapter.retire("second");
        adapter.observe_session_end("clear");
        assert_eq!(
            adapter.poll_events().unwrap(),
            vec![NativeEvent::Retired {
                reason: "first".to_owned()
            }]
        );
        assert!(adapter.poll_events().unwrap().is_empty());
        assert_eq!(adapter.capabilities().reason.as_deref(), Some("first"));
    }
}
