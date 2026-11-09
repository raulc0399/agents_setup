//! Linux launcher and entry points for one explicitly registered Codex or Claude conversation.
use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use prism::bridge;
use prism::model::{AgentKind, WorkspaceKey};
use prism::session::{LiveBinding, RuntimeDirectory, TmuxClient, read_binding, write_binding};
use prism::store::Store;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use uuid::Uuid;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const STARTUP_POLL: Duration = Duration::from_millis(50);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Parser)]
#[command(
    version,
    about = "A visual workspace beside an explicitly bound Codex or Claude CLI",
    subcommand_negates_reqs = true
)]
struct Cli {
    #[arg(long, value_enum, required = true)]
    agent: Option<Agent>,
    /// Resume this exact persisted Codex thread or Claude session UUID; never chooses a recent one.
    #[arg(long, value_parser = conversation_id, requires = "agent")]
    resume: Option<String>,
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// Start a registered session without attaching; print its binding path as JSON.
    #[arg(long, requires = "agent")]
    detach: bool,
    #[command(subcommand)]
    command: Option<Entry>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Agent {
    Codex,
    Claude,
}

impl Agent {
    fn kind(self) -> AgentKind {
        match self {
            Agent::Codex => AgentKind::Codex,
            Agent::Claude => AgentKind::Claude,
        }
    }
}

#[derive(Subcommand)]
enum Entry {
    /// Display the persisted workspace associated with a protected launcher binding.
    Ui(Bound),
    /// Serve native MCP over standard input/output; configured by the Codex plugin.
    Mcp,
    /// Run the authenticated bridge for a protected launcher binding.
    Bridge(Bound),
    #[command(hide = true)]
    Supervise(Launch),
    #[command(hide = true)]
    NativeCli(NativeCli),
}

#[derive(Args)]
struct Bound {
    #[arg(long)]
    binding: PathBuf,
}

#[derive(Args)]
struct NativeCli {
    #[arg(long)]
    binding: PathBuf,
    #[arg(long, required_unless_present = "claude", conflicts_with = "claude")]
    codex: Option<PathBuf>,
    #[arg(long, requires = "codex")]
    codex_home: Option<PathBuf>,
    #[arg(long)]
    claude: Option<PathBuf>,
    #[arg(long, requires = "claude")]
    claude_config_dir: Option<PathBuf>,
    /// The pre-bound Claude conversation is a persisted session to resume, not a new session ID.
    #[arg(long, requires = "claude")]
    resume_conversation: bool,
}

#[derive(Args)]
struct Launch {
    #[command(flatten)]
    cli: NativeCli,
    #[arg(long, value_parser = conversation_id)]
    resume: Option<String>,
    #[arg(long)]
    inside_pane: Option<String>,
    #[arg(long)]
    tmux_socket: Option<PathBuf>,
}

fn conversation_id(value: &str) -> Result<String, String> {
    let id = Uuid::parse_str(value).map_err(|_| "resume requires an exact native thread UUID")?;
    if id.is_nil() || id.to_string() != value {
        return Err("resume requires a canonical nonempty native thread UUID".into());
    }
    Ok(value.to_owned())
}

fn executable(name: &str) -> Result<PathBuf> {
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(name);
        if fs::metadata(&candidate).is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0) {
            return Ok(candidate.canonicalize()?);
        }
    }
    bail!("{name} is required on PATH")
}

fn private_file(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

/// Keep a real child handle so PID reuse can never redirect termination.
struct OwnedServer(Child);

impl Drop for OwnedServer {
    fn drop(&mut self) {
        stop_child(&mut self.0);
    }
}

struct StartupSignals {
    stopping: Arc<AtomicBool>,
    registrations: Vec<signal_hook::SigId>,
}

impl StartupSignals {
    fn new() -> Result<Self> {
        let mut signals = Self {
            stopping: Arc::new(AtomicBool::new(false)),
            registrations: Vec::new(),
        };
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            signals.registrations.push(signal_hook::flag::register(
                signal,
                signals.stopping.clone(),
            )?);
        }
        Ok(signals)
    }

    fn check(&self) -> Result<()> {
        ensure!(
            !self.stopping.load(Ordering::Relaxed),
            "Prism startup cancelled"
        );
        Ok(())
    }
}

impl Drop for StartupSignals {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
}

struct StartingSupervisor {
    child: Option<Child>,
    binding_path: PathBuf,
}

