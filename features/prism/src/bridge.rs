//! Authenticated, conversation-bound operations used by native CLI extensions.
use crate::actions::{ContentAction, PublishRequest};
use crate::model::{DeliveryMode, Id, MAX_PAYLOAD_BYTES};
use crate::session::{LiveBinding, TmuxClient, process_start_time, read_binding, write_binding};
use crate::store::Store;
use crate::ui::ConnectionInfo;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: u16 = 1;
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RESPONSE_LIMIT: usize = 4 * MAX_PAYLOAD_BYTES;
const LIST_LIMIT: usize = 100;

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
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectParams { pub document_id: Option<Id> }

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionParams {
    pub document_id: Id,
    pub expected_revision: u64,
    pub action: ContentAction,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionParams { pub request_id: Option<Id> }

pub fn socket_path(binding_path: &Path) -> Result<PathBuf> {
    Ok(binding_path.parent().context("binding directory missing")?.join("bridge.sock"))
}

pub fn store_path(binding: &LiveBinding) -> PathBuf {
    binding.project.join(".prism/state.db")
}

pub fn wait_for_binding(path: &Path) -> Result<LiveBinding> {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        match read_binding(path) {
            Ok(binding) => return Ok(binding),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error).context("no verified Prism binding; launch or resume with prism --agent codex"),
        }
    }
}

/// Authentication precedes identity changes: an invalid credential cannot retire a binding.
pub fn authorize(binding: &mut LiveBinding, token: &str, call: &BridgeCall) -> Result<()> {
    ensure!(!binding.retired, "binding retired; explicitly resume the conversation");
    let difference = binding.token.as_bytes().iter().zip(token.as_bytes()).fold(0u8, |acc, (a,b)| acc | (a ^ b));
    ensure!(binding.token.len() == 64 && token.len() == 64 && difference == 0, "invalid binding credential");
    ensure!(call.version == PROTOCOL_VERSION, "unsupported bridge protocol version");
    ensure!(call.generation == binding.generation, "stale binding generation");
    let thread = Id::parse_str(&call.thread_id).context("native thread ID is invalid")?;
    ensure!(!thread.is_nil() && thread.to_string() == call.thread_id, "native thread ID must be canonical");
    binding.bind_conversation(&call.thread_id)?;
    Ok(())
}

/// Only explicit public content and shared questions are exposed; never ViewState.
pub fn apply_operation(store: &mut Store, binding: &LiveBinding, operation: Operation) -> Result<Value> {
    let workspace = store.workspace(&binding.workspace_key()?)?;
    match operation {
        Operation::Publish(request) => Ok(serde_json::to_value(store.publish(workspace.id, &request)?)?),
        Operation::Inspect(InspectParams { document_id: Some(id) }) => Ok(serde_json::to_value(store.document(workspace.id, id)?)?),
        Operation::Inspect(InspectParams { document_id: None }) => {
            let documents = store.documents(workspace.id)?;
            let total = documents.len();
            Ok(json!({"documents": documents.into_iter().take(LIST_LIMIT).map(|d| json!({"id":d.id,"title":d.title,"revision":d.revision})).collect::<Vec<_>>(), "total": total}))
        }
        Operation::Action(params) => Ok(serde_json::to_value(store.apply(workspace.id, params.document_id, params.expected_revision, &params.action)?)?),
        Operation::Questions(QuestionParams { request_id: Some(id) }) => {
            let question = store.question(workspace.id, id)?;
            ensure!(question.mode == DeliveryMode::Share, "question was not explicitly shared for CLI retrieval");
            Ok(serde_json::to_value(question)?)
        }
        Operation::Questions(QuestionParams { request_id: None }) => {
            let questions = store.questions(workspace.id)?;
            Ok(json!({"questions":questions.into_iter().filter(|q|q.mode == DeliveryMode::Share).take(LIST_LIMIT).map(|q|json!({"request_id":q.request_id,"document_id":q.snapshot.selection.document_id,"revision":q.snapshot.selection.revision,"text":q.text,"outcome":q.outcome})).collect::<Vec<_>>()}))
        }
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
            .no_proxy().unix_socket(socket_path(binding_path)?)
            .timeout(IO_TIMEOUT).redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never()).build()?;
        Ok(Self { binding_path: binding_path.to_owned(), client })
    }

    pub fn call(&self, thread_id: &str, operation: Operation) -> Result<Value> {
        let binding = read_binding(&self.binding_path)?;
        ensure!(!binding.retired, "binding retired; explicitly resume the conversation");
        let call = BridgeCall { version: PROTOCOL_VERSION, generation: binding.generation, thread_id: thread_id.to_owned(), operation };
        let body = serde_json::to_vec(&call)?;
        ensure!(body.len() <= MAX_PAYLOAD_BYTES, "request exceeds payload limit");
        let response = self.client.post("http://localhost/call")
            .bearer_auth(&binding.token).header("Content-Type", "application/json").body(body).send()?;
        let status = response.status();
        let mut bytes = Vec::new();
        response.take(RESPONSE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= RESPONSE_LIMIT, "bridge response exceeds limit");
        let value: Value = serde_json::from_slice(&bytes)?;
        if !status.is_success() {
            bail!("{}", value.get("error").and_then(Value::as_str).unwrap_or("bridge request failed"));
        }
        Ok(value)
    }
}

struct Service {
    binding_path: PathBuf,
    executable: PathBuf,
    store: Store,
}

