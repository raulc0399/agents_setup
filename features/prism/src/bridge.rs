//! Authenticated, conversation-bound operations used by native CLI extensions.
use crate::actions::{ContentAction, PublishRequest};
use crate::adapters::{
    AgentAdapter, NativeEvent,
    claude::ClaudeAdapter,
    codex::{CodexAdapter, RpcError as CodexRpcError},
};
use crate::model::{AgentKind, DeliveryMode, Id, MAX_PAYLOAD_BYTES};
use crate::session::{LiveBinding, TmuxClient, process_start_time, read_binding, write_binding};
use crate::store::Store;
use crate::ui::ConnectionInfo;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: u16 = 1;
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const BINDING_VERIFY_RETRIES: u32 = 5;
const NATIVE_RETRY_INITIAL: Duration = Duration::from_secs(1);
const NATIVE_RETRY_MAX: Duration = Duration::from_secs(30);
const MAX_NATIVE_VERSION_CHARS: usize = 256;
const RESPONSE_LIMIT: usize = 4 * MAX_PAYLOAD_BYTES;
const LIST_LIMIT: usize = 100;
const QUESTION_PREVIEW_CHARS: usize = 512;
const HEADER_LIMIT: usize = 16 * 1024;
const HEADER_TIMEOUT: Duration = Duration::from_millis(200);
const HEALTH_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_LIFECYCLE_REASON_BYTES: usize = 64;
const LIFECYCLE_IS_NATIVE: &str = "lifecycle events are native for this agent";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeCall {
    pub version: u16,
    pub generation: u64,
    pub thread_id: String,
    pub operation: Operation,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Operation {
    Publish(PublishRequest),
    Inspect(InspectParams),
    Action(ActionParams),
    Questions(QuestionParams),
    Lifecycle(LifecycleEvent),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectParams {
    pub document_id: Option<Id>,
    pub cursor: Option<Id>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionParams {
    pub document_id: Id,
    pub expected_revision: u64,
    pub action: ContentAction,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionParams {
    pub request_id: Option<Id>,
    pub cursor: Option<Id>,
}

/// Native lifecycle observations reported by a CLI extension that has no server to poll.
#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum LifecycleEvent {
    SessionEnd { reason: String },
}

pub fn socket_path(binding_path: &Path) -> Result<PathBuf> {
    Ok(crate::session::socket_directory(binding_path)?.join("bridge.sock"))
}

pub fn store_path(binding: &LiveBinding) -> PathBuf {
    binding.project.join(".prism/state.db")
}

pub fn wait_for_binding(path: &Path) -> Result<LiveBinding> {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        match read_binding(path) {
            Ok(binding) => return Ok(binding),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                return Err(error).context(
                    "no verified Prism binding; launch or resume with prism --agent codex|claude",
                );
            }
        }
    }
}

/// Authentication precedes identity changes: an invalid credential cannot retire a binding.
pub fn authorize(binding: &mut LiveBinding, token: &str, call: &BridgeCall) -> Result<()> {
    ensure!(
        !binding.retired,
        "binding retired; explicitly resume the conversation"
    );
    authenticate(binding, token)?;
    ensure!(
        call.version == PROTOCOL_VERSION,
        "unsupported bridge protocol version"
    );
    ensure!(
        call.generation == binding.generation,
        "stale binding generation"
    );
    let thread = Id::parse_str(&call.thread_id).context("native thread ID is invalid")?;
    ensure!(
        !thread.is_nil() && thread.to_string() == call.thread_id,
        "native thread ID must be canonical"
    );
    binding.bind_conversation(&call.thread_id)?;
    Ok(())
}

fn authenticate(binding: &LiveBinding, token: &str) -> Result<()> {
    let difference = binding
        .token
        .as_bytes()
        .iter()
        .zip(token.as_bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    ensure!(
        !binding.retired && binding.token.len() == 64 && token.len() == 64 && difference == 0,
        "invalid or retired binding credential"
    );
    Ok(())
}

/// Only explicit public content and shared questions are exposed; never ViewState.
pub fn apply_operation(
    store: &mut Store,
    binding: &LiveBinding,
    operation: Operation,
) -> Result<Value> {
    let workspace = store.workspace(&binding.workspace_key()?)?;
    match operation {
        Operation::Publish(request) => Ok(serde_json::to_value(
            store.publish(workspace.id, &request)?,
        )?),
        Operation::Inspect(InspectParams {
            document_id: Some(id),
            cursor: None,
        }) => Ok(serde_json::to_value(store.document(workspace.id, id)?)?),
        Operation::Inspect(InspectParams {
            document_id: None,
            cursor,
        }) => {
            let documents = store.documents(workspace.id)?;
            let total = documents.len();
            let mut page = documents
                .into_iter()
                .filter(|document| cursor.is_none_or(|cursor| document.id > cursor))
                .take(LIST_LIMIT + 1)
                .collect::<Vec<_>>();
            let next_cursor = if page.len() > LIST_LIMIT {
                page.pop();
                page.last().map(|document| document.id)
            } else {
                None
            };
            Ok(
                json!({"documents":page.into_iter().map(|d| json!({"id":d.id,"title":d.title,"revision":d.revision})).collect::<Vec<_>>(), "total": total, "next_cursor":next_cursor}),
            )
        }
        Operation::Action(params) => Ok(serde_json::to_value(store.apply(
            workspace.id,
            params.document_id,
            params.expected_revision,
            &params.action,
        )?)?),
        Operation::Questions(QuestionParams {
            request_id: Some(id),
            cursor: None,
        }) => {
            let question = store.question(workspace.id, id)?;
            ensure!(
                question.mode == DeliveryMode::Share,
                "question was not explicitly shared for CLI retrieval"
            );
            Ok(serde_json::to_value(question)?)
        }
        Operation::Questions(QuestionParams {
            request_id: None,
            cursor,
        }) => {
            let questions = store
                .questions(workspace.id)?
                .into_iter()
                .filter(|question| question.mode == DeliveryMode::Share)
                .collect::<Vec<_>>();
            let total = questions.len();
            let mut page = questions
                .into_iter()
                .filter(|question| cursor.is_none_or(|cursor| question.request_id > cursor))
                .take(LIST_LIMIT + 1)
                .collect::<Vec<_>>();
            let mut next_cursor = if page.len() > LIST_LIMIT {
                page.pop();
                page.last().map(|question| question.request_id)
            } else {
                None
            };
            let mut summaries = Vec::new();
            let mut encoded_bytes = HEADER_LIMIT;
            let mut last_id = None;
            for question in page {
                let preview: String = question.text.chars().take(QUESTION_PREVIEW_CHARS).collect();
                let summary = json!({"request_id":question.request_id,"document_id":question.snapshot.selection.document_id,"revision":question.snapshot.selection.revision,"text_truncated":preview.len() < question.text.len(),"text":preview,"outcome":question.outcome});
                let summary_bytes = serde_json::to_vec(&summary)?.len() + 1;
                if encoded_bytes + summary_bytes > RESPONSE_LIMIT {
                    ensure!(
                        last_id.is_some(),
                        "question summary exceeds response limit; retrieve the question by ID"
                    );
                    next_cursor = last_id;
                    break;
                }
                encoded_bytes += summary_bytes;
                last_id = Some(question.request_id);
                summaries.push(summary);
            }
            Ok(json!({"questions":summaries, "total":total, "next_cursor":next_cursor}))
        }
        Operation::Inspect(_) | Operation::Questions(_) => {
            bail!("cursor is only valid when listing summaries")
        }
        Operation::Lifecycle(_) => bail!("lifecycle events are not content operations"),
    }
}

pub struct BridgeClient {
    binding_path: PathBuf,
    client: reqwest::blocking::Client,
}

impl BridgeClient {
    pub fn new(binding_path: &Path) -> Result<Self> {
        wait_for_binding(binding_path)?;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .unix_socket(socket_path(binding_path)?)
            .timeout(IO_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()?;
        Ok(Self {
            binding_path: binding_path.to_owned(),
            client,
        })
    }

    pub fn call(&self, thread_id: &str, operation: Operation) -> Result<Value> {
        let binding = read_binding(&self.binding_path)?;
        ensure!(
            !binding.retired,
            "binding retired; explicitly resume the conversation"
        );
        let call = BridgeCall {
            version: PROTOCOL_VERSION,
            generation: binding.generation,
            thread_id: thread_id.to_owned(),
            operation,
        };
        let body = serde_json::to_vec(&call)?;
        ensure!(
            body.len() <= MAX_PAYLOAD_BYTES,
            "request exceeds payload limit"
        );
        let response = self
            .client
            .post("http://localhost/call")
            .bearer_auth(&binding.token)
            .header("Content-Type", "application/json")
            .body(body)
            .send()?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(RESPONSE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= RESPONSE_LIMIT,
            "bridge response exceeds limit"
        );
        let value: Value = serde_json::from_slice(&bytes)?;
        if !status.is_success() {
            bail!(
                "{}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("bridge request failed")
            );
        }
        Ok(value)
    }
}

/// Bridge-owned native observation, scoped to the bound conversation.
enum NativeObserver {
    Codex(CodexObserver),
    Claude(ClaudeAdapter),
}

impl NativeObserver {
    fn connect(binding: &LiveBinding, conversation: &str) -> Result<Self> {
        match binding.agent {
            AgentKind::Codex => Ok(Self::Codex(CodexObserver {
                adapter: None,
                connecting: None,
                retry_at: Instant::now(),
                retry_delay: NATIVE_RETRY_INITIAL,
            })),
            AgentKind::Claude => Ok(Self::Claude(ClaudeAdapter::new(
                conversation,
                binding.generation,
            )?)),
        }
    }

    fn connected(&self) -> bool {
        match self {
            Self::Codex(observer) => observer.adapter.is_some(),
            Self::Claude(_) => true,
        }
    }
}

struct CodexObserver {
    adapter: Option<Box<CodexAdapter>>,
    connecting: Option<std::thread::JoinHandle<Result<CodexAdapter>>>,
    retry_at: Instant,
    retry_delay: Duration,
}

impl CodexObserver {
    fn connect(binding: &LiveBinding) -> Result<CodexAdapter> {
        let socket = binding
            .server_endpoint
            .strip_prefix("unix://")
            .context("native server must use a Unix socket")?;
        let adapter = CodexAdapter::connect(
            Path::new(socket),
            binding
                .conversation
                .as_deref()
                .context("native thread missing")?,
            binding.generation,
            IO_TIMEOUT,
        )?;
        ensure!(
            adapter.server_pid == binding.server_pid,
            "native server process changed"
        );
        let version: String = adapter
            .server_user_agent
            .chars()
            .take(MAX_NATIVE_VERSION_CHARS)
            .collect();
        eprintln!("prism: Codex observer connected; app-server userAgent={version:?}");
        Ok(adapter)
    }

    fn disconnect(&mut self, reason: &str) {
        self.adapter = None;
        self.retry_at = Instant::now() + self.retry_delay;
        eprintln!(
            "prism: Codex observer disconnected ({reason}); retry in {:?}",
            self.retry_delay
        );
        self.retry_delay = (self.retry_delay * 2).min(NATIVE_RETRY_MAX);
    }

    fn poll(&mut self, binding: &LiveBinding) -> Option<String> {
        if self
            .connecting
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            let result = self
                .connecting
                .take()
                .expect("finished connection")
                .join()
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!("Codex observer connection worker failed"))
                });
            match result {
                Ok(adapter) => self.adapter = Some(Box::new(adapter)),
                Err(error) => {
                    if matches!(error.downcast_ref::<CodexRpcError>(), Some(CodexRpcError::ThreadClosed { server_pid }) if *server_pid == binding.server_pid)
                    {
                        return Some(error.to_string());
                    }
                    self.disconnect(&error.to_string());
                }
            }
        }
        if let Some(adapter) = &mut self.adapter {
            let result = adapter.poll_events();
            if adapter.thread_closed() {
                return adapter.retirement_reason().map(str::to_owned);
            }
            let reason = adapter
                .retirement_reason()
                .map(str::to_owned)
                .or_else(|| result.err().map(|error| error.to_string()));
            if let Some(reason) = reason {
                self.disconnect(&reason);
            } else {
                self.retry_delay = NATIVE_RETRY_INITIAL;
            }
        }
        if self.adapter.is_none() && self.connecting.is_none() && Instant::now() >= self.retry_at {
            // A stalled handshake/resume must not block bridge health or pane lifecycle checks.
            let binding = binding.clone();
            match std::thread::Builder::new()
                .name("codex-observer".into())
                .spawn(move || Self::connect(&binding))
            {
                Ok(task) => self.connecting = Some(task),
                Err(error) => self.disconnect(&error.to_string()),
            }
        }
        None
    }
}

