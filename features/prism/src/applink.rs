//! App link: the bridge's WebSocket connection to the Prism desktop app on the host.
//!
//! One link thread per bridge owns the socket and its own store connection, so app traffic
//! never blocks the bridge loop or a tool operation. The bridge starts it and, when the
//! session ends, hands it the `closed` reason over a channel. Before every connection attempt
//! the thread re-reads the binding and the pairing file, checks the binding every poll
//! interval until the conversation is bound, and tries every address the host name resolves
//! to with a connect timeout. While the file or the app is absent it retries with exponential
//! backoff and logs each distinct failure once to the bridge's own log, never to the CLI.
//!
//! Commits are noticed through SQLite's `data_version`, which changes whenever another
//! connection commits to `.prism/state.db`: the bridge's tool operations, the Claude mod's
//! prompt-box report and the separate terminal UI process all commit that way. On a change
//! the thread compares the content with what it last sent and sends the difference as
//! `update`, so commits made between two polls arrive in one `update`. Commands from the app
//! commit through the thread's own store and are followed by the same comparison before their
//! `reply`.
use crate::actions::{self, PrepareQuestion};
use crate::bridge::{ActionParams, Operation, apply_operation, store_path};
use crate::model::{
    AgentKind, ContentAction, DeliveryMode, Document, Id, Question, Selection, WorkspaceKey,
};
use crate::session::{LiveBinding, TmuxClient, read_binding, write_binding};
use crate::store::{BUSY_TIMEOUT_SECONDS, Store};
use anyhow::{Context, Result, anyhow, bail, ensure};
use prism_core::pairing::{
    APP_HOST_ENV, DEFAULT_APP_HOST, PAIRING_FILE, PAIRING_FILE_ENV, Pairing,
};
use prism_core::protocol::{
    self, APP_PROTOCOL, AppMessage, BridgeMessage, CliProcess, CommandError, MAX_MESSAGE_BYTES,
    Prover, Refusal, Session,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

const RETRY_INITIAL: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the bridge waits for the link thread at the end of the session; covers sending
/// `closed` and the close handshake.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
const CLOSE_POLL: Duration = Duration::from_millis(10);
/// Bound time of a binding written before it was recorded. The protocol never sends 0, and 1
/// loses to every recorded bind time, so a legacy binding never supersedes a newer one.
const LEGACY_BOUND_AT_MS: u64 = 1;
const WELCOME_TYPE: &str = "welcome";
const MAX_PAIRING_BYTES: u64 = 4096;
const CLAUDE_HOME: &str = ".claude";
const CODEX_HOME: &str = ".codex";
const HOSTNAME_FILE: &str = "/proc/sys/kernel/hostname";

/// Where the link dials and how often; tests shorten the intervals.
#[derive(Clone)]
pub struct LinkConfig {
    /// Pairing file; `None` reads the agent's own below the home directory.
    pub pairing_file: Option<PathBuf>,
    pub host: String,
    /// First wait after a failed attempt. It doubles up to `retry_max` and starts over when a
    /// connection that completed the handshake ends without a refusal or a protocol violation
    /// from the app.
    pub retry_initial: Duration,
    pub retry_max: Duration,
    /// How often a connected link checks the store for commits and the bridge for the end,
    /// and how often a link waiting for the conversation checks the binding.
    pub poll_interval: Duration,
}

impl LinkConfig {
    /// `PRISM_APP_FILE` and `PRISM_APP_HOST` override the defaults; empty values are ignored.
    pub fn from_env() -> Self {
        Self {
            pairing_file: std::env::var_os(PAIRING_FILE_ENV)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            host: std::env::var(APP_HOST_ENV)
                .ok()
                .filter(|host| !host.is_empty())
                .unwrap_or_else(|| DEFAULT_APP_HOST.to_owned()),
            retry_initial: RETRY_INITIAL,
            retry_max: RETRY_MAX,
            poll_interval: POLL_INTERVAL,
        }
    }

    fn pairing_path(&self, agent: &AgentKind) -> Option<PathBuf> {
        if let Some(path) = &self.pairing_file {
            return Some(path.clone());
        }
        let directory = match agent {
            AgentKind::Claude => CLAUDE_HOME,
            AgentKind::Codex => CODEX_HOME,
        };
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
        Some(PathBuf::from(home).join(directory).join(PAIRING_FILE))
    }
}

/// The bridge's handle on the link thread.
pub struct AppLink {
    close: Sender<String>,
    thread: JoinHandle<()>,
}

impl AppLink {
    pub fn start(binding_path: &Path, config: LinkConfig) -> io::Result<Self> {
        let (close, closing) = mpsc::channel();
        let mut link = Link {
            binding_path: binding_path.to_owned(),
            config,
            closing,
            last_failure: None,
            recovered: false,
        };
        let thread = std::thread::Builder::new()
            .name("app-link".into())
            .spawn(move || link.run())?;
        Ok(Self { close, thread })
    }

    /// Sends `closed` with `reason` when connected and waits at most `CLOSE_WAIT` for the
    /// thread. A thread still resolving the host or in a handshake is left to end on its own
    /// timeouts, so it never holds up the bridge's exit.
    pub fn close(self, reason: &str) {
        let Self { close, thread } = self;
        let _ = close.send(reason.to_owned());
        // A thread that took the reason but could not send it then sees the channel closed.
        drop(close);
        let deadline = Instant::now() + CLOSE_WAIT;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(CLOSE_POLL);
        }
        if thread.is_finished() {
            let _ = thread.join();
        }
    }
}