impl Service {
    fn handle(&mut self, token: &str, call: BridgeCall) -> Result<Value> {
        let mut binding = read_binding(&self.binding_path)?;
        let previous_generation = binding.generation;
        let previous_workspace = binding.workspace_key().ok();
        if let Err(error) = authorize(&mut binding, token, &call) {
            if binding.retired {
                write_binding(&self.binding_path, &binding)?;
                if let Some(key) = previous_workspace {
                    let workspace = self.store.workspace(&key)?;
                    self.store.invalidate_generation(workspace.id, previous_generation, "native conversation changed")?;
                }
            }
            return Err(error);
        }
        let tmux = TmuxClient::socket(&binding.tmux.socket_path);
        tmux.verify_binding(&binding)?;
        write_binding(&self.binding_path, &binding)?;
        let data = apply_operation(&mut self.store, &binding, call.operation)?;
        // Publication is already committed. Pane failure must not roll it back or replay it.
        let pane = match tmux.ensure_ui(&mut binding, &self.binding_path, &self.executable) {
            Ok(pane) => json!({"id":pane.pane_id}),
            Err(error) => json!({"error":error.to_string()}),
        };
        Ok(json!({"data":data,"pane":pane}))
    }
}

fn retire(path: &Path, store: &mut Store, reason: &str) -> Result<()> {
    let mut binding = read_binding(path)?;
    if binding.retired { return Ok(()); }
    let generation = binding.generation;
    if let Ok(key) = binding.workspace_key() {
        let workspace = store.workspace(&key)?;
        store.invalidate_generation(workspace.id, generation, reason)?;
    }
    binding.retire()?;
    write_binding(path, &binding)?;
    Ok(())
}

pub fn connection_info(path: &Path) -> ConnectionInfo {
    let state = read_binding(path).and_then(|binding| {
        TmuxClient::socket(&binding.tmux.socket_path).verify_binding(&binding)?;
        if !socket_path(path).map_err(std::io::Error::other)?.exists() {
            return Err(std::io::Error::other("Prism bridge is disconnected"));
        }
        Ok(binding)
    });
    match state {
        Ok(binding) if !binding.retired => ConnectionInfo { generation: binding.generation, connected: true,
            send_modes: vec![DeliveryMode::Share], message: "Share explicitly, then ask Codex to retrieve the Prism question".into() },
        Ok(binding) => ConnectionInfo { generation: binding.generation, connected: false, send_modes: vec![], message: "Binding retired; explicitly resume".into() },
        Err(error) => ConnectionInfo { generation: 0, connected: false, send_modes: vec![], message: error.to_string() },
    }
}

pub fn run(binding_path: &Path) -> Result<()> {
    let binding = wait_for_binding(binding_path)?;
    let directory = binding_path.parent().context("binding directory missing")?;
    let lease = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .mode(0o600).custom_flags(libc::O_NOFOLLOW).open(directory.join("bridge.lock"))?;
    lease.try_lock().context("a bridge already owns this binding")?;
    let socket = socket_path(binding_path)?;
    ensure!(!socket.exists(), "bridge socket already exists; use a fresh explicit launcher/resume binding");
    let server = tiny_http::Server::http_unix(&socket).map_err(|e|anyhow::anyhow!(e.to_string()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let mut service = Service { binding_path: binding_path.to_owned(), executable: std::env::current_exe()?, store: Store::open(store_path(&binding))? };
    let stopping = Arc::new(AtomicBool::new(false));
    let mut signals = Vec::new();
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT, signal_hook::consts::SIGHUP] {
        signals.push(signal_hook::flag::register(signal, stopping.clone())?);
    }
    let result = (|| -> Result<()> {
        while !stopping.load(Ordering::Relaxed) {
            let current = read_binding(binding_path)?;
            if current.retired || TmuxClient::socket(&current.tmux.socket_path).verify_binding(&current).is_err() {
                break;
            }
            let Some(mut request) = server.recv_timeout(POLL_INTERVAL)? else { continue; };
            let result = (|| -> Result<Value> {
                ensure!(request.method() == &tiny_http::Method::Post && request.url() == "/call", "unsupported endpoint");
                let token = request.headers().iter().find(|h|h.field.equiv("Authorization"))
                    .and_then(|h|h.value.as_str().strip_prefix("Bearer ")).unwrap_or("").to_owned();
                let mut bytes = Vec::new();
                request.as_reader().take(MAX_PAYLOAD_BYTES as u64 + 1).read_to_end(&mut bytes)?;
                ensure!(bytes.len() <= MAX_PAYLOAD_BYTES, "request exceeds payload limit");
                service.handle(&token, serde_json::from_slice(&bytes)?)
            })();
            let (status, value) = match result { Ok(value) => (200,value), Err(error) => (400,json!({"error":error.to_string()})) };
            let mut encoded = serde_json::to_vec(&value)?;
            let status = if encoded.len() > RESPONSE_LIMIT { encoded = serde_json::to_vec(&json!({"error":"response exceeds limit; request one document or question"}))?; 413 } else { status };
            let response = tiny_http::Response::from_data(encoded).with_status_code(status)
                .with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").expect("constant HTTP header"));
            // A disconnected caller does not invalidate an already committed publication.
            let _ = request.respond(response);
        }
        Ok(())
    })();
    let retirement = retire(binding_path, &mut service.store, "bridge disconnected");
    for signal in signals { signal_hook::low_level::unregister(signal); }
    drop(server);
    let _ = fs::remove_file(&socket);
    if process_start_time(binding.server_pid).ok().as_deref() == Some(binding.server_start_time.as_str()) {
        // This server was created exclusively for this launcher binding.
        unsafe { libc::kill(binding.server_pid as i32, libc::SIGTERM); }
    }
    result.and(retirement)
}
