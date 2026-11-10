//! The app without its windows: what the shell calls and the events it forwards.
use crate::arrangement::{Arrangement, CloseWindow, SessionLabel, WindowId};
use crate::listener;
use crate::pairing::{PairingFile, remove_pairing, write_pairing};
use crate::registry::{
    Attach, Command, Connection, Content, HostEvent, Link, Registry, SessionSummary, session_key,
};
use crate::settings::Settings;
use prism_core::model::Id;
use prism_core::pairing::Pairing;
use prism_core::protocol::{APP_PROTOCOL, BridgeMessage, Session, nonce};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::Semaphore;

/// A connection that has not finished `hello` -> `welcome` -> `auth` by then is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A command without a reply by then is reported as failed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// Commands of a session disconnected this long fail instead of waiting for a resend.
const DISCONNECT_GRACE: Duration = Duration::from_secs(20);
/// The app pings each bridge this often; see `MISSED_PONGS`.
const PING_INTERVAL: Duration = Duration::from_secs(15);
/// How often unanswered commands are checked against their deadlines.
const EXPIRY_CHECK_INTERVAL: Duration = Duration::from_millis(250);
/// View and focus changes are saved together this long after the first of them.
const QUIET_SAVE_DELAY: Duration = Duration::from_millis(500);
/// Handshakes in progress at once; further connections are dropped until one finishes, so
/// a local process opening sockets cannot exhaust the app.
const MAX_PENDING_HANDSHAKES: usize = 16;
/// A new problem replaces the shown one at most this often.
const PROBLEM_REPORT_INTERVAL: Duration = Duration::from_secs(1);
/// Longest problem text shown; a peer controls part of it.
const MAX_PROBLEM_CHARS: usize = 300;

pub type EventSink = Box<dyn Fn(HostEvent) + Send + Sync>;

/// Deadlines of the bridge connections; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub handshake: Duration,
    pub command: Duration,
    pub disconnect_grace: Duration,
    pub ping_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            handshake: HANDSHAKE_TIMEOUT,
            command: COMMAND_TIMEOUT,
            disconnect_grace: DISCONNECT_GRACE,
            ping_interval: PING_INTERVAL,
        }
    }
}

pub struct HostConfig {
    pub address: SocketAddr,
    /// `~/.claude`, `~/.codex`: where pairing files go when the directory exists.
    pub cli_directories: Vec<PathBuf>,
    /// The app's own data directory: settings and arrangement.
    pub data_directory: PathBuf,
    pub timing: Timing,
    /// A settings file that could not be used, shown to the user.
    pub settings_error: Option<String>,
}

impl HostConfig {
    pub fn new(home: &Path, data_directory: PathBuf) -> Self {
        let (settings, settings_error) = Settings::load(&data_directory);
        Self {
            address: settings.address(),
            cli_directories: crate::pairing::cli_directories(home),
            data_directory,
            timing: Timing::default(),
            settings_error,
        }
    }
}

/// Listener and pairing state, for the empty state and the sessions panel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub address: SocketAddr,
    pub listening: bool,
    /// Why the app does not listen.
    pub listen_error: Option<String>,
    /// Files the app could not use (settings, arrangement) or save.
    pub warnings: Vec<String>,
    pub pairing: Vec<PairingFile>,
    /// The latest handshake or protocol problem with a bridge; cleared when one connects.
    pub problem: Option<String>,
    pub settings_file: PathBuf,
    /// This app's version and the app protocol it speaks, shown beside the status.
    pub version: String,
    pub protocol: u32,
}

/// Everything a window needs except content.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppState {
    pub status: Status,
    /// Sessions of this run, then sessions known only from the saved arrangement.
    pub sessions: Vec<SessionSummary>,
    pub arrangement: Arrangement,
}

pub(crate) struct Shared {
    pub(crate) token: String,
    pub(crate) timing: Timing,
    pub(crate) handshakes: Arc<Semaphore>,
    registry: Mutex<Registry>,
    arrangement: Mutex<Arrangement>,
    arrangement_path: PathBuf,
    /// A quiet change waits for its delayed save.
    save_scheduled: AtomicBool,
    status: Mutex<Status>,
    problem_shown_at: Mutex<Option<Instant>>,
    runtime: Handle,
    sink: EventSink,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn truncate(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut truncated: String = text.chars().take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}

impl Shared {
    /// Runs `change` on the registry and emits its events after releasing it.
    fn with_registry<R>(&self, change: impl FnOnce(&mut Registry) -> R) -> R {
        let (result, events) = {
            let mut registry = lock(&self.registry);
            let result = change(&mut registry);
            (result, registry.take_events())
        };
        for event in events {
            (self.sink)(event);
        }
        result
    }