fn stop_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        // The unreaped child handle pins this PID while termination is requested.
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(STARTUP_POLL);
        }
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
    }
    let _ = child.wait();
}

struct AttachedClient(Child);

impl Drop for AttachedClient {
    fn drop(&mut self) {
        stop_child(&mut self.0);
    }
}

impl Drop for StartingSupervisor {
    fn drop(&mut self) {
        let Some(child) = &mut self.child else {
            return;
        };
        let binding = read_binding(&self.binding_path).ok();
        stop_child(child);
        if let Some(binding) = binding {
            let _ = TmuxClient::socket(&binding.tmux.socket_path).cleanup_cli(&binding);
        }
    }
}

fn lock_launches(runtime: &RuntimeDirectory, signals: &StartupSignals) -> Result<File> {
    let root = runtime.path().parent().context("runtime root missing")?;
    let lock = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(root)?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        signals.check()?;
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                ensure!(
                    Instant::now() < deadline,
                    "another Prism launch is still starting"
                );
                std::thread::sleep(STARTUP_POLL);
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn reject_duplicate_resume(
    runtime: &RuntimeDirectory,
    agent: &AgentKind,
    conversation: Option<&str>,
) -> Result<()> {
    let Some(conversation) = conversation else {
        return Ok(());
    };
    for entry in fs::read_dir(runtime.path().parent().context("runtime root missing")?)? {
        let path = entry?.path().join("binding.json");
        let Ok(binding) = read_binding(&path) else {
            continue;
        };
        if binding.agent == *agent
            && binding.conversation.as_deref() == Some(conversation)
            && !binding.retired
            && TmuxClient::socket(&binding.tmux.socket_path)
                .verify_binding(&binding)
                .is_ok()
        {
            ensure!(
                bridge::is_ready(&path),
                "this conversation's CLI is still live but its Prism bridge is unavailable; recover the existing binding with: prism bridge --binding {}",
                path.display()
            );
            bail!(
                "this exact {} conversation already has a live Prism binding at {}; use its existing tmux pane {}",
                agent.label(),
                path.display(),
                binding.cli.pane_id
            );
        }
    }
    Ok(())
}

struct UnstartedRuntime(Option<PathBuf>);

impl Drop for UnstartedRuntime {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = prism::session::remove_socket_directory(&path.join("binding.json"));
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn prune_retired_runtimes(runtime: &RuntimeDirectory) -> Result<()> {
    let root = runtime.path().parent().context("runtime root missing")?;
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let Ok(binding) = read_binding(&path.join("binding.json")) else {
            continue;
        };
        let alive = |pid, birth: &str| {
            prism::session::process_start_time(pid).is_ok_and(|actual| actual == birth)
        };
        if !binding.retired
            || alive(binding.server_pid, &binding.server_start_time)
            || alive(binding.cli.pid, &binding.cli.start_time)
            || binding
                .prism
                .as_ref()
                .is_some_and(|pane| alive(pane.pid, &pane.start_time))
        {
            continue;
        }
        let Ok(lease) = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path.join("bridge.lock"))
        else {
            continue;
        };
        if lease.try_lock().is_err() {
            continue;
        }
        prism::session::remove_socket_directory(&path.join("binding.json"))?;
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn launch(agent: Agent, project: &Path, resume: Option<String>, detach: bool) -> Result<()> {
    let signals = StartupSignals::new()?;
    let agent = agent.kind();
    let native = executable(agent.cli_name())?;
    executable("tmux")?;
    let project = project.canonicalize()?;
    let runtime = RuntimeDirectory::create(&project)?;
    let mut unstarted = UnstartedRuntime(Some(runtime.path().to_owned()));
    let binding_path = runtime.binding_path();
    let launch_lock = lock_launches(&runtime, &signals)?;
    prune_retired_runtimes(&runtime)?;
    reject_duplicate_resume(&runtime, &agent, resume.as_deref())?;
    let inside = std::env::var("TMUX_PANE")
        .ok()
        .filter(|_| std::env::var_os("TMUX").is_some());
    let tmux_identity = if inside.is_some() {
        Some(TmuxClient::default().identity()?)
    } else {
        None
    };
    let log_path = runtime.path().join("supervisor.log");
    let log = private_file(&log_path)?;
    // Each CLI has its own configuration directory variable, forwarded only when set.
    let (cli_flag, configuration, configuration_flag) = match agent {
        AgentKind::Codex => ("--codex", "CODEX_HOME", "--codex-home"),
        AgentKind::Claude => ("--claude", "CLAUDE_CONFIG_DIR", "--claude-config-dir"),
    };
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("supervise")
        .arg("--binding")
        .arg(&binding_path)
        .arg(cli_flag)
        .arg(&native);
    if let Some(directory) = std::env::var_os(configuration) {
        command
            .arg(configuration_flag)
            .arg(PathBuf::from(directory).canonicalize()?);
    }
    if let Some(resume) = resume {
        command.arg("--resume").arg(resume);
    }
    if let Some(pane) = &inside {
        command.arg("--inside-pane").arg(pane);
    }
    if let Some(identity) = &tmux_identity {
        command.arg("--tmux-socket").arg(&identity.socket_path);
    }
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: only async-signal-safe setsid is called between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    signals.check()?;
    let mut supervisor = StartingSupervisor {
        child: Some(command.spawn().context("start Prism supervisor")?),
        binding_path: binding_path.clone(),
    };
    unstarted.0 = None;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let binding = loop {
        signals.check()?;
        if let Some(status) = supervisor
            .child
            .as_mut()
            .expect("startup owns its supervisor")
            .try_wait()?
        {
            bail!(
                "Prism startup failed ({status}); see {}",
                log_path.display()
            );
        }
        if let Ok(binding) = read_binding(&binding_path) {
            if binding.retired {
                bail!(
                    "Prism startup retired the binding; see {}",
                    log_path.display()
                );
            }
            let ready = match binding.agent {
                AgentKind::Codex => bridge::is_ready(&binding_path),
                AgentKind::Claude => bridge::connection_info(&binding_path).connected,
            };
            if ready {
                break binding;
            }
        }
        if Instant::now() >= deadline {
            bail!("Prism startup timed out; see {}", log_path.display());
        }
        std::thread::sleep(STARTUP_POLL);
    };
    signals.check()?;
    // The healthy bridge now owns its lifetime; normal detach must keep it running.
    let mut owned_supervisor = supervisor.child.take();
    drop(launch_lock);
    let tmux = TmuxClient::socket(&binding.tmux.socket_path);
    if detach {
        println!(
            "{}",
            serde_json::json!({"binding": binding_path, "pane": binding.cli.pane_id, "session": binding.cli.session_id, "tmux_socket": binding.tmux.socket_path})
        );
    } else if inside.is_some() {
        tmux.select(&binding)?;
        println!(
            "Prism registered {} in {}. Ask {} to open Prism.",
            agent.label(),
            binding.cli.pane_id,
            agent.label()
        );
    } else {
        let mut attached = AttachedClient(tmux.attach(&binding)?);
        loop {
            if let Some(child) = &mut owned_supervisor
                && child.try_wait()?.is_some()
            {
                owned_supervisor = None;
            }
            ensure!(
                !signals.stopping.load(Ordering::Relaxed),
                "Prism attachment interrupted; the registered session remains {}",
                binding.cli.session_id
            );
            if let Some(status) = attached.0.try_wait()? {
                ensure!(
                    status.success(),
                    "tmux attachment failed; the registered session remains {}",
                    binding.cli.session_id
                );
                break;
            }
            std::thread::sleep(STARTUP_POLL);
        }
    }
    Ok(())
}

fn launch_project(binding_path: &Path) -> Result<PathBuf> {
    let runtime = binding_path.parent().context("binding directory missing")?;
    let run = runtime.parent().context("runtime root missing")?;
    let prism = run.parent().context("Prism directory missing")?;
    let project = prism.parent().context("project missing")?;
    ensure!(
        binding_path.file_name() == Some(OsStr::new("binding.json"))
            && run.file_name() == Some(OsStr::new("run"))
            && prism.file_name() == Some(OsStr::new(".prism"))
            && runtime
                .file_name()
                .and_then(OsStr::to_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                .is_some(),
        "invalid launcher binding path"
    );
    ensure!(
        project.canonicalize()? == project,
        "project path must be canonical"
    );
    for path in [runtime, run, prism] {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.mode() & 0o7777 == 0o700
                && metadata.uid() == unsafe { libc::geteuid() },
            "launcher directories must be private and owned by the current user"
        );
    }
    ensure!(!binding_path.exists(), "launcher binding already exists");
    Ok(project.to_owned())
}

/// Codex alone gets a private app-server; the pane TUI connects to it with `--remote`.
struct CodexServer {
    process: OwnedServer,
    endpoint: String,
}

fn start_codex_server(
    codex: &Path,
    options: &Launch,
    project: &Path,
    signals: &StartupSignals,
) -> Result<CodexServer> {
    let directory = options
        .cli
        .binding
        .parent()
        .context("binding directory missing")?;
    let socket = prism::session::socket_directory(&options.cli.binding)?.join("app.sock");
    let endpoint = format!("unix://{}", socket.display());
    let log = private_file(&directory.join("codex.log"))?;
    let mut command = Command::new(codex);
    command
        .args(["app-server", "--listen", &endpoint])
        .current_dir(project)
        .env("PRISM_BINDING_FILE", &options.cli.binding)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    if let Some(home) = &options.cli.codex_home {
        command.env("CODEX_HOME", home);
    }
    let parent = std::process::id();
    // SAFETY: prctl/getppid are used without allocation in the child before exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) < 0
                || libc::getppid() != parent as i32
            {
                return Err(io::Error::other("native server lost its supervisor"));
            }
            Ok(())
        });
    }
    let mut server = OwnedServer(command.spawn().context("start private Codex app-server")?);
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !socket.exists() {
        signals.check()?;
        ensure!(
            server.0.try_wait()?.is_none(),
            "Codex app-server exited; inspect codex.log"
        );
        ensure!(
            Instant::now() < deadline,
            "Codex app-server startup timed out"
        );
        std::thread::sleep(STARTUP_POLL);
    }
    signals.check()?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    Ok(CodexServer {
        process: server,
        endpoint,
    })
}

