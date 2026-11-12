//! App protocol: JSON messages over the WebSocket that a bridge opens to the desktop app.
//!
//! Handshake: the bridge sends `hello` with its nonce. The app answers `welcome` with its own
//! nonce and its proof of the pairing token over the bridge's nonce. Only after verifying
//! that proof does the bridge answer `auth` with its proof over the app's nonce and its
//! `session`, then the `snapshot`, then an `update` after every committed change and `closed`
//! when the conversation ends. The token itself never crosses. Proving first leaks nothing
//! to a fake app: the role labels in the proofs keep one side's proof from being reflected
//! as the other's, and a relay would have to sit at the address the bridge dials while the
//! real app runs.
//!
//! Refusals: either side answers a peer it does not accept with `refused` and closes. The app
//! decides superseding only after the `auth` proof verified, and may send `refused:
//! superseded` at any time after that, also to an older bridge that is already connected
//! when a newer one authenticates. The bridge stops retrying only on `superseded` from an app
//! whose `welcome` proof it verified on that connection; on every other refusal or a failed
//! proof it closes and retries with backoff.
//!
//! Content: documents, and questions shared for the CLI prompt box (`DeliveryMode::Share`);
//! other questions never cross. A snapshot is the whole content: its frames go back to back
//! with no `update` between them, and when the `more: false` frame arrives the app drops
//! every document and question that snapshot did not contain. An `update` only adds or
//! replaces by ID. The app keeps view state (hidden, filter, zoom, pan, and which items are
//! selected); a selection crosses only inside `ask`, as the content it asks about.
//!
//! Closed documents: whether a document is closed is shared state in the store, never app
//! state. Closed documents still cross with the rest, so the app can list and reopen them;
//! `closed` names those that are closed. In a snapshot the IDs of all its frames together are
//! the whole closed set; an `update` always carries the whole set after its change, which
//! replaces the previous one. Questions and answers on a closed document cross as before.
//!
//! Bookmarks: the user's marks on items are shared state too, records of their document
//! (`Document::bookmarks`) that cross with it. The `bookmark` and `unbookmark` actions change
//! them like any other action; protocol 3 added them.
//!
//! Commands: the app sends commands with a fresh `id`; the bridge answers each one it
//! receives with a `reply` carrying that `id`, after the `update` (or, for `fetch`, the
//! `snapshot`) it caused. A command sent just before a connection dropped may have committed
//! without its reply arriving. Resend it after the reconnect: `ask` uses `id` as the
//! question's request ID, so the store returns the question it already holds; `action` and
//! `undo` carry no request ID, so a resend of one that committed fails with `stale_revision`,
//! and the app then sends `fetch` and shows the current content.
use crate::model::{
    AgentKind, ContentAction, Document, Id, Intent, MAX_PAYLOAD_BYTES, Question, Selection,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

/// Versioning rule: a new optional field does not change this number, and an older peer
/// ignores it. A new message type, a new required field or a changed meaning increases it.
/// Peers with different numbers refuse each other with `refused`.
pub const APP_PROTOCOL: u32 = 3;

/// Largest WebSocket message either side sends or accepts. Snapshot frames pack records up
/// to `MAX_PAYLOAD_BYTES`, and the store refuses any single record larger than that, so a
/// frame with its envelope stays below this.
pub const MAX_MESSAGE_BYTES: usize = 2 * MAX_PAYLOAD_BYTES;

/// Bridge to app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeMessage {
    Hello {
        protocol: u32,
        nonce: String,
    },
    /// The bridge's proof of the pairing token over the app's `welcome` nonce, and who the
    /// bridge speaks for; sent only after the `welcome` proof verified.
    Auth {
        proof: String,
        session: Session,
    },
    /// One frame of the whole content: after `auth` and in answer to `fetch`. `more: false`
    /// marks the last frame; see the module doc for how the app applies it.
    Snapshot {
        documents: Vec<Document>,
        questions: Vec<Question>,
        /// Closed documents; see the module doc.
        closed: Vec<Id>,
        more: bool,
    },
    /// Documents and questions changed by one committed change; the app adds or replaces
    /// them by ID. `closed` is the whole closed set after the change.
    Update {
        documents: Vec<Document>,
        questions: Vec<Question>,
        closed: Vec<Id>,
    },
    /// The conversation ended; the app keeps the tab and marks it ended.
    Closed {
        reason: String,
    },
    /// `error` is `None` when the command committed.
    Reply {
        id: Id,
        error: Option<CommandError>,
    },
    /// Answers a `welcome` the bridge does not accept, then the bridge closes the connection.
    Refused {
        protocol: u32,
        reason: Refusal,
    },
}

