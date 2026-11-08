//! Authenticated, conversation-bound operations used by native CLI extensions.
use crate::actions::{ContentAction, PublishRequest};
use crate::adapters::{AgentAdapter, NativeEvent, codex::CodexAdapter};
use crate::model::{DeliveryMode, Id, MAX_PAYLOAD_BYTES};
use crate::session::{LiveBinding, TmuxClient, process_start_time, read_binding, write_binding};
use crate::store::Store;
use crate::ui::ConnectionInfo;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
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
const RESPONSE_LIMIT: usize = 4 * MAX_PAYLOAD_BYTES;
const LIST_LIMIT: usize = 100;
const QUESTION_PREVIEW_CHARS: usize = 512;
const HEADER_LIMIT: usize = 16 * 1024;
const HEALTH_TIMEOUT: Duration = Duration::from_millis(500);

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

pub fn socket_path(binding_path: &Path) -> Result<PathBuf> {
    Ok(binding_path
        .parent()
        .context("binding directory missing")?
        .join("bridge.sock"))
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
                    "no verified Prism binding; launch or resume with prism --agent codex",
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
                    self.store.invalidate_generation(
                        workspace.id,
                        previous_generation,
                        "native conversation changed",
                    )?;
                }
            }
            return Err(error);
        }
        let tmux = TmuxClient::socket(&binding.tmux.socket_path);
        tmux.verify_binding(&binding)?;
        write_binding(&self.binding_path, &binding)?;
        binding = read_binding(&self.binding_path)?;
        authorize(&mut binding, token, &call)?;
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

fn healthy_binding(path: &Path) -> Result<LiveBinding> {
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
    Ok(binding)
}

pub fn connection_info(path: &Path) -> ConnectionInfo {
    match healthy_binding(path) {
        Ok(binding) => ConnectionInfo {
            generation: binding.generation,
            connected: true,
            send_modes: vec![DeliveryMode::Share],
            message: "Share explicitly, then ask Codex to retrieve the Prism question".into(),
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
        deadline: Instant::now() + timeout,
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

fn poll_native(observer: &mut Option<CodexAdapter>, binding: &LiveBinding) -> Result<()> {
    let Some(thread) = &binding.conversation else {
        return Ok(());
    };
    if observer.is_none() {
        let socket = binding
            .server_endpoint
            .strip_prefix("unix://")
            .context("native server must use a Unix socket")?;
        let adapter =
            CodexAdapter::connect(Path::new(socket), thread, binding.generation, IO_TIMEOUT)?;
        ensure!(
            adapter.server_pid == binding.server_pid,
            "native server process changed"
        );
        *observer = Some(adapter);
    }
    for event in observer
        .as_mut()
        .expect("observer initialized")
        .poll_events()?
    {
        if let NativeEvent::Retired { reason } = event {
            bail!("{reason}");
        }
    }
    Ok(())
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
    ensure!(
        !socket.exists(),
        "bridge socket already exists; use a fresh explicit launcher/resume binding"
    );
    let mut service = Service {
        binding_path: binding_path.to_owned(),
        executable: std::env::current_exe()?,
        store: Store::open(store_path(&binding))?,
    };
    let server = UnixListener::bind(&socket)?;
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
        let mut observer = None;
        while !stopping.load(Ordering::Relaxed) {
            let current = read_binding(binding_path)?;
            if current.retired
                || TmuxClient::socket(&current.tmux.socket_path)
                    .verify_binding(&current)
                    .is_err()
            {
                break;
            }
            poll_native(&mut observer, &current)?;
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
                    return Ok(
                        json!({"version":PROTOCOL_VERSION,"binding_id":current.binding_id,"generation":current.generation}),
                    );
                }
                service.handle(&request.token, serde_json::from_slice(&request.body)?)
            })();
            // A disconnected caller does not invalidate an already committed publication.
            let _ = respond(&stream, result, IO_TIMEOUT);
        }
        Ok(())
    })();
    let retirement = retire(binding_path, &mut service.store, "bridge disconnected");
    drop(server);
    drop(cleanup);
    if process_start_time(binding.server_pid).ok().as_deref()
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
    fn changed_native_thread_retires_persisted_binding_and_pending_question() {
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
        assert!(read_binding(&fixture.service.binding_path).unwrap().retired);
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