struct SocketCleanup(PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = prism::session::remove_socket_directory(&self.0);
    }
}

fn supervise(options: Launch) -> Result<()> {
    let signals = StartupSignals::new()?;
    let project = launch_project(&options.cli.binding)?;
    // Declared before the native server so its process stops before sockets are removed.
    let _sockets = SocketCleanup(options.cli.binding.clone());
    let tmux = options
        .tmux_socket
        .as_deref()
        .map(TmuxClient::socket)
        .unwrap_or_default();
    let mut arguments = vec![
        OsString::from("native-cli"),
        OsString::from("--binding"),
        options.cli.binding.as_os_str().to_owned(),
    ];
    let server = match &options.cli.claude {
        Some(claude) => {
            arguments.extend([OsString::from("--claude"), claude.as_os_str().to_owned()]);
            if let Some(directory) = &options.cli.claude_config_dir {
                arguments.extend([
                    OsString::from("--claude-config-dir"),
                    directory.as_os_str().to_owned(),
                ]);
            }
            if options.resume.is_some() {
                arguments.push(OsString::from("--resume-conversation"));
            }
            None
        }
        None => {
            let codex = options
                .cli
                .codex
                .as_deref()
                .context("--codex or --claude is required")?;
            arguments.extend([OsString::from("--codex"), codex.as_os_str().to_owned()]);
            if let Some(home) = &options.cli.codex_home {
                arguments.extend([OsString::from("--codex-home"), home.as_os_str().to_owned()]);
            }
            Some(start_codex_server(codex, &options, &project, &signals)?)
        }
    };
    let cli = tmux.launch_cli(
        &project,
        &std::env::current_exe()?,
        &arguments,
        options.inside_pane.as_deref(),
    )?;
    let mut binding = match &server {
        Some(server) => LiveBinding::new(
            &project,
            AgentKind::Codex,
            server.endpoint.clone(),
            server.process.0.id(),
            tmux.identity()?,
            cli,
        )?,
        None => LiveBinding::new(
            &project,
            AgentKind::Claude,
            format!("claude-cli://{}", cli.pane_id),
            cli.pid,
            tmux.identity()?,
            cli,
        )?,
    };
    // Claude receives its session ID from the launcher, so the conversation is bound up front.
    match (&options.resume, binding.agent == AgentKind::Claude) {
        (Some(resume), _) => binding.bind_conversation(resume)?,
        (None, true) => binding.bind_conversation(&Uuid::new_v4().to_string())?,
        (None, false) => {}
    }
    tmux.tag_cli(&binding)?;
    let result = (|| {
        signals.check()?;
        write_binding(&options.cli.binding, &binding)?;
        bridge::run(&options.cli.binding)
    })();
    // Use the original tag generation: bridge retirement increments the persisted generation.
    // The Claude pane is the user's CLI and stays open; only the Codex TUI pane is Prism's.
    if binding.agent == AgentKind::Codex {
        let _ = tmux.cleanup_cli(&binding);
    }
    result
}