/// How one connection attempt ended.
enum Ended {
    /// The session ended or the bridge dropped its handle.
    Stopped,
    /// An app whose `welcome` proof verified gave the tab to a later binding.
    Superseded,
    /// The conversation is not bound yet: check again after the poll interval.
    Unbound,
    /// Retry after the backoff. `authenticated` (the handshake completed and the app neither
    /// refused nor broke the protocol) resets it.
    Lost {
        authenticated: bool,
        error: anyhow::Error,
    },
}

struct Link {
    binding_path: PathBuf,
    config: LinkConfig,
    closing: Receiver<String>,
    last_failure: Option<String>,
    /// A connection was logged after `last_failure`.
    recovered: bool,
}

impl Link {
    fn run(&mut self) {
        let mut delay = self.config.retry_initial;
        loop {
            let mut authenticated = false;
            let ended = self
                .connect(&mut authenticated)
                .unwrap_or_else(|error| Ended::Lost {
                    authenticated,
                    error,
                });
            let wait = match ended {
                Ended::Stopped => return,
                Ended::Superseded => {
                    eprintln!(
                        "prism: app link superseded by a later binding of this conversation; not reconnecting"
                    );
                    // Only the end of the session is left to wait for.
                    let _ = self.closing.recv();
                    return;
                }
                Ended::Unbound => self.config.poll_interval,
                Ended::Lost {
                    authenticated,
                    error,
                } => {
                    if authenticated {
                        delay = self.config.retry_initial;
                    }
                    self.report(&error);
                    let wait = delay;
                    delay = (delay * 2).min(self.config.retry_max);
                    wait
                }
            };
            match self.closing.recv_timeout(wait) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(_) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Logs a failure only when it differs from the previous one, so an absent app or pairing
    /// file leaves one line in the bridge log instead of one per attempt.
    fn report(&mut self, error: &anyhow::Error) {
        let message = format!("{error:#}");
        if self.last_failure.as_deref() != Some(message.as_str()) {
            eprintln!("prism: app link: {message}; retrying");
            self.last_failure = Some(message);
            self.recovered = false;
        }
    }

    /// Logs a connection only after a logged failure, and once per failure, so an app that
    /// drops every connection right after `auth` leaves no line per retry.
    fn report_connected(&mut self, address: SocketAddr) {
        if self.last_failure.is_some() && !self.recovered {
            eprintln!("prism: app link connected to {address}");
            self.recovered = true;
        }
    }

    /// True once the bridge asked the link to stop. Checked only before `auth`, where there
    /// is no `closed` to send.
    fn stopping(&self) -> bool {
        !matches!(self.closing.try_recv(), Err(TryRecvError::Empty))
    }

    fn connect(&mut self, authenticated: &mut bool) -> Result<Ended> {
        let Some(binding) = self.ready_binding()? else {
            return Ok(Ended::Unbound);
        };
        let pairing_path = self
            .config
            .pairing_path(&binding.agent)
            .context("no home directory for the pairing file")?;
        let pairing = read_pairing(&pairing_path)?;
        if self.stopping() {
            return Ok(Ended::Stopped);
        }
        let (mut socket, address) = open_socket(&self.config.host, pairing.port)?;
        if self.stopping() {
            return Ok(Ended::Stopped);
        }

        // Handshake: the app proves the token first, and only a verified app hears from us.
        let nonce = protocol::nonce();
        send(
            &mut socket,
            &BridgeMessage::Hello {
                protocol: APP_PROTOCOL,
                nonce: nonce.clone(),
            },
        )?;
        let text = read_text(&mut socket, Instant::now() + IO_TIMEOUT)?;
        if self.stopping() {
            return Ok(Ended::Stopped);
        }
        if let Err(error) = protocol::check_protocol(&text) {
            // Only a `welcome` is answered with `refused`; an app's own `refused` ends it.
            if message_type(&text).as_deref() == Some(WELCOME_TYPE) {
                refuse(socket, Refusal::Protocol);
            }
            return Err(error.into());
        }
        let app_nonce = match serde_json::from_str::<AppMessage>(&text)? {
            AppMessage::Welcome {
                nonce: app_nonce,
                proof,
                ..
            } => {
                if !protocol::verify_proof(&pairing.token, Prover::App, &nonce, &proof) {
                    refuse(socket, Refusal::Token);
                    bail!("the app's proof of the pairing token does not verify");
                }
                app_nonce
            }
            // Not verified on this connection: even `superseded` only leads to a retry.
            AppMessage::Refused { reason, .. } => {
                bail!("the app refused the connection ({reason:?})")
            }
            _ => bail!("the app did not answer hello with welcome"),
        };
        let proof = match protocol::proof(&pairing.token, Prover::Bridge, &app_nonce) {
            Ok(proof) => proof,
            Err(error) => {
                refuse(socket, Refusal::Protocol);
                return Err(error.into());
            }
        };

        let mut connection = Connection::open(socket, &self.binding_path, &binding)?;
        let session = Session {
            host: host_name(),
            project: connection.key.project.clone(),
            agent: binding.agent.clone(),
            conversation: connection.key.conversation.clone(),
            cli: cli_process(&binding),
            binding_id: binding.binding_id,
            bound_at_ms: binding.bound_at_ms,
        };
        send(
            &mut connection.socket,
            &BridgeMessage::Auth { proof, session },
        )?;
        connection.send_snapshot()?;
        *authenticated = true;
        self.report_connected(address);
        connection.serve(&self.closing, self.config.poll_interval)
    }

    /// The binding once it speaks for a conversation; `None` until the first tool call binds
    /// one. A binding written before the bound time was recorded gets `LEGACY_BOUND_AT_MS`,
    /// persisted, so a restarted bridge reports the same value.
    fn ready_binding(&self) -> Result<Option<LiveBinding>> {
        let mut binding = read_binding(&self.binding_path)?;
        ensure!(!binding.retired, "binding retired");
        if binding.conversation.is_none() {
            return Ok(None);
        }
        if binding.bound_at_ms == 0 {
            binding.bound_at_ms = LEGACY_BOUND_AT_MS;
            write_binding(&self.binding_path, &binding)?;
            binding = read_binding(&self.binding_path)?;
        }
        Ok(Some(binding))
    }
}

/// Content last sent to the app: document revisions and question digests by ID.
#[derive(Default)]
struct Sent {
    documents: BTreeMap<Id, u64>,
    questions: BTreeMap<Id, Vec<u8>>,
}

struct Connection {
    socket: WebSocket<TcpStream>,
    binding_path: PathBuf,
    key: WorkspaceKey,
    store: Store,
    workspace: Id,
    /// A second connection whose `data_version` moves on every commit made elsewhere.
    watcher: rusqlite::Connection,
    data_version: i64,
    sent: Sent,
}

impl Connection {
    fn open(
        socket: WebSocket<TcpStream>,
        binding_path: &Path,
        binding: &LiveBinding,
    ) -> Result<Self> {
        let key = binding.workspace_key()?;
        let path = store_path(binding);
        let mut store = Store::open(&path)?;
        let workspace = store.workspace(&key)?.id;
        let watcher = rusqlite::Connection::open(&path)?;
        watcher.busy_timeout(Duration::from_secs(BUSY_TIMEOUT_SECONDS))?;
        let data_version = data_version(&watcher)?;
        Ok(Self {
            socket,
            binding_path: binding_path.to_owned(),
            key,
            store,
            workspace,
            watcher,
            data_version,
            sent: Sent::default(),
        })
    }

    fn serve(&mut self, closing: &Receiver<String>, poll_interval: Duration) -> Result<Ended> {
        self.socket
            .get_mut()
            .set_read_timeout(Some(poll_interval))?;
        loop {
            match closing.try_recv() {
                Ok(reason) => {
                    // The session is over either way; a failed send only loses the notice.
                    let _ = send(&mut self.socket, &BridgeMessage::Closed { reason });
                    finish(&mut self.socket);
                    return Ok(Ended::Stopped);
                }
                Err(TryRecvError::Disconnected) => return Ok(Ended::Stopped),
                Err(TryRecvError::Empty) => {}
            }
            if self.store_changed()? {
                self.send_update()?;
            }
            let text = match self.socket.read() {
                Ok(Message::Text(text)) => text,
                Ok(Message::Close(_)) => bail!("the app closed the connection"),
                Ok(_) => continue,
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    continue;
                }
                // An oversized or malformed frame breaks the protocol like an unreadable message.
                // The socket cannot be read again after it, so it is dropped, not closed.
                Err(
                    error @ (tungstenite::Error::Capacity(_)
                    | tungstenite::Error::Protocol(_)
                    | tungstenite::Error::Utf8(_)),
                ) => {
                    return Ok(Ended::Lost {
                        authenticated: false,
                        error: error.into(),
                    });
                }
                Err(error) => return Err(error.into()),
            };
            let message = match serde_json::from_str::<AppMessage>(text.as_str()) {
                Ok(message) => message,
                // A command the bridge cannot read still gets its reply; a message without an
                // ID breaks the protocol.
                Err(error) => match command_id(text.as_str()) {
                    Some(id) => {
                        let error = CommandError::Invalid {
                            message: error.to_string(),
                        };
                        send(
                            &mut self.socket,
                            &BridgeMessage::Reply {
                                id,
                                error: Some(error),
                            },
                        )?;
                        continue;
                    }
                    None => return Ok(self.violation(anyhow!("unreadable app message: {error}"))),
                },
            };
            let (id, result) = match message {
                AppMessage::Refused {
                    reason: Refusal::Superseded,
                    ..
                } => return Ok(Ended::Superseded),
                AppMessage::Refused { reason, .. } => {
                    return Ok(Ended::Lost {
                        authenticated: false,
                        error: anyhow!("the app refused the connection ({reason:?})"),
                    });
                }
                AppMessage::Welcome { .. } => {
                    return Ok(self.violation(anyhow!("the app sent welcome twice")));
                }
                AppMessage::Ask {
                    id,
                    selection,
                    text,
                } => (id, self.ask(id, selection, text)),
                AppMessage::Action {
                    id,
                    document_id,
                    expected_revision,
                    action,
                } => (id, self.action(document_id, expected_revision, action)),
                AppMessage::Undo {
                    id,
                    document_id,
                    expected_revision,
                } => (id, self.undo(document_id, expected_revision)),
                AppMessage::Fetch { id } => {
                    self.send_snapshot()?;
                    (id, Ok(()))
                }
            };
            // The reply follows the update (or snapshot) its command caused.
            self.send_update()?;
            send(
                &mut self.socket,
                &BridgeMessage::Reply {
                    id,
                    error: result.err(),
                },
            )?;
        }
    }

    /// Closes after a protocol violation; the retry keeps its backoff.
    fn violation(&mut self, error: anyhow::Error) -> Ended {
        finish(&mut self.socket);
        Ended::Lost {
            authenticated: false,
            error,
        }
    }

    /// The checks the tools make: the binding is live, verified against tmux, and still bound
    /// to the conversation this connection speaks for.
    fn verified_binding(&self) -> Result<LiveBinding, CommandError> {
        let binding = read_binding(&self.binding_path).map_err(failed)?;
        if binding.retired {
            return Err(failed(
                "binding retired; explicitly resume the conversation",
            ));
        }
        TmuxClient::socket(&binding.tmux.socket_path)
            .verify_binding(&binding)
            .map_err(failed)?;
        if binding.workspace_key().ok().as_ref() != Some(&self.key) {
            return Err(failed("the binding now speaks for another conversation"));
        }
        Ok(binding)
    }

    /// Shares the selection for the CLI prompt box, where the mod's `pending` poll finds it.
    fn ask(&mut self, id: Id, selection: Selection, text: String) -> Result<(), CommandError> {
        let binding = self.verified_binding()?;
        self.store
            .prepare_question(
                self.workspace,
                &PrepareQuestion {
                    request_id: id,
                    generation: binding.generation,
                    selection,
                    text,
                    mode: DeliveryMode::Share,
                },
            )
            .map_err(store_error)?;
        Ok(())
    }

    fn action(
        &mut self,
        document_id: Id,
        expected_revision: u64,
        action: ContentAction,
    ) -> Result<(), CommandError> {
        let binding = self.verified_binding()?;
        apply_operation(
            &mut self.store,
            &binding,
            Operation::Action(ActionParams {
                document_id,
                expected_revision,
                action,
            }),
        )
        .map_err(command_error)?;
        Ok(())
    }

    /// Undoes the latest undoable action, like the terminal UI; a resent undo that already
    /// committed reports `stale_revision`.
    fn undo(&mut self, document_id: Id, expected_revision: u64) -> Result<(), CommandError> {
        self.verified_binding()?;
        self.store
            .undo_latest(self.workspace, document_id, expected_revision)
            .map_err(store_error)?;
        Ok(())
    }

    fn store_changed(&mut self) -> Result<bool> {
        let version = data_version(&self.watcher)?;
        Ok(std::mem::replace(&mut self.data_version, version) != version)
    }

    /// Documents and the questions shared for the CLI prompt box; other questions never cross.
    fn content(&self) -> Result<(Vec<Document>, Vec<Question>)> {
        let documents = self.store.documents(self.workspace)?;
        let questions = self.store.shared_questions(self.workspace)?;
        Ok((documents, questions))
    }

    fn send_snapshot(&mut self) -> Result<()> {
        let (documents, questions) = self.content()?;
        self.sent = Sent::default();
        for document in &documents {
            self.sent.documents.insert(document.id, document.revision);
        }
        for question in &questions {
            self.sent
                .questions
                .insert(question.request_id, digest(question)?);
        }
        for frame in protocol::snapshot_frames(documents, questions)? {
            send(&mut self.socket, &frame)?;
        }
        Ok(())
    }

    fn send_update(&mut self) -> Result<()> {
        let (documents, questions) = self.content()?;
        let mut changed_documents = Vec::new();
        for document in documents {
            if self.sent.documents.insert(document.id, document.revision) != Some(document.revision)
            {
                changed_documents.push(document);
            }
        }
        let mut changed_questions = Vec::new();
        for question in questions {
            let digest = digest(&question)?;
            if self.sent.questions.get(&question.request_id) != Some(&digest) {
                self.sent.questions.insert(question.request_id, digest);
                changed_questions.push(question);
            }
        }
        if changed_documents.is_empty() && changed_questions.is_empty() {
            return Ok(());
        }
        // Snapshot framing packs the records under the message limit; each frame becomes one
        // `update`, which the app applies by ID like any other.
        for frame in protocol::snapshot_frames(changed_documents, changed_questions)? {
            if let BridgeMessage::Snapshot {
                documents,
                questions,
                ..
            } = frame
            {
                send(
                    &mut self.socket,
                    &BridgeMessage::Update {
                        documents,
                        questions,
                    },
                )?;
            }
        }
        Ok(())
    }
}

fn data_version(connection: &rusqlite::Connection) -> Result<i64> {
    Ok(connection.pragma_query_value(None, "data_version", |row| row.get(0))?)
}

fn digest(question: &Question) -> Result<Vec<u8>> {
    Ok(Sha256::digest(serde_json::to_vec(question)?).to_vec())
}

fn failed(error: impl std::fmt::Display) -> CommandError {
    CommandError::Failed {
        message: error.to_string(),
    }
}

fn store_error(error: actions::Error) -> CommandError {
    match error {
        actions::Error::Conflict { current } => CommandError::StaleRevision { current },
        actions::Error::NotFound(message) => CommandError::NotFound { message },
        actions::Error::Invalid(message) => CommandError::Invalid { message },
        actions::Error::RequestConflict => CommandError::RequestConflict,
        actions::Error::UndoConflict => CommandError::UndoConflict,
        other => failed(other),
    }
}

/// Errors from the shared tool operations keep their store cause when they have one.
fn command_error(error: anyhow::Error) -> CommandError {
    match error.downcast::<actions::Error>() {
        Ok(error) => store_error(error),
        Err(error) => failed(error),
    }
}

/// Attached bindings and Claude record the CLI process itself as their native process; a
/// launcher-started Codex records its private app-server, so its CLI is the pane process.
fn cli_process(binding: &LiveBinding) -> CliProcess {
    if binding.attached || binding.agent == AgentKind::Claude {
        return CliProcess {
            pid: binding.server_pid,
            start_time: binding.server_start_time.clone(),
        };
    }
    CliProcess {
        pid: binding.cli.pid,
        start_time: binding.cli.start_time.clone(),
    }
}

fn host_name() -> String {
    std::fs::read_to_string(HOSTNAME_FILE)
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

fn read_pairing(path: &Path) -> Result<Pairing> {
    let file =
        File::open(path).with_context(|| format!("no pairing file at {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PAIRING_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_PAIRING_BYTES,
        "pairing file exceeds size limit"
    );
    serde_json::from_slice(&bytes).context("pairing file is invalid")
}

/// Gate 1.1: the host name can resolve to an unreachable IPv6 address beside a reachable IPv4
/// one, so every address gets a bounded try.
fn open_socket(host: &str, port: u16) -> Result<(WebSocket<TcpStream>, SocketAddr)> {
    let mut failure = anyhow!("{host} resolves to no address");
    for address in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok((upgrade(stream, address)?, address)),
            Err(error) => failure = anyhow!("app not reachable at {address}: {error}"),
        }
    }
    Err(failure)
}

fn upgrade(stream: TcpStream, address: SocketAddr) -> Result<WebSocket<TcpStream>> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let (socket, _) =
        tungstenite::client::client_with_config(format!("ws://{address}/"), stream, Some(config))
            .map_err(|error| anyhow!("WebSocket handshake with {address} failed: {error}"))?;
    Ok(socket)
}