struct Service {
    binding_path: PathBuf,
    executable: PathBuf,
    store: Store,
    observer: Option<NativeObserver>,
    /// The native reason for retirement, kept even when the CLI pane dies before the next poll.
    native_retirement: Option<String>,
}

impl Service {
    fn handle(&mut self, token: &str, call: BridgeCall) -> Result<Value> {
        let mut binding = read_binding(&self.binding_path)?;
        // The MCP precheck can race the first caller that binds this serialized service.
        if binding.agent == AgentKind::Codex {
            authenticate(&binding, token)?;
            ensure!(
                binding
                    .conversation
                    .as_deref()
                    .is_none_or(|thread| thread == call.thread_id),
                "Only the bound Codex thread may use this Prism binding"
            );
        }
        let previous_generation = binding.generation;
        let previous_workspace = binding.workspace_key().ok();
        if let Err(error) = authorize(&mut binding, token, &call) {
            if binding.retired {
                write_binding(&self.binding_path, &binding)?;
                if let Some(key) = previous_workspace {
                    let workspace = self.store.workspace(&key)?;
                    self.store.invalidate_generation(
                        workspace.id,
                        previous_generation,
                        "native conversation changed",
                    )?;
                }
            }
            return Err(error);
        }
        // Lifecycle reports are Claude-only; refuse before any binding state is persisted.
        if matches!(call.operation, Operation::Lifecycle(_)) {
            ensure!(binding.agent == AgentKind::Claude, LIFECYCLE_IS_NATIVE);
        }
        let tmux = TmuxClient::socket(&binding.tmux.socket_path);
        tmux.verify_binding(&binding)?;
        write_binding(&self.binding_path, &binding)?;
        binding = read_binding(&self.binding_path)?;
        authorize(&mut binding, token, &call)?;
        if let Operation::Lifecycle(event) = &call.operation {
            // The bridge loop retires the binding once the observer reports the event.
            self.observe_lifecycle(event)?;
            return Ok(json!({"data":{"retiring":true}}));
        }
        let data = apply_operation(&mut self.store, &binding, call.operation)?;
        // Publication is already committed. Pane failure must not roll it back or replay it.
        let pane = match tmux.ensure_ui(&mut binding, &self.binding_path, &self.executable) {
            Ok(pane) => json!({"id":pane.pane_id}),
            Err(error) => json!({"error":error.to_string()}),
        };
        Ok(json!({"data":data,"pane":pane}))
    }