fn native_cli(options: NativeCli) -> Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let binding = loop {
        if let Ok(binding) = read_binding(&options.binding) {
            ensure!(
                !binding.retired,
                "binding retired before {} startup",
                binding.agent.label()
            );
            let ready = match binding.agent {
                AgentKind::Codex => bridge::is_ready(&options.binding),
                AgentKind::Claude => bridge::connection_info(&options.binding).connected,
            };
            if ready {
                break binding;
            }
        }
        ensure!(
            Instant::now() < deadline,
            "Prism supervisor did not become ready"
        );
        std::thread::sleep(STARTUP_POLL);
    };
    if let Some(claude) = &options.claude {
        return exec_claude(claude, &options, &binding);
    }
    let codex = options
        .codex
        .as_ref()
        .context("--codex or --claude is required")?;
    let mut command = Command::new(codex);
    if let Some(conversation) = binding.conversation {
        command.args(["resume", &conversation]);
    }
    command
        .args(["--remote", &binding.server_endpoint])
        .current_dir(&binding.project)
        .env("PRISM_BINDING_FILE", &options.binding);
    if let Some(home) = options.codex_home {
        command.env("CODEX_HOME", home);
    }
    Err(command.exec()).context("start Codex TUI")
}

/// Claude is told its exact session: a fresh `--session-id` or an explicit `--resume`.
fn exec_claude(claude: &Path, options: &NativeCli, binding: &LiveBinding) -> Result<()> {
    let conversation = binding
        .conversation
        .as_deref()
        .context("Claude conversation must be bound by the supervisor")?;
    let mut command = Command::new(claude);
    if options.resume_conversation {
        command.args(["--resume", conversation]);
    } else {
        command.args(["--session-id", conversation]);
    }
    command
        .current_dir(&binding.project)
        .env("PRISM_BINDING_FILE", &options.binding);
    if let Some(directory) = &options.claude_config_dir {
        command.env("CLAUDE_CONFIG_DIR", directory);
    }
    Err(command.exec()).context("start Claude CLI")
}