fn send(socket: &mut WebSocket<TcpStream>, message: &BridgeMessage) -> Result<()> {
    socket.send(Message::Text(serde_json::to_string(message)?.into()))?;
    Ok(())
}

/// The next text message before `deadline`; pings and pongs cannot stretch the wait.
fn read_text(socket: &mut WebSocket<TcpStream>, deadline: Instant) -> Result<String> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .context("the app did not answer in time")?;
        socket.get_mut().set_read_timeout(Some(remaining))?;
        match socket.read()? {
            Message::Text(text) => return Ok(text.as_str().to_owned()),
            Message::Close(_) => bail!("the app closed the connection"),
            _ => {}
        }
    }
}

/// The `type` of a message that may not parse as any known one.
fn message_type(text: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Tagged {
        r#type: String,
    }

    serde_json::from_str::<Tagged>(text)
        .ok()
        .map(|tagged| tagged.r#type)
}

/// The `id` of a command that does not parse, so it can still get its reply.
fn command_id(text: &str) -> Option<Id> {
    #[derive(Deserialize)]
    struct Identified {
        id: Id,
    }

    serde_json::from_str::<Identified>(text)
        .ok()
        .map(|identified| identified.id)
}

/// Answers a `welcome` the bridge does not accept; nothing else follows.
fn refuse(mut socket: WebSocket<TcpStream>, reason: Refusal) {
    let _ = send(
        &mut socket,
        &BridgeMessage::Refused {
            protocol: APP_PROTOCOL,
            reason,
        },
    );
    finish(&mut socket);
}

/// Closes and waits briefly for the app's close reply, so the last message is not lost to a
/// connection reset.
fn finish(socket: &mut WebSocket<TcpStream>) {
    let _ = socket.close(None);
    let _ = socket.get_mut().set_read_timeout(Some(CLOSE_TIMEOUT));
    let deadline = Instant::now() + CLOSE_TIMEOUT;
    while Instant::now() < deadline {
        match socket.read() {
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
    }
}