    /// Only Claude reports lifecycle over the bridge; Codex lifecycle is observed natively.
    fn observe_lifecycle(&mut self, event: &LifecycleEvent) -> Result<()> {
        let LifecycleEvent::SessionEnd { reason } = event;
        // The reason is rendered and persisted verbatim, so only identifier characters pass.
        ensure!(
            reason.len() <= MAX_LIFECYCLE_REASON_BYTES
                && reason
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
            "lifecycle reason must be a short lowercase identifier"
        );
        // The bridge loop creates the pre-bound Claude observer before it accepts any call.
        let Some(NativeObserver::Claude(adapter)) = &mut self.observer else {
            bail!(LIFECYCLE_IS_NATIVE);
        };
        adapter.observe_session_end(reason);
        self.native_retirement = adapter.retired().map(str::to_owned);
        Ok(())
    }
}

fn retire(path: &Path, store: &mut Store, reason: &str) -> Result<()> {
    let mut binding = read_binding(path)?;
    if binding.retired {
        return Ok(());
    }
    let generation = binding.generation;
    if let Ok(key) = binding.workspace_key() {
        let workspace = store.workspace(&key)?;
        store.invalidate_generation(workspace.id, generation, reason)?;
    }
    binding.retire()?;
    write_binding(path, &binding)?;
    Ok(())
}