/// App to bridge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMessage {
    /// Answers `hello`: the app's nonce and its proof of the pairing token over the bridge's.
    Welcome {
        protocol: u32,
        nonce: String,
        proof: String,
    },
    /// Shares the selection with the item reference and quoted text for the CLI prompt box.
    /// `id` is also the question's request ID.
    Ask {
        id: Id,
        selection: Selection,
        text: String,
        /// Stored on the question; `None` for an ordinary ask. Optional and left out when
        /// `None`: an older bridge ignores it and shares the request as an ordinary ask.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<Intent>,
        /// The item quoted by a part the user selected (`Question::quoted`); optional as
        /// `intent`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quoted: Option<Id>,
    },
    Action {
        id: Id,
        document_id: Id,
        expected_revision: u64,
        action: ContentAction,
    },
    /// Undoes the latest action on the document that can still be undone.
    Undo {
        id: Id,
        document_id: Id,
        expected_revision: u64,
    },
    /// Asks for a fresh `snapshot`.
    Fetch { id: Id },
    /// Answers a `hello` or `auth` the app does not accept, then the app closes the
    /// connection. `superseded` may follow at any time after `auth` verified.
    Refused { protocol: u32, reason: Refusal },
}

/// Who is speaking: the app keys tabs by agent and conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Host name of the container the bridge runs in.
    pub host: String,
    pub project: String,
    pub agent: AgentKind,
    pub conversation: String,
    pub cli: CliProcess,
    pub binding_id: Id,
    /// When the conversation was bound (or resumed) to this CLI, in milliseconds since the
    /// Unix epoch. It belongs to the binding, so a restarted bridge reports the same value.
    /// Never 0: a bridge that loads an older binding without it fills in 1, which means the
    /// bind time is unknown (the binding was written before this field existed) and always
    /// loses to a recorded time.
    pub bound_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliProcess {
    pub pid: u32,
    pub start_time: String,
}

impl Session {
    /// Two bridges for one conversation (a resume in a second container): the later binding
    /// wins, and the binding ID breaks a tie. Equal `(bound_at_ms, binding_id)` is the same
    /// bridge reconnecting: neither supersedes the other, and the app replaces the old
    /// connection with the new one instead of refusing it.
    pub fn supersedes(&self, other: &Session) -> bool {
        self.agent == other.agent
            && self.conversation == other.conversation
            && (self.bound_at_ms, self.binding_id) > (other.bound_at_ms, other.binding_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Token,
    Protocol,
    /// A later binding of the same conversation holds the tab.
    Superseded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CommandError {
    /// The document moved on; the app shows `current` and asks again from there.
    #[error("revision conflict; current revision is {current}")]
    StaleRevision { current: u64 },
    #[error("not found: {message}")]
    NotFound { message: String },
    #[error("invalid content: {message}")]
    Invalid { message: String },
    #[error("request ID was already used with a different payload")]
    RequestConflict,
    #[error("undo conflicts with subsequent changes")]
    UndoConflict,
    /// The bridge could not run the command, for example because the binding retired.
    #[error("{message}")]
    Failed { message: String },
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("message carries no protocol number")]
    Missing,
    #[error(
        "peer speaks Prism protocol {theirs}, this side speaks {APP_PROTOCOL}; update the older one"
    )]
    Unsupported { theirs: u32 },
    #[error("handshake nonce must be {NONCE_CHARS} lowercase hex characters")]
    InvalidNonce,
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("record of {bytes} bytes exceeds the snapshot frame limit of {MAX_PAYLOAD_BYTES}")]
    TooLarge { bytes: usize },
    #[error("record does not serialize: {0}")]
    Json(#[from] serde_json::Error),
}