fn run_ui(path: &Path) -> Result<()> {
    let binding = read_binding(path)?;
    let key = WorkspaceKey {
        project: binding
            .project
            .to_str()
            .context("project path must be UTF-8")?
            .to_owned(),
        agent: binding.agent.clone(),
        conversation: binding
            .conversation
            .clone()
            .context("no native conversation has published to Prism yet")?,
    };
    let mut store = Store::open(bridge::store_path(&binding))?;
    let workspace = store.workspace(&key)?;
    prism::ui::run(store, workspace, || bridge::connection_info(path))
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Entry::Ui(options)) => run_ui(&options.binding),
        Some(Entry::Bridge(options)) => bridge::run(&options.binding),
        Some(Entry::Mcp) => {
            let binding = std::env::var_os("PRISM_BINDING_FILE").map(PathBuf::from);
            prism::mcp::serve(io::stdin().lock(), io::stdout().lock(), binding.as_deref())
        }
        Some(Entry::Supervise(options)) => supervise(options),
        Some(Entry::NativeCli(options)) => native_cli(options),
        None => launch(
            cli.agent.context("--agent codex|claude is required")?,
            &cli.project,
            cli.resume,
            cli.detach,
        ),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("prism: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn long_project_socket_directory_is_removed_on_supervisor_exit() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("p".repeat(100));
        fs::create_dir(&project).unwrap();
        let runtime = RuntimeDirectory::create(&project).unwrap();
        let sockets = prism::session::socket_directory(&runtime.binding_path()).unwrap();
        assert!(sockets.is_dir());
        drop(SocketCleanup(runtime.binding_path()));
        assert!(!sockets.exists());
        assert!(runtime.path().is_dir());
    }

    #[test]
    fn owned_codex_server_gets_graceful_shutdown() {
        let mut child = Command::new("sh")
            .args([
                "-c",
                "trap 'echo stopped; exit 0' TERM; echo ready; while :; do :; done",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        drop(OwnedServer(child));
        line.clear();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "stopped\n");
    }

    #[test]
    fn owned_codex_server_escalates_when_sigterm_is_ignored() {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 30"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        let started = Instant::now();
        drop(OwnedServer(child));
        assert!(started.elapsed() >= SHUTDOWN_GRACE);
        assert!(started.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(2));
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }
}