fn healthy_binding(path: &Path) -> Result<(LiveBinding, bool)> {
    let binding = read_binding(path)?;
    ensure!(!binding.retired, "Binding retired; explicitly resume");
    TmuxClient::socket(&binding.tmux.socket_path).verify_binding(&binding)?;
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .unix_socket(socket_path(path)?)
        .timeout(HEALTH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()?;
    let response = client
        .get("http://localhost/health")
        .bearer_auth(&binding.token)
        .send()?;
    ensure!(
        response.status().is_success(),
        "Prism bridge is disconnected"
    );
    let mut bytes = Vec::new();
    response
        .take(HEADER_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= HEADER_LIMIT,
        "invalid bridge health response"
    );
    let response: Value = serde_json::from_slice(&bytes)?;
    ensure!(
        response["binding_id"] == binding.binding_id.to_string()
            && response["generation"] == binding.generation
            && response["version"] == PROTOCOL_VERSION,
        "bridge identity changed"
    );
    let native_connected =
        binding.agent != AgentKind::Codex || response["native_connected"].as_bool().unwrap_or(true);
    Ok((binding, native_connected))
}

/// Launcher readiness does not depend on the optional Codex lifecycle observer.
pub fn is_ready(path: &Path) -> bool {
    healthy_binding(path).is_ok()
}

pub fn connection_info(path: &Path) -> ConnectionInfo {
    match healthy_binding(path) {
        Ok((binding, true)) => ConnectionInfo {
            generation: binding.generation,
            connected: true,
            send_modes: vec![DeliveryMode::Share],
            message: format!(
                "Share explicitly, then ask {} to retrieve the Prism question",
                binding.agent.label()
            ),
        },
        Ok((binding, false)) => ConnectionInfo {
            generation: binding.generation,
            connected: false,
            send_modes: vec![],
            message: "Codex observer disconnected; reconnecting".into(),
        },
        Err(error) => ConnectionInfo {
            generation: 0,
            connected: false,
            send_modes: vec![],
            message: error.to_string(),
        },
    }
}

struct DeadlineReader<'a> {
    stream: &'a UnixStream,
    deadline: Instant,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "bridge request deadline exceeded")
            })?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}

struct DeadlineWriter<'a> {
    stream: &'a UnixStream,
    deadline: Instant,
}

impl DeadlineWriter<'_> {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "bridge response deadline exceeded")
            })
    }
}

impl Write for DeadlineWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        crate::io::write_with_deadline(self.stream, bytes, self.deadline)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.flush()
    }
}

struct HttpRequest {
    health: bool,
    token: String,
    body: Vec<u8>,
}

fn read_request(stream: &UnixStream, timeout: Duration) -> Result<HttpRequest> {
    let mut input = BufReader::new(DeadlineReader {
        stream,
        deadline: Instant::now() + timeout.min(HEADER_TIMEOUT),
    });
    let mut header = Vec::new();
    loop {
        let available = HEADER_LIMIT
            .checked_sub(header.len())
            .context("HTTP headers exceed limit")?;
        ensure!(available > 0, "HTTP headers exceed limit");
        let mut line = Vec::new();
        let count = (&mut input)
            .take(available as u64)
            .read_until(b'\n', &mut line)?;
        ensure!(count > 0 && line.ends_with(b"\r\n"), "invalid HTTP header");
        header.extend_from_slice(&line);
        if line == b"\r\n" {
            break;
        }
    }
    let header = std::str::from_utf8(&header)?;
    let mut lines = header.split("\r\n");
    let health = match lines.next() {
        Some("GET /health HTTP/1.1") => true,
        Some("POST /call HTTP/1.1") => false,
        _ => bail!("unsupported endpoint"),
    };
    let mut token = None;
    let mut length = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').context("invalid HTTP header")?;
        let value = value.trim();
        ensure!(
            !name.eq_ignore_ascii_case("transfer-encoding"),
            "transfer encoding is unsupported"
        );
        if name.eq_ignore_ascii_case("authorization") {
            ensure!(token.is_none(), "duplicate authorization header");
            token = Some(
                value
                    .strip_prefix("Bearer ")
                    .context("invalid binding credential")?
                    .to_owned(),
            );
        }
        if name.eq_ignore_ascii_case("content-length") {
            ensure!(length.is_none(), "duplicate content length");
            length = Some(value.parse::<usize>()?);
        }
    }
    let length = if health {
        length.unwrap_or(0)
    } else {
        length.context("content length required")?
    };
    ensure!(
        length <= MAX_PAYLOAD_BYTES && (!health || length == 0),
        "request exceeds payload limit"
    );
    let mut body = vec![0; length];
    input.get_mut().deadline = Instant::now() + timeout;
    input.read_exact(&mut body)?;
    Ok(HttpRequest {
        health,
        token: token.context("binding credential missing")?,
        body,
    })
}

fn respond(stream: &UnixStream, result: Result<Value>, timeout: Duration) -> Result<()> {
    let (status, value) = match result {
        Ok(value) => ("200 OK", value),
        Err(error) => ("400 Bad Request", json!({"error":error.to_string()})),
    };
    let mut encoded = serde_json::to_vec(&value)?;
    let status = if encoded.len() > RESPONSE_LIMIT {
        encoded = serde_json::to_vec(
            &json!({"error":"response exceeds limit; request one document or question"}),
        )?;
        "413 Payload Too Large"
    } else {
        status
    };
    let mut output = DeadlineWriter {
        stream,
        deadline: Instant::now() + timeout,
    };
    write!(
        output,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        encoded.len()
    )?;
    output.write_all(&encoded)?;
    Ok(())
}

