//! Exact-thread Codex app-server client. Approval requests are never answered.

use super::{AgentAdapter, Capabilities, NativeEvent, delivery_fingerprint};
use crate::model::{DeliveryMode, DeliveryOutcome, Question};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};
use uuid::Uuid;

const MAX_EVENTS: usize = 512;
const MAX_ATTEMPTS: usize = 1024;
const EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(10);
const THREAD_CLOSED_REASON: &str = "Codex closed the bound thread";
const SEND_GATE_REASON: &str = "Native sends are disabled: current TUI conversation focus and all required approval ownership checks are not verified. Share the question and retrieve it from the bound Codex conversation.";

#[derive(Debug, thiserror::Error)]
pub(crate) enum RpcError {
    #[error("Codex rejected the request ({code}): {message}")]
    Rejected { code: i64, message: String },
    #[error("{0}")]
    Connection(String),
    #[error("{THREAD_CLOSED_REASON}")]
    ThreadClosed { server_pid: u32 },
}

pub(crate) fn connect_unix(path: &Path) -> io::Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: zero is valid for the plain C Unix socket address structure.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "invalid native Unix socket path",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, byte) in address.sun_path.iter_mut().zip(bytes) {
        *target = *byte as libc::c_char;
    }
    // SOCK_NONBLOCK makes a full Unix listener queue fail promptly instead of blocking.
    // SAFETY: socket takes constants and returns a fresh descriptor or -1.
    let descriptor = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the fresh descriptor is owned exactly once, including all error paths.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    // SAFETY: the address pointer and length refer to the initialized pathname structure.
    if unsafe {
        libc::connect(
            descriptor.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            length as libc::socklen_t,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let stream = UnixStream::from(descriptor);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// A WebSocket frame can require many socket reads; all share one operation deadline.
#[derive(Debug)]
struct DeadlineStream {
    stream: UnixStream,
    deadline: Instant,
}

impl DeadlineStream {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::new(ErrorKind::TimedOut, "Codex operation deadline exceeded"))
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        crate::io::write_with_deadline(&self.stream, bytes, self.deadline)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.flush()
    }
}

pub struct CodexAdapter {
    socket: WebSocket<DeadlineStream>,
    thread_id: String,
    generation: u64,
    timeout: Duration,
    next_request_id: u64,
    events: Vec<NativeEvent>,
    retired: Option<String>,
    thread_closed: bool,
    capabilities: Capabilities,
    attempts: HashMap<Uuid, (String, DeliveryOutcome)>,
    pub server_user_agent: String,
    pub server_pid: u32,
}