    /// Runs `change` on the arrangement; when it changed anything, saves it and tells the
    /// windows.
    fn with_arrangement<R>(&self, change: impl FnOnce(&mut Arrangement) -> R) -> R {
        let (result, changed) = {
            let mut arrangement = lock(&self.arrangement);
            let before = arrangement.clone();
            let result = change(&mut arrangement);
            (result, *arrangement != before)
        };
        if changed {
            self.save_arrangement();
            (self.sink)(HostEvent::State);
        }
        result
    }

    /// Writes the arrangement now; a delayed save scheduled earlier is covered by this one.
    fn save_arrangement(&self) {
        self.save_scheduled.store(false, Ordering::SeqCst);
        let saved = lock(&self.arrangement).save(&self.arrangement_path);
        if let Err(error) = saved {
            let warning = format!(
                "Cannot save the window arrangement to {}: {error}",
                self.arrangement_path.display()
            );
            let mut status = lock(&self.status);
            if !status.warnings.contains(&warning) {
                status.warnings.push(warning);
            }
        }
    }

    pub(crate) fn attach(&self, session: Session, connection: Connection) -> Attach {
        let key = session_key(&session.agent, &session.conversation);
        let label = SessionLabel::from(&session);
        // The handshake succeeded, so an earlier pairing problem is over.
        lock(&self.status).problem = None;
        let attach = self.with_registry(|registry| registry.attach(session, connection));
        if attach == Attach::Accepted {
            // A window always exists, so placing never needs a new native window.
            let _opened_in = self.with_arrangement(|arrangement| arrangement.place(&key, label));
        }
        attach
    }

    pub(crate) fn detach(&self, connection: u64) {
        self.with_registry(|registry| registry.detach(connection));
    }

    pub(crate) fn receive(&self, connection: u64, message: BridgeMessage) {
        self.with_registry(|registry| registry.receive(connection, message));
    }

    /// Shows a problem with a bridge; repeats and bursts (a misconfigured bridge retrying)
    /// do not flood the windows.
    pub(crate) fn report_problem(&self, problem: String) {
        let problem = truncate(problem, MAX_PROBLEM_CHARS);
        {
            let mut shown_at = lock(&self.problem_shown_at);
            let mut status = lock(&self.status);
            let recent = shown_at.is_some_and(|at| at.elapsed() < PROBLEM_REPORT_INTERVAL);
            if status.problem.as_ref() == Some(&problem) || recent {
                return;
            }
            status.problem = Some(problem);
            *shown_at = Some(Instant::now());
        }
        (self.sink)(HostEvent::State);
    }
}

pub struct Host {
    shared: Arc<Shared>,
}

impl Host {
    /// Listens, writes the pairing files and restores the arrangement. Never fails: a port
    /// in use or another listener problem is part of the status the windows show.
    /// Must run inside a Tokio runtime; the listener runs on it.
    pub async fn start(config: HostConfig, sink: EventSink) -> Self {
        let arrangement_path = Arrangement::path(&config.data_directory);
        let (mut arrangement, arrangement_error) = Arrangement::load(&arrangement_path);
        arrangement.ensure_window();
        let token = nonce();

        let mut status = Status {
            address: config.address,
            listening: false,
            listen_error: None,
            warnings: config
                .settings_error
                .into_iter()
                .chain(arrangement_error)
                .collect(),
            pairing: Vec::new(),
            problem: None,
            settings_file: Settings::path(&config.data_directory),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: APP_PROTOCOL,
        };
        let listener = match listener::bind(config.address).await {
            Ok(listener) => {
                status.address = listener.local_addr().unwrap_or(config.address);
                status.listening = true;
                status.pairing = write_pairing(
                    &config.cli_directories,
                    &Pairing {
                        port: status.address.port(),
                        token: token.clone(),
                    },
                );
                Some(listener)
            }
            Err(error) => {
                status.listen_error = Some(error);
                None
            }
        };

        let shared = Arc::new(Shared {
            token,
            timing: config.timing,
            handshakes: Arc::new(Semaphore::new(MAX_PENDING_HANDSHAKES)),
            registry: Mutex::new(Registry::default()),
            arrangement: Mutex::new(arrangement),
            arrangement_path,
            save_scheduled: AtomicBool::new(false),
            status: Mutex::new(status),
            problem_shown_at: Mutex::new(None),
            runtime: Handle::current(),
            sink,
        });
        if let Some(listener) = listener {
            tokio::spawn(listener::accept_loop(listener, shared.clone()));
        }
        tokio::spawn(expire_commands(shared.clone()));
        Self { shared }
    }

