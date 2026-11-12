//! Sessions keyed by conversation: the connection that currently speaks for each, the
//! content it sent (kept in memory only), and the commands awaiting their `reply`. Applies
//! the rules of `prism_core::protocol` for snapshots, superseding and resending.
use prism_core::ask::{AskItem, request_text};
use prism_core::model::{
    AgentKind, ContentAction, Document, Id, Intent, MAX_TEXT_BYTES, Question, Selection,
};
use prism_core::protocol::{
    APP_PROTOCOL, AppMessage, BridgeMessage, CommandError, Refusal, Session,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

/// One tab per conversation: `<agent>:<conversation>`.
pub type SessionKey = String;

pub fn session_key(agent: &AgentKind, conversation: &str) -> SessionKey {
    format!("{}:{conversation}", agent.cli_name())
}

/// What the registry asks a connection task to do.
#[derive(Debug)]
pub enum Outgoing {
    Message(AppMessage),
    Close,
}

#[derive(Debug)]
pub struct Connection {
    pub id: u64,
    pub sender: UnboundedSender<Outgoing>,
}

impl Connection {
    /// A send fails only when the connection task already ended; its detach follows.
    fn send(&self, message: AppMessage) {
        let _ = self.sender.send(Outgoing::Message(message));
    }

    fn close(&self) {
        let _ = self.sender.send(Outgoing::Close);
    }
}

/// A lost connection is `disconnected`, not ended: the bridge retries and the conversation
/// may still go on. Only `closed` from the bridge ends it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Link {
    Live,
    Disconnected,
    Ended {
        reason: String,
    },
    /// Known from the saved arrangement; its bridge has not connected since the app started.
    Waiting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionSummary {
    pub key: SessionKey,
    pub host: String,
    pub project: String,
    pub agent: AgentKind,
    pub conversation: String,
    pub link: Link,
    /// A complete snapshot arrived; before that the tab has nothing to show.
    pub loaded: bool,
    /// Known only from the saved arrangement: its bridge has not connected since the app
    /// started, so there is no content.
    pub restored: bool,
}

/// Documents and shared questions of one session, in ID order, and the IDs of the closed
/// documents as the store has them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Content {
    pub documents: Vec<Document>,
    pub questions: Vec<Question>,
    pub closed: Vec<Id>,
}

/// A command from the UI; the registry gives it a fresh ID.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// `items` name the selected items as the list shows them, each with its text or the
    /// part the user selected (`quoted` names that item); the user types the question in the
    /// CLI prompt box. With `intent` explain, the agent is asked to explain them instead.
    Ask {
        selection: Selection,
        items: Vec<AskItem>,
        #[serde(default)]
        intent: Option<Intent>,
        #[serde(default)]
        quoted: Option<Id>,
    },
    Action {
        document_id: Id,
        expected_revision: u64,
        action: ContentAction,
    },
    Undo {
        document_id: Id,
        expected_revision: u64,
    },
    Fetch,
}