impl CodexAdapter {
    /// The caller must authenticate the launcher binding before connecting.
    pub fn connect(
        path: &Path,
        thread_id: &str,
        generation: u64,
        timeout: Duration,
    ) -> Result<Self> {
        if thread_id.is_empty() || thread_id.len() > 512 || generation == 0 || timeout.is_zero() {
            bail!("invalid Codex binding or RPC timeout");
        }
        let stream = connect_unix(path).context("connect to the bound Codex app-server")?;
        let stream = DeadlineStream {
            stream,
            deadline: Instant::now() + timeout,
        };
        let (socket, _) = tungstenite::client("ws://localhost/", stream)
            .map_err(|error| anyhow::anyhow!("Codex WebSocket handshake: {error}"))?;
        let mut adapter = Self {
            socket,
            thread_id: thread_id.to_owned(),
            generation,
            timeout,
            next_request_id: 0,
            events: Vec::new(),
            retired: None,
            thread_closed: false,
            capabilities: Capabilities {
                start: false,
                steer: false,
                queue: false,
                share: true,
                reason: Some(SEND_GATE_REASON.to_owned()),
            },
            attempts: HashMap::new(),
            server_user_agent: String::new(),
            server_pid: 0,
        };
        let initialized = adapter.call("initialize", json!({"clientInfo": {"name": "prism", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {"experimentalApi": true}}))?;
        adapter.server_user_agent = initialized["userAgent"]
            .as_str()
            .context("Codex initialize omitted userAgent")?
            .to_owned();
        adapter.socket.get_mut().deadline = Instant::now() + timeout;
        adapter.socket.send(Message::Text(
            json!({"method": "initialized", "params": {}})
                .to_string()
                .into(),
        ))?;
        let diagnostics = adapter.call("server/diagnostics", json!({}))?;
        adapter.server_pid = diagnostics["process"]["id"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .filter(|id| *id > 0)
            .context("Codex diagnostics omitted process identity")?;
        let resumed = adapter.call("thread/resume", json!({"threadId": thread_id}))?;
        if resumed["thread"]["id"].as_str() != Some(thread_id) {
            bail!("Codex resumed a different thread; binding rejected");
        }
        Ok(adapter)
    }

    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    pub(crate) fn thread_closed(&self) -> bool {
        self.thread_closed
    }

    pub(crate) fn retirement_reason(&self) -> Option<&str> {
        self.retired.as_deref()
    }

    fn push_event(&mut self, event: NativeEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.clear();
            self.retire("Codex event buffer overflow; reconnect and revalidate the binding");
            return;
        }
        self.events.push(event);
    }

    fn observe(&mut self, message: &Value) {
        let Some(method) = message["method"].as_str() else {
            return;
        };
        let params = &message["params"];
        let observed_thread = params["threadId"]
            .as_str()
            .or_else(|| params["thread"]["id"].as_str());
        if method == "thread/started" && observed_thread.is_some_and(|id| id != self.thread_id) {
            self.retire("Another conversation started on the bound server; revalidate CLI focus");
            return;
        }
        if observed_thread != Some(self.thread_id.as_str()) {
            return;
        }
        if message.get("id").is_some() {
            self.push_event(NativeEvent::ApprovalPending {
                method: method.to_owned(),
                turn_id: params["turnId"].as_str().map(str::to_owned),
            });
            return;
        }
        match method {
            "thread/closed" => {
                self.thread_closed = true;
                self.retire(THREAD_CLOSED_REASON);
            }
            "thread/unsubscribed" => self.retire("Codex unsubscribed the observer"),
            "turn/started" => {
                if let Some(turn_id) = params["turn"]["id"].as_str() {
                    self.push_event(NativeEvent::TurnStarted {
                        turn_id: turn_id.to_owned(),
                    });
                }
            }
            "turn/completed" => {
                if let (Some(turn_id), Some(status)) = (
                    params["turn"]["id"].as_str(),
                    params["turn"]["status"].as_str(),
                ) {
                    self.push_event(NativeEvent::TurnCompleted {
                        turn_id: turn_id.to_owned(),
                        status: status.to_owned(),
                    });
                }
            }
            _ => {}
        }
    }

    fn call(&mut self, method: &str, params: Value) -> std::result::Result<Value, RpcError> {
        if let Some(reason) = &self.retired {
            return Err(RpcError::Connection(reason.clone()));
        }
        self.next_request_id += 1;
        let request_id = self.next_request_id;
        let deadline = Instant::now() + self.timeout;
        self.socket.get_mut().deadline = deadline;
        self.socket
            .send(Message::Text(
                json!({"id": request_id, "method": method, "params": params})
                    .to_string()
                    .into(),
            ))
            .map_err(|error| {
                self.retire("Codex connection failed while writing a request");
                RpcError::Connection(error.to_string())
            })?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.retire("Codex RPC timed out; the request must not be resent automatically");
                return Err(RpcError::Connection("Codex RPC timed out".to_owned()));
            }
            let frame = self.socket.read().map_err(|error| {
                self.retire("Codex connection closed or timed out; revalidate before reconnecting");
                RpcError::Connection(error.to_string())
            })?;
            let Message::Text(text) = frame else {
                if matches!(frame, Message::Close(_)) {
                    self.retire("Codex closed the native connection");
                    return Err(RpcError::Connection(
                        "Codex closed the native connection".to_owned(),
                    ));
                }
                continue;
            };
            let message: Value = match serde_json::from_str(&text) {
                Ok(message) => message,
                Err(_) => {
                    self.retire("Codex sent an invalid JSON response");
                    return Err(RpcError::Connection(
                        "invalid Codex JSON response".to_owned(),
                    ));
                }
            };
            if message.get("method").is_some() {
                self.observe(&message);
                if self.thread_closed {
                    return Err(RpcError::ThreadClosed {
                        server_pid: self.server_pid,
                    });
                }
                if let Some(reason) = &self.retired {
                    return Err(RpcError::Connection(reason.clone()));
                }
                continue;
            }
            if message["id"].as_u64() != Some(request_id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(RpcError::Rejected {
                    code: error["code"].as_i64().unwrap_or(-32603),
                    message: error["message"]
                        .as_str()
                        .unwrap_or("unspecified native error")
                        .to_owned(),
                });
            }
            if let Some(result) = message.get("result") {
                return Ok(result.clone());
            }
            self.retire("Codex response omitted its result");
            return Err(RpcError::Connection(
                "Codex response omitted result".to_owned(),
            ));
        }
    }

    fn submit_native(&mut self, question: &Question) -> DeliveryOutcome {
        let state = match self.call(
            "thread/read",
            json!({"threadId": self.thread_id, "includeTurns": true}),
        ) {
            Ok(state) if state["thread"]["id"].as_str() == Some(self.thread_id.as_str()) => state,
            Ok(_) => {
                self.retire("Codex returned state for a different thread");
                return DeliveryOutcome::Refused {
                    reason: "Codex thread identity changed".to_owned(),
                };
            }
            Err(error) => {
                return DeliveryOutcome::Failed {
                    reason: error.to_string(),
                };
            }
        };
        let active = state["thread"]["turns"]
            .as_array()
            .and_then(|turns| {
                turns
                    .iter()
                    .rev()
                    .find(|turn| turn["status"] == "inProgress")
            })
            .and_then(|turn| turn["id"].as_str());
        let (method, mut params) = if matches!(question.mode, DeliveryMode::Steer) {
            let Some(turn_id) = active else {
                return DeliveryOutcome::Refused { reason: "Codex has no active turn to steer; refresh and explicitly choose a new turn".to_owned() };
            };
            ("turn/steer", json!({"expectedTurnId": turn_id}))
        } else {
            if active.is_some() {
                return DeliveryOutcome::Refused {
                    reason:
                        "Codex is busy; explicitly choose steering after refreshing the active turn"
                            .to_owned(),
                };
            }
            ("turn/start", json!({}))
        };
        let snapshot = match serde_json::to_string(&question.snapshot) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return DeliveryOutcome::Failed {
                    reason: error.to_string(),
                };
            }
        };
        params["threadId"] = json!(self.thread_id);
        params["clientUserMessageId"] = json!(question.request_id);
        params["input"] = json!([{"type": "text", "text": format!("The user explicitly sent this Prism question. Selection snapshot: {snapshot}\n\nQuestion: {}", question.text)}]);
        match self.call(method, params) {
            Ok(result) => {
                let turn_id = result["turn"]["id"]
                    .as_str()
                    .or_else(|| result["turnId"].as_str());
                if let Some(turn_id) = turn_id {
                    self.push_event(NativeEvent::InputAccepted {
                        request_id: question.request_id,
                        turn_id: turn_id.to_owned(),
                    });
                    // A start request can race with the CLI and steer an existing turn.
                    // The acknowledgement proves acceptance, not this input's execution.
                    DeliveryOutcome::Submitted
                } else {
                    DeliveryOutcome::Unknown { reason: "Codex acknowledged input without a turn ID; do not resend automatically".to_owned() }
                }
            }
            Err(RpcError::Rejected { code, message }) => {
                // Refresh only; a stale steering precondition must never fall back to start.
                let _ = self.call("thread/read", json!({"threadId": self.thread_id}));
                DeliveryOutcome::Refused {
                    reason: format!("Codex refused input ({code}): {message}"),
                }
            }
            Err(error) => DeliveryOutcome::Unknown {
                reason: format!("{error}; input may have been accepted; explicit retry only"),
            },
        }
    }
}