    pub fn state(&self) -> AppState {
        let mut sessions = lock(&self.shared.registry).summaries();
        let arrangement = lock(&self.shared.arrangement).clone();
        for (key, label) in &arrangement.labels {
            if sessions.iter().all(|session| &session.key != key) {
                sessions.push(SessionSummary {
                    key: key.clone(),
                    host: label.host.clone(),
                    project: label.project.clone(),
                    agent: label.agent.clone(),
                    conversation: label.conversation.clone(),
                    link: Link::Waiting,
                    loaded: false,
                });
            }
        }
        AppState {
            status: lock(&self.shared.status).clone(),
            sessions,
            arrangement,
        }
    }

    pub fn content(&self, key: &str) -> Option<Content> {
        lock(&self.shared.registry).content(key)
    }

    /// Sends a command to the session's bridge; its `reply` arrives as an event.
    pub fn send(&self, key: &str, command: Command) -> Result<Id, String> {
        self.shared
            .with_registry(|registry| registry.send_command(key, command))
            .map_err(|error| error.to_string())
    }

    pub fn activate_tab(&self, window: &str, key: &str) {
        self.shared
            .with_arrangement(|arrangement| arrangement.activate(window, key));
    }

    pub fn close_tab(&self, key: &str) {
        self.shared
            .with_arrangement(|arrangement| arrangement.close_tab(key));
    }

    /// Opens a session's tab in `window`, whether it was closed or never opened.
    pub fn reopen_tab(&self, key: &str, window: &str) {
        let label = lock(&self.shared.registry)
            .session(key)
            .map(SessionLabel::from)
            .or_else(|| lock(&self.shared.arrangement).labels.get(key).cloned());
        let Some(label) = label else {
            return;
        };
        self.shared
            .with_arrangement(|arrangement| arrangement.reopen(key, label, window));
    }

    /// Moves a tab to another window, or to a new one when `target` is `None`; the shell
    /// opens the native window for a new one.
    pub fn move_tab(&self, key: &str, target: Option<&str>) -> Option<WindowId> {
        self.shared
            .with_arrangement(|arrangement| arrangement.move_tab(key, target))
    }

    /// View state belongs to the window that shows the tab; no other window needs to hear
    /// about it, and it is saved with a delay.
    pub fn set_view(&self, key: &str, view: serde_json::Value) {
        self.change_quietly(|arrangement| arrangement.set_view(key, view));
    }

    pub fn focus_window(&self, window: &str) {
        self.change_quietly(|arrangement| arrangement.focus(window));
    }

    pub fn close_window(&self, window: &str) -> CloseWindow {
        self.shared
            .with_arrangement(|arrangement| arrangement.close_window(window))
    }

    pub fn window_title(&self, window: &str) -> String {
        lock(&self.shared.arrangement).window_title(window)
    }

    /// Closes the bridge connections, saves a pending arrangement change and removes the
    /// pairing files this app wrote, so bridges wait quietly for the next start.
    pub fn shutdown(&self) {
        lock(&self.shared.registry).close_all();
        if self.shared.save_scheduled.load(Ordering::SeqCst) {
            self.shared.save_arrangement();
        }
        remove_pairing(&lock(&self.shared.status).pairing, &self.shared.token);
    }

    /// Applies a change no window needs to hear about and saves it after `QUIET_SAVE_DELAY`,
    /// together with the changes that follow it meanwhile.
    fn change_quietly(&self, change: impl FnOnce(&mut Arrangement)) {
        let changed = {
            let mut arrangement = lock(&self.shared.arrangement);
            let before = arrangement.clone();
            change(&mut arrangement);
            *arrangement != before
        };
        if !changed || self.shared.save_scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = self.shared.clone();
        self.shared.runtime.spawn(async move {
            tokio::time::sleep(QUIET_SAVE_DELAY).await;
            if shared.save_scheduled.load(Ordering::SeqCst) {
                shared.save_arrangement();
            }
        });
    }
}

async fn expire_commands(shared: Arc<Shared>) {
    let mut interval = tokio::time::interval(EXPIRY_CHECK_INTERVAL);
    loop {
        interval.tick().await;
        let timing = shared.timing;
        shared.with_registry(|registry| {
            registry.expire(Instant::now(), timing.command, timing.disconnect_grace)
        });
    }
}