struct Cleanup {
    socket: PathBuf,
    signals: Vec<signal_hook::SigId>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            signal_hook::low_level::unregister(signal);
        }
        let _ = fs::remove_file(&self.socket);
    }
}

/// Returns the native retirement reason once the observer reports that the conversation ended.
/// A transient tmux query failure must not retire a live binding and tear down the session;
/// only consecutive failures count as a lost binding.
pub fn binding_still_live(verified: bool, consecutive_failures: &mut u32) -> bool {
    if verified {
        *consecutive_failures = 0;
        return true;
    }
    *consecutive_failures += 1;
    *consecutive_failures < BINDING_VERIFY_RETRIES
}

fn poll_native(
    observer: &mut Option<NativeObserver>,
    binding: &LiveBinding,
) -> Result<Option<String>> {
    let Some(thread) = &binding.conversation else {
        return Ok(None);
    };
    if observer.is_none() {
        *observer = Some(NativeObserver::connect(binding, thread)?);
    }
    let adapter = match observer.as_mut().expect("observer initialized") {
        NativeObserver::Codex(observer) => return Ok(observer.poll(binding)),
        NativeObserver::Claude(adapter) => adapter,
    };
    for event in adapter.poll_events()? {
        if let NativeEvent::Retired { reason } = event {
            return Ok(Some(reason));
        }
    }
    Ok(None)
}

fn bind_listener(socket: &Path) -> Result<UnixListener> {
    if let Ok(metadata) = fs::symlink_metadata(socket) {
        ensure!(
            metadata.file_type().is_socket()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o7777 == 0o600,
            "invalid existing bridge socket"
        );
        match crate::adapters::codex::connect_unix(socket) {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                fs::remove_file(socket)?
            }
            _ => bail!("bridge socket is still live or cannot be verified"),
        }
    }
    Ok(UnixListener::bind(socket)?)
}