impl AgentAdapter for CodexAdapter {
    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    fn submit(&mut self, question: &Question) -> DeliveryOutcome {
        if question.generation != self.generation {
            return DeliveryOutcome::Refused {
                reason: "Question belongs to a stale binding generation".to_owned(),
            };
        }
        let Some(fingerprint) = delivery_fingerprint(question) else {
            return DeliveryOutcome::Refused {
                reason: "Question exceeds the native delivery bounds".to_owned(),
            };
        };
        if let Some((previous, outcome)) = self.attempts.get(&question.request_id) {
            return if previous == &fingerprint {
                outcome.clone()
            } else {
                DeliveryOutcome::Refused {
                    reason: "Question request ID was reused with different content".to_owned(),
                }
            };
        }
        if let Some(reason) = &self.retired {
            return DeliveryOutcome::Refused {
                reason: reason.clone(),
            };
        }
        let enabled = match question.mode {
            DeliveryMode::Start => self.capabilities.start,
            DeliveryMode::Steer => self.capabilities.steer,
            DeliveryMode::Queue | DeliveryMode::Share => false,
        };
        if !enabled {
            return DeliveryOutcome::Refused {
                reason: self
                    .capabilities
                    .reason
                    .clone()
                    .unwrap_or_else(|| "This native delivery mode is unavailable".to_owned()),
            };
        }
        if !matches!(question.outcome, DeliveryOutcome::Pending)
            || self.attempts.len() >= MAX_ATTEMPTS
        {
            return DeliveryOutcome::Refused { reason: "Question was already attempted or this connection reached its attempt limit; never resend automatically".to_owned() };
        }
        let outcome = self.submit_native(question);
        self.attempts
            .insert(question.request_id, (fingerprint, outcome.clone()));
        outcome
    }