/// Checks the protocol number of a `hello`, `welcome` or `refused` before parsing the rest,
/// so a newer peer is recognized even when its message no longer parses here.
pub fn check_protocol(message: &str) -> Result<(), ProtocolError> {
    #[derive(Deserialize)]
    struct Versioned {
        protocol: Option<u32>,
    }

    let theirs = serde_json::from_str::<Versioned>(message)
        .ok()
        .and_then(|versioned| versioned.protocol)
        .ok_or(ProtocolError::Missing)?;
    if theirs != APP_PROTOCOL {
        return Err(ProtocolError::Unsupported { theirs });
    }
    Ok(())
}

/// Splits a snapshot into frames under `MAX_MESSAGE_BYTES`: the closed set in the first
/// frame, documents first, then questions, each frame packed up to `MAX_PAYLOAD_BYTES` of
/// records. Always at least one frame, so an empty conversation still ends with `more: false`.
/// A record (or the closed set) larger than `MAX_PAYLOAD_BYTES` on its own is an error, never
/// an oversized frame.
pub fn snapshot_frames(
    documents: Vec<Document>,
    questions: Vec<Question>,
    closed: Vec<Id>,
) -> Result<Vec<BridgeMessage>, FrameError> {
    #[derive(Default)]
    struct Frame {
        documents: Vec<Document>,
        questions: Vec<Question>,
        closed: Vec<Id>,
        bytes: usize,
    }

    // A record that does not fit the current frame opens the next one.
    fn frame_for(frames: &mut Vec<Frame>, bytes: usize) -> Result<&mut Frame, FrameError> {
        if bytes > MAX_PAYLOAD_BYTES {
            return Err(FrameError::TooLarge { bytes });
        }
        if frames
            .last()
            .is_none_or(|frame| frame.bytes > 0 && frame.bytes + bytes > MAX_PAYLOAD_BYTES)
        {
            frames.push(Frame::default());
        }
        let frame = frames.last_mut().expect("a frame was pushed");
        frame.bytes += bytes;
        Ok(frame)
    }

    let mut frames = vec![Frame::default()];
    let bytes = serde_json::to_vec(&closed)?.len();
    frame_for(&mut frames, bytes)?.closed = closed;
    for document in documents {
        let bytes = serde_json::to_vec(&document)?.len();
        frame_for(&mut frames, bytes)?.documents.push(document);
    }
    for question in questions {
        let bytes = serde_json::to_vec(&question)?.len();
        frame_for(&mut frames, bytes)?.questions.push(question);
    }

    let count = frames.len();
    Ok(frames
        .into_iter()
        .enumerate()
        .map(|(index, frame)| BridgeMessage::Snapshot {
            documents: frame.documents,
            questions: frame.questions,
            closed: frame.closed,
            more: index + 1 < count,
        })
        .collect())
}

/// Whose proof of the pairing token. The challenge is always the verifier's nonce: the app
/// proves over the bridge's `hello` nonce, the bridge over the app's `welcome` nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prover {
    App,
    Bridge,
}

impl Prover {
    fn label(self) -> &'static str {
        match self {
            Prover::App => "app",
            Prover::Bridge => "bridge",
        }
    }
}

const NONCE_CHARS: usize = 64;

/// A fresh handshake nonce: 64 lowercase hex characters from two random UUIDs.
pub fn nonce() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn check_nonce(nonce: &str) -> Result<(), ProtocolError> {
    let valid = nonce.len() == NONCE_CHARS
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err(ProtocolError::InvalidNonce);
    }
    Ok(())
}

fn token_mac(token: &str, prover: Prover, challenge: &str) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(prover.label().as_bytes());
    mac.update(challenge.as_bytes());
    mac
}

/// HMAC-SHA256 keyed with the pairing token over the prover's label and the challenge (the
/// nonce the peer sent), as lowercase hex. A malformed nonce is refused, not proven over.
pub fn proof(token: &str, prover: Prover, challenge: &str) -> Result<String, ProtocolError> {
    check_nonce(challenge)?;
    Ok(token_mac(token, prover, challenge)
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Checks the peer's proof over this side's own nonce in constant time. An empty token or a
/// malformed nonce never verifies.
pub fn verify_proof(token: &str, prover: Prover, challenge: &str, proof: &str) -> bool {
    if token.is_empty() || check_nonce(challenge).is_err() {
        return false;
    }
    let Some(bytes) = decode_hex(proof) else {
        return false;
    };
    token_mac(token, prover, challenge)
        .verify_slice(&bytes)
        .is_ok()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}