impl Command {
    fn into_message(self, id: Id) -> AppMessage {
        match self {
            Command::Ask {
                selection,
                items,
                intent,
                quoted,
            } => AppMessage::Ask {
                id,
                selection,
                text: request_text(&items, intent),
                intent,
                quoted,
            },
            Command::Action {
                document_id,
                expected_revision,
                action,
            } => AppMessage::Action {
                id,
                document_id,
                expected_revision,
                action,
            },
            Command::Undo {
                document_id,
                expected_revision,
            } => AppMessage::Undo {
                id,
                document_id,
                expected_revision,
            },
            Command::Fetch => AppMessage::Fetch { id },
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CommandRejected {
    #[error("the conversation has not connected since Prism started")]
    Unknown(SessionKey),
    #[error("the conversation is not connected; wait until its bridge reconnects")]
    NotLive,
    /// The store refuses a question text over `MAX_TEXT_BYTES`; refused here, before queuing.
    #[error(
        "the question with its quoted items is {bytes} bytes, more than the {MAX_TEXT_BYTES} the CLI accepts; quote less or ask about fewer items"
    )]
    TooLong { bytes: usize },
}

/// What changed, for the shell to forward to the windows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostEvent {
    /// Sessions, arrangement or listener status changed; windows read the state again.
    State,
    /// The content of one session changed.
    Content { key: SessionKey },
    /// A command finished: answered by the bridge, or failed because its session moved on.
    Reply {
        key: SessionKey,
        id: Id,
        error: Option<CommandError>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Attach {
    Accepted,
    /// A later binding of this conversation holds the tab; the connection is refused.
    Superseded,
}

#[derive(Default)]
struct SnapshotFrames {
    documents: BTreeMap<Id, Document>,
    questions: BTreeMap<Id, Question>,
    closed: BTreeSet<Id>,
}

struct Entry {
    session: Session,
    link: Link,
    connection: Option<Connection>,
    documents: BTreeMap<Id, Document>,
    questions: BTreeMap<Id, Question>,
    closed: BTreeSet<Id>,
    /// Frames of a snapshot in progress; content is replaced when its last frame arrives.
    snapshot: Option<SnapshotFrames>,
    loaded: bool,
    /// Commands sent and not answered yet, in sending order.
    pending: Vec<Pending>,
    /// Since when the session has had no connection, while it is disconnected.
    disconnected_at: Option<Instant>,
}

struct Pending {
    message: AppMessage,
    /// First sent; a resend after a reconnect keeps it, so the total wait stays bounded.
    sent_at: Instant,
}

impl Entry {
    fn new(session: Session, connection: Connection) -> Self {
        Self {
            session,
            link: Link::Live,
            connection: Some(connection),
            documents: BTreeMap::new(),
            questions: BTreeMap::new(),
            closed: BTreeSet::new(),
            snapshot: None,
            loaded: false,
            pending: Vec::new(),
            disconnected_at: None,
        }
    }

    fn send(&self, message: AppMessage) {
        if let Some(connection) = &self.connection {
            connection.send(message);
        }
    }

    /// Commands the bridge will never answer, reported to the UI as failed.
    fn fail_pending(&mut self, key: &str, message: &str, events: &mut Vec<HostEvent>) {
        self.fail_pending_where(key, message, events, |_| true);
    }

    fn fail_pending_where(
        &mut self,
        key: &str,
        message: &str,
        events: &mut Vec<HostEvent>,
        failed: impl Fn(&Pending) -> bool,
    ) {
        self.pending.retain(|pending| {
            if !failed(pending) {
                return true;
            }
            if let Some(id) = command_id(&pending.message) {
                events.push(HostEvent::Reply {
                    key: key.to_owned(),
                    id,
                    error: Some(CommandError::Failed {
                        message: message.to_owned(),
                    }),
                });
            }
            false
        });
    }

    fn push_pending(&mut self, message: AppMessage) {
        self.send(message.clone());
        self.pending.push(Pending {
            message,
            sent_at: Instant::now(),
        });
    }
}

fn command_id(message: &AppMessage) -> Option<Id> {
    match message {
        AppMessage::Ask { id, .. }
        | AppMessage::Action { id, .. }
        | AppMessage::Undo { id, .. }
        | AppMessage::Fetch { id } => Some(*id),
        AppMessage::Welcome { .. } | AppMessage::Refused { .. } => None,
    }
}

const SUPERSEDED_MESSAGE: &str =
    "the conversation moved to a newer session before the bridge answered";
const ENDED_MESSAGE: &str = "the conversation ended before the bridge answered";
const DISCONNECTED_MESSAGE: &str =
    "the bridge stayed disconnected; check the content and try again";
const TIMEOUT_MESSAGE: &str = "the bridge did not answer in time; check the content and try again";

#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<SessionKey, Entry>,
    events: Vec<HostEvent>,
}

impl Registry {
    /// Events since the last call, for the caller to emit after releasing the registry.
    pub fn take_events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.events)
    }

    /// Admits an authenticated connection. A newer binding replaces an older one, which is
    /// refused with `superseded`; the same binding reconnecting replaces its old connection
    /// and receives again the commands that were not answered.
    pub fn attach(&mut self, session: Session, connection: Connection) -> Attach {
        let key = session_key(&session.agent, &session.conversation);
        let Some(entry) = self.entries.get_mut(&key) else {
            self.entries.insert(key, Entry::new(session, connection));
            self.events.push(HostEvent::State);
            return Attach::Accepted;
        };
        if entry.session.supersedes(&session) {
            return Attach::Superseded;
        }

        if session.supersedes(&entry.session) {
            if let Some(older) = entry.connection.take() {
                older.send(AppMessage::Refused {
                    protocol: APP_PROTOCOL,
                    reason: Refusal::Superseded,
                });
                older.close();
            }
            entry.fail_pending(&key, SUPERSEDED_MESSAGE, &mut self.events);
            entry.session = session;
        } else if let Some(previous) = entry.connection.take() {
            previous.close();
        }
        entry.connection = Some(connection);
        entry.link = Link::Live;
        entry.snapshot = None;
        entry.disconnected_at = None;
        for pending in &entry.pending {
            entry.send(pending.message.clone());
        }
        self.events.push(HostEvent::State);
        Attach::Accepted
    }

    /// The connection ended without `closed`: its session waits for a reconnect. Returns the
    /// session's key.
    pub fn detach(&mut self, connection: u64) -> Option<SessionKey> {
        let (key, entry) = self
            .entries
            .iter_mut()
            .find(|(_, entry)| connection_is(entry, connection))?;
        entry.connection = None;
        entry.snapshot = None;
        entry.disconnected_at = Some(Instant::now());
        if entry.link == Link::Live {
            entry.link = Link::Disconnected;
        }
        self.events.push(HostEvent::State);
        Some(key.clone())
    }

    /// Fails the commands of sessions that stayed disconnected for `grace`, and every command
    /// unanswered after `timeout`, so the UI never waits forever.
    pub fn expire(&mut self, now: Instant, timeout: Duration, grace: Duration) {
        let Registry { entries, events } = self;
        for (key, entry) in entries.iter_mut() {
            let gone = entry
                .disconnected_at
                .is_some_and(|since| now.duration_since(since) >= grace);
            if gone {
                entry.fail_pending(key, DISCONNECTED_MESSAGE, events);
                continue;
            }
            entry.fail_pending_where(key, TIMEOUT_MESSAGE, events, |pending| {
                now.duration_since(pending.sent_at) >= timeout
            });
        }
    }

    /// Returns the key and reason of a session that `closed` ended, for the arrangement to
    /// remember.
    pub fn receive(
        &mut self,
        connection: u64,
        message: BridgeMessage,
    ) -> Option<(SessionKey, String)> {
        let Registry { entries, events } = self;
        let (key, entry) = entries
            .iter_mut()
            .find(|(_, entry)| connection_is(entry, connection))?;
        match message {
            BridgeMessage::Snapshot {
                documents,
                questions,
                closed,
                more,
            } => {
                let frames = entry.snapshot.get_or_insert_default();
                frames.documents.extend(documents.into_iter().map(by_id));
                frames
                    .questions
                    .extend(questions.into_iter().map(question_by_id));
                frames.closed.extend(closed);
                if more {
                    return None;
                }
                // The last frame completes the snapshot: whatever it lacks is gone.
                let frames = entry.snapshot.take().unwrap_or_default();
                entry.documents = frames.documents;
                entry.questions = frames.questions;
                entry.closed = frames.closed;
                if !entry.loaded {
                    entry.loaded = true;
                    events.push(HostEvent::State);
                }
                events.push(HostEvent::Content { key: key.clone() });
            }
            BridgeMessage::Update {
                documents,
                questions,
                closed,
            } => {
                entry.documents.extend(documents.into_iter().map(by_id));
                entry
                    .questions
                    .extend(questions.into_iter().map(question_by_id));
                // Every update carries the whole closed set.
                entry.closed = closed.into_iter().collect();
                events.push(HostEvent::Content { key: key.clone() });
            }
            BridgeMessage::Closed { reason } => {
                entry.link = Link::Ended {
                    reason: reason.clone(),
                };
                entry.fail_pending(key, ENDED_MESSAGE, events);
                events.push(HostEvent::State);
                return Some((key.clone(), reason));
            }
            BridgeMessage::Reply { id, error } => {
                let position = entry
                    .pending
                    .iter()
                    .position(|pending| command_id(&pending.message) == Some(id))?;
                let command = entry.pending.remove(position).message;
                // The content moved on (or a resent command had committed): show it as it is.
                let stale = matches!(error, Some(CommandError::StaleRevision { .. }));
                if stale && !matches!(command, AppMessage::Fetch { .. }) {
                    entry.push_pending(Command::Fetch.into_message(Id::new_v4()));
                }
                events.push(HostEvent::Reply {
                    key: key.clone(),
                    id,
                    error,
                });
            }
            // Handshake messages carry nothing once the handshake is done.
            BridgeMessage::Hello { .. }
            | BridgeMessage::Auth { .. }
            | BridgeMessage::Refused { .. } => {}
        }
        None
    }

    pub fn send_command(&mut self, key: &str, command: Command) -> Result<Id, CommandRejected> {
        let entry = self
            .entries
            .get_mut(key)
            .ok_or_else(|| CommandRejected::Unknown(key.to_owned()))?;
        if entry.link != Link::Live {
            return Err(CommandRejected::NotLive);
        }

        // Time-ordered: an `ask` ID becomes the question's request ID, and the UI shows the
        // latest question first by it.
        let id = Id::now_v7();
        let message = command.into_message(id);
        if let AppMessage::Ask { text, .. } = &message
            && text.len() > MAX_TEXT_BYTES
        {
            return Err(CommandRejected::TooLong { bytes: text.len() });
        }
        entry.push_pending(message);
        Ok(id)
    }

    pub fn session(&self, key: &str) -> Option<&Session> {
        self.entries.get(key).map(|entry| &entry.session)
    }

    pub fn summaries(&self) -> Vec<SessionSummary> {
        self.entries
            .iter()
            .map(|(key, entry)| SessionSummary {
                key: key.clone(),
                host: entry.session.host.clone(),
                project: entry.session.project.clone(),
                agent: entry.session.agent.clone(),
                conversation: entry.session.conversation.clone(),
                link: entry.link.clone(),
                loaded: entry.loaded,
                restored: false,
            })
            .collect()
    }

    pub fn content(&self, key: &str) -> Option<Content> {
        self.entries.get(key).map(|entry| Content {
            documents: entry.documents.values().cloned().collect(),
            questions: entry.questions.values().cloned().collect(),
            closed: entry.closed.iter().copied().collect(),
        })
    }

    /// Closes every connection, for example when the app quits.
    pub fn close_all(&mut self) {
        for entry in self.entries.values() {
            if let Some(connection) = &entry.connection {
                connection.close();
            }
        }
    }
}

fn connection_is(entry: &Entry, connection: u64) -> bool {
    entry
        .connection
        .as_ref()
        .is_some_and(|current| current.id == connection)
}

fn by_id(document: Document) -> (Id, Document) {
    (document.id, document)
}

fn question_by_id(question: Question) -> (Id, Question) {
    (question.request_id, question)
}