    fn poll_events(&mut self) -> Result<Vec<NativeEvent>> {
        if self.retired.is_some() {
            return Ok(std::mem::take(&mut self.events));
        }
        let deadline = Instant::now() + EVENT_POLL_TIMEOUT;
        self.socket.get_mut().deadline = deadline;
        while Instant::now() < deadline {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    let message: Value = serde_json::from_str(&text)?;
                    self.observe(&message);
                    if self.retired.is_some() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    self.retire("Codex closed the native connection");
                    break;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    break;
                }
                Err(error) => {
                    self.retire("Codex connection failed; revalidate the binding");
                    return Err(error.into());
                }
            }
        }
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
    use crate::adapters::codex::CodexAdapter;
    use crate::adapters::{AgentAdapter, NativeEvent};
    use crate::model::{DeliveryMode, DeliveryOutcome, Question, Selection, SelectionSnapshot};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;
    use tungstenite::Message;
    use uuid::Uuid;

    struct Fixture {
        _directory: TempDir,
        path: std::path::PathBuf,
        requests: Arc<Mutex<Vec<Value>>>,
        task: Option<thread::JoinHandle<()>>,
    }

    impl Fixture {
        fn new(mode: &'static str) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("codex.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&requests);
            let task = thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                while let Ok(Message::Text(text)) = socket.read() {
                    let request: Value = serde_json::from_str(&text).unwrap();
                    recorded.lock().unwrap().push(request.clone());
                    let Some(method) = request["method"].as_str() else {
                        continue;
                    };
                    let result = match method {
                        "initialize" => json!({"userAgent": "contract/0.167.0"}),
                        "initialized" => continue,
                        "server/diagnostics" => json!({"process": {"id": 42}}),
                        "thread/resume" => {
                            if mode == "approval" {
                                socket.send(Message::Text(json!({"id": 999, "method": "item/commandExecution/requestApproval", "params": {"threadId": "thread", "turnId": "turn"}}).to_string().into())).unwrap();
                            }
                            if mode == "other_events" {
                                socket.send(Message::Text(json!({"method": "turn/started", "params": {"threadId": "other", "turn": {"id": "unrelated"}}}).to_string().into())).unwrap();
                            }
                            json!({"thread": {"id": if mode == "wrong_thread" { "other" } else { "thread" }}})
                        }
                        "thread/read" => {
                            json!({"thread": {"id": "thread", "turns": if mode == "stale_steer" { vec![json!({"id": "active-turn", "status": "inProgress"})] } else { vec![] }}})
                        }
                        "turn/start" | "turn/steer" => {
                            if mode == "timeout" {
                                thread::sleep(Duration::from_millis(150));
                                continue;
                            }
                            if mode == "stale_steer" {
                                socket.send(Message::Text(json!({"id": request["id"], "error": {"code": -32600, "message": "expectedTurnId no longer matches"}}).to_string().into())).unwrap();
                                continue;
                            }
                            json!({"turn": {"id": "accepted-turn", "status": "inProgress"}})
                        }
                        _ => panic!("unexpected request {method}"),
                    };
                    if socket
                        .send(Message::Text(
                            json!({"id": request["id"], "result": result})
                                .to_string()
                                .into(),
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
            });
            Self {
                _directory: directory,
                path,
                requests,
                task: Some(task),
            }
        }

        fn connect(&self) -> CodexAdapter {
            CodexAdapter::connect(&self.path, "thread", 7, Duration::from_millis(60)).unwrap()
        }

        fn count(&self, method: &str) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request["method"] == method)
                .count()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(task) = self.task.take() {
                task.join().unwrap();
            }
        }
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
        }
    }

    #[test]
    fn rejects_wrong_thread_and_filters_unrelated_events() {
        let fixture = Fixture::new("wrong_thread");
        assert!(
            CodexAdapter::connect(&fixture.path, "thread", 7, Duration::from_millis(60)).is_err()
        );
        drop(fixture);
        let fixture = Fixture::new("other_events");
        let mut adapter = fixture.connect();
        assert!(adapter.poll_events().unwrap().is_empty());
    }

    #[test]
    fn observes_approval_without_replying() {
        let fixture = Fixture::new("approval");
        let mut adapter = fixture.connect();
        assert!(matches!(
            adapter.poll_events().unwrap().as_slice(),
            [NativeEvent::ApprovalPending { .. }]
        ));
        assert!(
            fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request["id"] != 999)
        );
    }

    #[test]
    fn unverified_native_sends_and_queue_are_refused() {
        let fixture = Fixture::new("normal");
        let mut adapter = fixture.connect();
        for mode in [
            DeliveryMode::Start,
            DeliveryMode::Steer,
            DeliveryMode::Queue,
        ] {
            assert!(matches!(
                adapter.submit(&question(mode)),
                DeliveryOutcome::Refused { .. }
            ));
        }
        assert_eq!(fixture.count("thread/read"), 0);
        assert!(!adapter.capabilities().start);
    }

    #[test]
    fn timeout_is_unknown_and_never_resends() {
        let fixture = Fixture::new("timeout");
        let mut adapter = fixture.connect();
        adapter.capabilities.start = true;
        let question = question(DeliveryMode::Start);
        let outcome = adapter.submit(&question);
        assert!(matches!(outcome, DeliveryOutcome::Unknown { .. }));
        assert_eq!(adapter.submit(&question), outcome);
        assert_eq!(fixture.count("turn/start"), 1);
    }

    #[test]
    fn stale_steering_refreshes_without_falling_back_to_start() {
        let fixture = Fixture::new("stale_steer");
        let mut adapter = fixture.connect();
        adapter.capabilities.steer = true;
        assert!(matches!(
            adapter.submit(&question(DeliveryMode::Steer)),
            DeliveryOutcome::Refused { .. }
        ));
        assert_eq!(fixture.count("turn/steer"), 1);
        assert_eq!(fixture.count("turn/start"), 0);
        assert_eq!(fixture.count("thread/read"), 2);
    }

    #[test]
    fn acknowledgement_correlates_request_without_claiming_start() {
        let fixture = Fixture::new("normal");
        let mut adapter = fixture.connect();
        adapter.capabilities.start = true;
        let question = question(DeliveryMode::Start);
        assert_eq!(adapter.submit(&question), DeliveryOutcome::Submitted);
        assert!(
            adapter
                .poll_events()
                .unwrap()
                .contains(&NativeEvent::InputAccepted {
                    request_id: question.request_id,
                    turn_id: "accepted-turn".into()
                })
        );
        let requests = fixture.requests.lock().unwrap();
        let start = requests
            .iter()
            .find(|request| request["method"] == "turn/start")
            .unwrap();
        assert_eq!(
            start["params"]["clientUserMessageId"],
            question.request_id.to_string()
        );
    }

    #[test]
    fn different_thread_start_retires_only_the_observer() {
        let fixture = Fixture::new("normal");
        let mut adapter = fixture.connect();
        adapter.observe(
            &json!({"method": "thread/started", "params": {"thread": {"id": "replacement"}}}),
        );
        assert!(matches!(
            adapter.poll_events().unwrap().as_slice(),
            [NativeEvent::Retired { .. }]
        ));
        assert!(matches!(
            adapter.submit(&question(DeliveryMode::Start)),
            DeliveryOutcome::Refused { .. }
        ));
        assert_eq!(fixture.count("turn/start"), 0);
    }

    #[test]
    fn stale_generation_and_changed_replay_cannot_send() {
        let fixture = Fixture::new("normal");
        let mut adapter = fixture.connect();
        adapter.capabilities.start = true;
        let mut question = question(DeliveryMode::Start);
        question.generation = 6;
        assert!(matches!(
            adapter.submit(&question),
            DeliveryOutcome::Refused { .. }
        ));
        assert_eq!(fixture.count("turn/start"), 0);
        question.generation = 7;
        assert_eq!(adapter.submit(&question), DeliveryOutcome::Submitted);
        question.text = "Different payload".into();
        assert!(matches!(
            adapter.submit(&question),
            DeliveryOutcome::Refused { .. }
        ));
        assert_eq!(fixture.count("turn/start"), 1);
    }

    #[test]
    fn fragmented_response_obeys_absolute_rpc_deadline() {
        use std::io::Write;
        use std::time::Instant;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("drip.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            socket.read().unwrap();
            let response = b"{\"id\":1,\"result\":{\"userAgent\":\"probe\"}}";
            socket
                .get_mut()
                .write_all(&[0x81, response.len() as u8])
                .unwrap();
            for byte in response {
                if socket.get_mut().write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        let result = CodexAdapter::connect(&path, "thread", 1, Duration::from_millis(60));
        let elapsed = started.elapsed();
        task.join().unwrap();
        assert!(result.is_err());
        assert!(
            elapsed < Duration::from_millis(300),
            "RPC exceeded its absolute deadline: {elapsed:?}"
        );
    }

    #[test]
    fn fragmented_handshake_obeys_absolute_deadline() {
        use std::io::{self, Read, Write};
        use std::os::unix::net::UnixStream;
        use std::time::Instant;
        struct DripWriter(UnixStream);
        impl Read for DripWriter {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                self.0.read(bytes)
            }
        }
        impl Write for DripWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                for byte in bytes {
                    self.0.write_all(&[*byte])?;
                    thread::sleep(Duration::from_millis(20));
                }
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.0.flush()
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("handshake.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let _ = tungstenite::accept(DripWriter(stream));
        });
        let started = Instant::now();
        let result = CodexAdapter::connect(&path, "thread", 1, Duration::from_millis(60));
        let elapsed = started.elapsed();
        task.join().unwrap();
        assert!(result.is_err());
        assert!(
            elapsed < Duration::from_millis(300),
            "handshake exceeded its absolute deadline: {elapsed:?}"
        );
    }

    #[test]
    fn fragmented_event_does_not_block_lifecycle_polling() {
        use std::io::Write;
        use std::time::Instant;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            while let Ok(Message::Text(text)) = socket.read() {
                let request: Value = serde_json::from_str(&text).unwrap();
                let result = match request["method"].as_str().unwrap() {
                    "initialize" => json!({"userAgent":"test"}),
                    "initialized" => continue,
                    "server/diagnostics" => json!({"process":{"id":42}}),
                    "thread/resume" => json!({"thread":{"id":"thread"}}),
                    _ => unreachable!(),
                };
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .unwrap();
                if request["method"] == "thread/resume" {
                    break;
                }
            }
            let event = b"{\"method\":\"turn/started\",\"params\":{\"threadId\":\"thread\",\"turn\":{\"id\":\"turn\"}}}";
            if socket
                .get_mut()
                .write_all(&[0x81, event.len() as u8])
                .is_err()
            {
                return;
            }
            for byte in event {
                if socket.get_mut().write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let mut adapter =
            CodexAdapter::connect(&path, "thread", 1, Duration::from_millis(60)).unwrap();
        let started = Instant::now();
        assert!(adapter.poll_events().unwrap().is_empty());
        assert!(started.elapsed() < Duration::from_millis(100));
        drop(adapter);
        task.join().unwrap();
    }

    #[test]
    fn full_unix_listener_backlog_fails_promptly() {
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("full.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // SAFETY: the valid listener descriptor is reconfigured to the smallest queue.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let queued = UnixStream::connect(&path).unwrap();
        let (sender, receiver) = mpsc::channel();
        let task = thread::spawn(move || {
            let result = CodexAdapter::connect(&path, "thread", 1, Duration::from_millis(60));
            let _ = sender.send(result.is_err());
        });
        let bounded = receiver.recv_timeout(Duration::from_millis(250));
        // Release a previously blocking connect before asserting, so failure never leaks a thread.
        drop(listener.accept().unwrap());
        drop(queued);
        drop(listener);
        task.join().unwrap();
        assert_eq!(
            bounded.ok(),
            Some(true),
            "full backlog blocked connect beyond its deadline"
        );
    }

    #[test]
    fn slow_native_reader_cannot_extend_write_deadline() {
        use std::io::{Read, Write};
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        use std::time::Instant;
        let (server, mut peer) = UnixStream::pair().unwrap();
        let buffer_size: libc::c_int = 4096;
        // SAFETY: a live descriptor and correctly sized socket-option pointer are supplied.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    server.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&buffer_size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&buffer_size) as libc::socklen_t,
                )
            },
            0
        );
        let reader = thread::spawn(move || {
            let mut bytes = [0; 2048];
            while matches!(peer.read(&mut bytes), Ok(count) if count > 0) {
                thread::sleep(Duration::from_millis(10));
            }
        });
        let started = Instant::now();
        let mut stream = super::DeadlineStream {
            stream: server,
            deadline: started + Duration::from_millis(60),
        };
        let result = stream.write_all(&vec![b'x'; 256 * 1024]);
        let elapsed = started.elapsed();
        drop(stream);
        reader.join().unwrap();
        assert!(
            elapsed < Duration::from_millis(250),
            "native write escaped deadline: {elapsed:?}"
        );
        assert!(result.is_err());
    }
}