pub fn run(binding_path: &Path) -> Result<()> {
    let binding = wait_for_binding(binding_path)?;
    let directory = binding_path.parent().context("binding directory missing")?;
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("bridge.lock"))?;
    let metadata = lease.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.mode() & 0o7777 == 0o600
            && metadata.uid() == unsafe { libc::geteuid() },
        "invalid bridge lease permissions"
    );
    lease
        .try_lock()
        .context("a bridge already owns this binding")?;
    let socket = socket_path(binding_path)?;
    let mut service = Service {
        binding_path: binding_path.to_owned(),
        executable: std::env::current_exe()?,
        store: Store::open(store_path(&binding))?,
        observer: None,
        native_retirement: None,
    };
    let server = bind_listener(&socket)?;
    let mut cleanup = Cleanup {
        socket: socket.clone(),
        signals: Vec::new(),
    };
    let result = (|| -> Result<()> {
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        server.set_nonblocking(true)?;
        let stopping = Arc::new(AtomicBool::new(false));
        for signal in [
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGHUP,
        ] {
            cleanup
                .signals
                .push(signal_hook::flag::register(signal, stopping.clone())?);
        }
        let mut verify_failures = 0;
        while !stopping.load(Ordering::Relaxed) {
            let current = read_binding(binding_path)?;
            if current.retired {
                break;
            }
            let verified = TmuxClient::socket(&current.tmux.socket_path)
                .verify_binding(&current)
                .is_ok();
            if !binding_still_live(verified, &mut verify_failures) {
                break;
            }
            if !verified {
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            if let Some(reason) = poll_native(&mut service.observer, &current)? {
                service.native_retirement = Some(reason.clone());
                bail!("{reason}");
            }
            let stream = match server.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let result = (|| -> Result<Value> {
                let request = read_request(&stream, IO_TIMEOUT)?;
                if request.health {
                    let current = read_binding(binding_path)?;
                    authenticate(&current, &request.token)?;
                    TmuxClient::socket(&current.tmux.socket_path).verify_binding(&current)?;
                    let native_connected = current.conversation.is_none()
                        || service
                            .observer
                            .as_ref()
                            .is_some_and(NativeObserver::connected);
                    return Ok(
                        json!({"version":PROTOCOL_VERSION,"binding_id":current.binding_id,"generation":current.generation,"native_connected":native_connected}),
                    );
                }
                service.handle(&request.token, serde_json::from_slice(&request.body)?)
            })();
            // A disconnected caller does not invalidate an already committed publication.
            let _ = respond(&stream, result, IO_TIMEOUT);
        }
        Ok(())
    })();
    let retirement = retire(
        binding_path,
        &mut service.store,
        service
            .native_retirement
            .as_deref()
            .unwrap_or("bridge disconnected"),
    );
    drop(server);
    drop(cleanup);
    // Only Codex owns a private server; the Claude pane process belongs to the user's CLI.
    if binding.agent == AgentKind::Codex
        && process_start_time(binding.server_pid).ok().as_deref()
            == Some(binding.server_start_time.as_str())
    {
        // This server was created exclusively for this launcher binding.
        unsafe {
            libc::kill(binding.server_pid as i32, libc::SIGTERM);
        }
    }
    result.and(retirement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_recovery_does_not_block_on_a_full_listener() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bridge.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut clients = Vec::new();
        while let Ok(client) = crate::adapters::codex::connect_unix(&path) {
            clients.push(client);
        }
        let (send, receive) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            send.send(bind_listener(&path).is_err()).unwrap();
        });
        let result = receive.recv_timeout(Duration::from_millis(200));
        let _accepted = listener.accept().unwrap();
        task.join().unwrap();
        assert!(result.unwrap());
    }

    #[test]
    fn bridge_listener_recovers_only_a_refused_socket() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bridge.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(bind_listener(&path).is_err());
        drop(listener);
        assert!(bind_listener(&path).is_ok());
    }

    #[test]
    fn slow_header_has_a_short_deadline() {
        let (server, _client) = UnixStream::pair().unwrap();
        let started = Instant::now();
        assert!(read_request(&server, Duration::from_millis(500)).is_err());
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    #[test]
    fn incomplete_body_has_a_total_deadline() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"POST /call HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 10\r\n\r\n{").unwrap();
        let start = Instant::now();
        assert!(read_request(&server, Duration::from_millis(30)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn headers_and_body_have_explicit_bounds() {
        for request in [
            format!("POST /call HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: {}\r\n\r\n", MAX_PAYLOAD_BYTES + 1),
            "POST /call HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n".into(),
            "POST /call HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n".into(),
            format!("GET /health HTTP/1.1\r\nX-Fill: {}\r\n\r\n", "x".repeat(HEADER_LIMIT)),
        ] {
            let (server, mut client) = UnixStream::pair().unwrap();
            client.write_all(request.as_bytes()).unwrap();
            assert!(read_request(&server, Duration::from_secs(1)).is_err());
        }
    }

    #[test]
    fn request_and_response_round_trip_with_content_length() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"POST /call HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: 2\r\n\r\n{}").unwrap();
        let request = read_request(&server, Duration::from_secs(1)).unwrap();
        assert!(!request.health);
        assert_eq!(request.token, "secret");
        assert_eq!(request.body, b"{}");
        respond(&server, Ok(json!({"result":"done"})), IO_TIMEOUT).unwrap();
        drop(server);
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.ends_with("{\"result\":\"done\"}"));
    }

    #[test]
    fn oversized_response_is_explicitly_rejected() {
        let (server, mut client) = UnixStream::pair().unwrap();
        respond(
            &server,
            Ok(json!({"large":"x".repeat(RESPONSE_LIMIT)})),
            IO_TIMEOUT,
        )
        .unwrap();
        drop(server);
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
        assert!(response.len() < HEADER_LIMIT);
    }

    #[test]
    fn codex_observer_connection_failure_keeps_binding_live() {
        let mut fixture = Fixture::new();
        fixture.binding.bind_conversation("thread").unwrap();
        let result = poll_codex_connection(&mut fixture.service.observer, &fixture.binding);
        assert!(matches!(result, Ok(None)), "{result:?}");
        assert!(!fixture.binding.retired);
        let Some(NativeObserver::Codex(observer)) = &fixture.service.observer else {
            panic!("Codex observer missing");
        };
        assert!(observer.adapter.is_none());
        assert!(observer.connecting.is_none());
        assert!(observer.retry_at > Instant::now());
    }

    fn poll_codex_connection(
        observer: &mut Option<NativeObserver>,
        binding: &LiveBinding,
    ) -> Result<Option<String>> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let result = poll_native(observer, binding)?;
            if result.is_some()
                || matches!(observer, Some(NativeObserver::Codex(observer)) if observer.connecting.is_none())
            {
                return Ok(result);
            }
            assert!(
                Instant::now() < deadline,
                "observer connection did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn codex_test_connection(
        listener: UnixListener,
        before_resume: Option<Value>,
    ) -> tungstenite::WebSocket<UnixStream> {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        while let Ok(tungstenite::Message::Text(text)) = socket.read() {
            let request: Value = serde_json::from_str(&text).unwrap();
            if request["method"] == "thread/resume"
                && let Some(event) = &before_resume
            {
                socket
                    .send(tungstenite::Message::Text(event.to_string().into()))
                    .unwrap();
                return socket;
            }
            let result = match request["method"].as_str().unwrap() {
                "initialize" => json!({"userAgent":"test/0.167.0"}),
                "initialized" => continue,
                "server/diagnostics" => json!({"process":{"id":std::process::id()}}),
                "thread/resume" => json!({"thread":{"id":"thread"}}),
                method => panic!("unexpected method {method}"),
            };
            socket
                .send(tungstenite::Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .unwrap();
            if request["method"] == "thread/resume" {
                return socket;
            }
        }
        panic!("observer disconnected before resume");
    }

    fn codex_observer_event(event: Value) -> Option<String> {
        let mut fixture = Fixture::new();
        fixture.binding.bind_conversation("thread").unwrap();
        let socket_path = fixture._project.path().join("native.sock");
        fixture.binding.server_endpoint = format!("unix://{}", socket_path.display());
        let listener = UnixListener::bind(socket_path).unwrap();
        let task = std::thread::spawn(move || {
            let mut socket = codex_test_connection(listener, None);
            if !event.is_null() {
                socket
                    .send(tungstenite::Message::Text(event.to_string().into()))
                    .unwrap();
            }
            socket.close(None).unwrap();
        });
        poll_native(&mut fixture.service.observer, &fixture.binding).unwrap();
        task.join().unwrap();
        let result = poll_codex_connection(&mut fixture.service.observer, &fixture.binding);
        result.unwrap()
    }

    #[test]
    fn codex_observer_reconnects_after_backoff_without_rebinding() {
        let mut fixture = Fixture::new();
        fixture.binding.bind_conversation("thread").unwrap();
        let path = fixture._project.path().join("native.sock");
        fixture.binding.server_endpoint = format!("unix://{}", path.display());
        let generation = fixture.binding.generation;
        let observer = &mut fixture.service.observer;
        assert!(
            poll_codex_connection(observer, &fixture.binding)
                .unwrap()
                .is_none()
        );
        let listener = UnixListener::bind(path).unwrap();
        assert!(poll_native(observer, &fixture.binding).unwrap().is_none());
        let Some(NativeObserver::Codex(state)) = observer else {
            panic!("missing observer");
        };
        assert!(state.connecting.is_none(), "must wait for backoff");
        state.retry_at = Instant::now();
        let task = std::thread::spawn(move || codex_test_connection(listener, None));
        poll_native(observer, &fixture.binding).unwrap();
        let _socket = task.join().unwrap();
        assert!(
            poll_codex_connection(observer, &fixture.binding)
                .unwrap()
                .is_none()
        );
        assert!(observer.as_ref().unwrap().connected());
        assert_eq!(fixture.binding.generation, generation);
        assert_eq!(fixture.binding.conversation.as_deref(), Some("thread"));
    }

    #[test]
    fn codex_observer_close_during_resume_checks_server_identity() {
        for server_pid in [std::process::id(), u32::MAX] {
            let mut fixture = Fixture::new();
            fixture.binding.bind_conversation("thread").unwrap();
            fixture.binding.server_pid = server_pid;
            let path = fixture._project.path().join("native.sock");
            fixture.binding.server_endpoint = format!("unix://{}", path.display());
            let listener = UnixListener::bind(path).unwrap();
            let task = std::thread::spawn(move || {
                codex_test_connection(
                    listener,
                    Some(json!({"method":"thread/closed","params":{"threadId":"thread"}})),
                )
            });
            poll_native(&mut fixture.service.observer, &fixture.binding).unwrap();
            let _socket = task.join().unwrap();
            let result =
                poll_codex_connection(&mut fixture.service.observer, &fixture.binding).unwrap();
            assert_eq!(result.is_some(), server_pid == std::process::id());
        }
    }

    #[test]
    fn codex_observer_handshake_does_not_block_lifecycle_polling() {
        let mut fixture = Fixture::new();
        fixture.binding.bind_conversation("thread").unwrap();
        let path = fixture._project.path().join("native.sock");
        fixture.binding.server_endpoint = format!("unix://{}", path.display());
        let listener = UnixListener::bind(path).unwrap();
        poll_native(&mut fixture.service.observer, &fixture.binding).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        assert!(
            poll_native(&mut fixture.service.observer, &fixture.binding)
                .unwrap()
                .is_none()
        );
        assert!(started.elapsed() < Duration::from_millis(200));
        drop(stream);
        assert!(
            poll_codex_connection(&mut fixture.service.observer, &fixture.binding)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn codex_observer_socket_close_does_not_retire_binding() {
        assert!(codex_observer_event(Value::Null).is_none());
    }

    #[test]
    fn codex_observer_other_thread_does_not_retire_binding() {
        assert!(
            codex_observer_event(
                json!({"method":"thread/started","params":{"thread":{"id":"other"}}})
            )
            .is_none()
        );
    }

    #[test]
    fn codex_observer_unsubscribe_does_not_retire_binding() {
        assert!(
            codex_observer_event(
                json!({"method":"thread/unsubscribed","params":{"threadId":"thread"}})
            )
            .is_none()
        );
    }

    #[test]
    fn codex_observer_bound_thread_close_retires_binding() {
        assert!(
            codex_observer_event(json!({"method":"thread/closed","params":{"threadId":"thread"}}))
                .is_some()
        );
    }

    struct Fixture {
        _project: tempfile::TempDir,
        name: String,
        binding: LiveBinding,
        service: Service,
    }

    impl Fixture {
        fn new() -> Self {
            let project = tempfile::TempDir::new().unwrap();
            let runtime = crate::session::RuntimeDirectory::create(project.path()).unwrap();
            let name = format!("prism-service-test-{}", Id::new_v4());
            let tmux = TmuxClient::named(&name);
            let cli = tmux
                .launch_cli(project.path(), Path::new("/usr/bin/cat"), &[], None)
                .unwrap();
            let binding = LiveBinding::new(
                project.path(),
                AgentKind::Codex,
                "unix:///unused".into(),
                std::process::id(),
                tmux.identity().unwrap(),
                cli,
            )
            .unwrap();
            tmux.tag_cli(&binding).unwrap();
            runtime.write_binding(&binding).unwrap();
            let service = Service {
                binding_path: runtime.binding_path(),
                executable: project.path().join("missing-executable"),
                store: Store::open(store_path(&binding)).unwrap(),
                observer: None,
                native_retirement: None,
            };
            Self {
                _project: project,
                name,
                binding,
                service,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .args(["-L", &self.name, "kill-server"])
                .output();
        }
    }

    #[test]
    fn publication_survives_pane_failure_and_lost_response_retry() {
        let mut fixture = Fixture::new();
        let thread = Id::new_v4().to_string();
        let request = PublishRequest {
            request_id: Id::new_v4(),
            document_id: None,
            expected_revision: None,
            title: "Committed".into(),
            elements: vec![],
            relationships: vec![],
            groups: vec![],
        };
        let call = || BridgeCall {
            version: PROTOCOL_VERSION,
            generation: 1,
            thread_id: thread.clone(),
            operation: Operation::Publish(request.clone()),
        };
        let result = fixture
            .service
            .handle(&fixture.binding.token, call())
            .unwrap();
        assert!(result["pane"]["error"].is_string());
        assert_eq!(result["data"]["revision"], 1);
        assert_eq!(
            fixture
                .service
                .handle(&fixture.binding.token, call())
                .unwrap()["data"],
            result["data"]
        );
        let stored = read_binding(&fixture.service.binding_path).unwrap();
        assert_eq!(stored.conversation, Some(thread));
        let workspace = fixture
            .service
            .store
            .workspace(&stored.workspace_key().unwrap())
            .unwrap();
        assert_eq!(
            fixture.service.store.documents(workspace.id).unwrap().len(),
            1
        );
    }

    #[test]
    fn foreign_codex_thread_preserves_persisted_binding_and_shared_question() {
        use crate::actions::PrepareQuestion;
        use crate::model::{DeliveryOutcome, Selection};
        let mut fixture = Fixture::new();
        let thread = Id::new_v4().to_string();
        fixture.binding.bind_conversation(&thread).unwrap();
        write_binding(&fixture.service.binding_path, &fixture.binding).unwrap();
        let workspace = fixture
            .service
            .store
            .workspace(&fixture.binding.workspace_key().unwrap())
            .unwrap();
        let element = Id::new_v4();
        let document = fixture
            .service
            .store
            .publish(
                workspace.id,
                &PublishRequest {
                    request_id: Id::new_v4(),
                    document_id: None,
                    expected_revision: None,
                    title: "Doc".into(),
                    elements: vec![crate::actions::ElementInput {
                        id: element,
                        title: "Node".into(),
                        body: String::new(),
                        kind: crate::model::ElementKind::Text,
                        source: None,
                    }],
                    relationships: vec![],
                    groups: vec![],
                },
            )
            .unwrap();
        let question = fixture
            .service
            .store
            .prepare_question(
                workspace.id,
                &PrepareQuestion {
                    request_id: Id::new_v4(),
                    generation: 1,
                    selection: Selection {
                        document_id: document.document_id,
                        revision: 1,
                        ids: vec![element],
                    },
                    text: "Explicit question".into(),
                    mode: DeliveryMode::Share,
                },
            )
            .unwrap();
        let call = BridgeCall {
            version: PROTOCOL_VERSION,
            generation: 1,
            thread_id: Id::new_v4().to_string(),
            operation: Operation::Inspect(InspectParams {
                document_id: None,
                cursor: None,
            }),
        };
        assert!(
            fixture
                .service
                .handle(&fixture.binding.token, call)
                .is_err()
        );
        assert!(!read_binding(&fixture.service.binding_path).unwrap().retired);
        assert!(matches!(
            fixture
                .service
                .store
                .question(workspace.id, question.request_id)
                .unwrap()
                .outcome,
            DeliveryOutcome::Pending
        ));
    }

    #[test]
    fn lifecycle_wire_shape_is_flat_and_denies_unknown_fields() {
        let operation: Operation = serde_json::from_value(
            json!({"method":"lifecycle","params":{"event":"session_end","reason":"clear"}}),
        )
        .unwrap();
        assert!(matches!(
            operation,
            Operation::Lifecycle(LifecycleEvent::SessionEnd { ref reason }) if reason == "clear"
        ));
        assert!(
            serde_json::from_value::<Operation>(
                json!({"method":"lifecycle","params":{"event":"session_end","reason":"clear","pane":"%1"}}),
            )
            .is_err()
        );
    }

    #[test]
    fn codex_binding_rejects_lifecycle_events_without_retiring() {
        let mut fixture = Fixture::new();
        let thread = Id::new_v4().to_string();
        let call = BridgeCall {
            version: PROTOCOL_VERSION,
            generation: 1,
            thread_id: thread.clone(),
            operation: Operation::Lifecycle(LifecycleEvent::SessionEnd {
                reason: "clear".into(),
            }),
        };
        let error = fixture
            .service
            .handle(&fixture.binding.token, call)
            .unwrap_err();
        assert!(error.to_string().contains("native for this agent"));
        assert!(fixture.service.observer.is_none());
        // An operation invalid for the agent must not pre-bind the conversation either.
        let stored = read_binding(&fixture.service.binding_path).unwrap();
        assert!(!stored.retired);
        assert_eq!(stored.generation, 1);
        assert!(stored.conversation.is_none());
    }

    #[test]
    fn slow_response_reader_cannot_extend_total_write_deadline() {
        use std::os::fd::AsRawFd;
        let (server, mut client) = UnixStream::pair().unwrap();
        let buffer_size: libc::c_int = 4096;
        // SAFETY: the descriptor and pointer to the correctly sized socket option are valid.
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
        let reader = std::thread::spawn(move || {
            let mut bytes = [0; 2048];
            while matches!(client.read(&mut bytes), Ok(count) if count > 0) {
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let started = Instant::now();
        let result = respond(
            &server,
            Ok(json!({"text":"x".repeat(256 * 1024)})),
            Duration::from_millis(60),
        );
        let elapsed = started.elapsed();
        drop(server);
        reader.join().unwrap();
        assert!(
            elapsed < Duration::from_millis(250),
            "response escaped absolute deadline: {elapsed:?}"
        );
        assert!(result.is_err());
    }
}
