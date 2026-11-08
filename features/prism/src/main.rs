//! Linux launcher and entry points for one explicitly registered Codex conversation.
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
const UNIX_SOCKET_LIMIT: usize = 108;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Parser)]
#[command(
    version,
    about = "A visual workspace beside an explicitly bound Codex CLI",
    subcommand_negates_reqs = true
)]
struct Cli {
    #[arg(long, value_enum, required = true)]
    agent: Option<Agent>,
    /// Resume this exact persisted Codex thread UUID; never chooses a recent thread.
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
    #[arg(long)]
    codex: PathBuf,
    #[arg(long)]
    codex_home: Option<PathBuf>,
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
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
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

fn reject_duplicate_resume(runtime: &RuntimeDirectory, conversation: Option<&str>) -> Result<()> {
    let Some(conversation) = conversation else {
        return Ok(());
    };
    for entry in fs::read_dir(runtime.path().parent().context("runtime root missing")?)? {
        let path = entry?.path().join("binding.json");
        let Ok(binding) = read_binding(&path) else {
            continue;
        };
        if binding.agent == AgentKind::Codex
            && binding.conversation.as_deref() == Some(conversation)
            && !binding.retired
            && TmuxClient::socket(&binding.tmux.socket_path)
                .verify_binding(&binding)
                .is_ok()
        {
            bail!(
                "this exact Codex conversation already has a live Prism binding at {}; use its existing tmux pane {}",
                path.display(),
                binding.cli.pane_id
            );
        }
    }
    Ok(())
}

fn launch(agent: Agent, project: &Path, resume: Option<String>, detach: bool) -> Result<()> {
    let signals = StartupSignals::new()?;
    let Agent::Codex = agent;
    let codex = executable("codex")?;
    executable("tmux")?;
    let runtime = RuntimeDirectory::create(project)?;
    let binding_path = runtime.binding_path();
    ensure!(
        runtime
            .path()
            .join("bridge.sock")
            .as_os_str()
            .as_encoded_bytes()
            .len()
            < UNIX_SOCKET_LIMIT,
        "project path is too long for a private Unix socket; use a shorter canonical project path"
    );
    let launch_lock = lock_launches(&runtime, &signals)?;
    reject_duplicate_resume(&runtime, resume.as_deref())?;
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
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("supervise")
        .arg("--binding")
        .arg(&binding_path)
        .arg("--codex")
        .arg(&codex);
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        command
            .arg("--codex-home")
            .arg(PathBuf::from(home).canonicalize()?);
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
            if bridge::connection_info(&binding_path).connected {
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
    supervisor.child.take();
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
            "Prism registered Codex in {}. Ask Codex to open Prism.",
            binding.cli.pane_id
        );
    } else {
        let mut attached = AttachedClient(tmux.attach(&binding)?);
        loop {
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

fn supervise(options: Launch) -> Result<()> {
    let signals = StartupSignals::new()?;
    let project = launch_project(&options.cli.binding)?;
    let directory = options
        .cli
        .binding
        .parent()
        .context("binding directory missing")?;
    let socket = directory.join("app.sock");
    let endpoint = format!("unix://{}", socket.display());
    let log = private_file(&directory.join("codex.log"))?;
    let mut command = Command::new(&options.cli.codex);
    command
        .args(["app-server", "--listen", &endpoint])
        .current_dir(&project)
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
    let tmux = options
        .tmux_socket
        .as_deref()
        .map(TmuxClient::socket)
        .unwrap_or_default();
    let mut arguments = vec![
        OsString::from("native-cli"),
        OsString::from("--binding"),
        options.cli.binding.as_os_str().to_owned(),
        OsString::from("--codex"),
        options.cli.codex.as_os_str().to_owned(),
    ];
    if let Some(home) = &options.cli.codex_home {
        arguments.extend([OsString::from("--codex-home"), home.as_os_str().to_owned()]);
    }
    let cli = tmux.launch_cli(
        &project,
        &std::env::current_exe()?,
        &arguments,
        options.inside_pane.as_deref(),
    )?;
    let mut binding = LiveBinding::new(&project, endpoint, server.0.id(), tmux.identity()?, cli)?;
    if let Some(resume) = &options.resume {
        binding.bind_conversation(resume)?;
    }
    tmux.tag_cli(&binding)?;
    let result = (|| {
        signals.check()?;
        write_binding(&options.cli.binding, &binding)?;
        bridge::run(&options.cli.binding)
    })();
    // Use the original tag generation: bridge retirement increments the persisted generation.
    let _ = tmux.cleanup_cli(&binding);
    result
}

fn native_cli(options: NativeCli) -> Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let binding = loop {
        if let Ok(binding) = read_binding(&options.binding) {
            ensure!(!binding.retired, "binding retired before Codex startup");
            if bridge::connection_info(&options.binding).connected {
                break binding;
            }
        }
        ensure!(
            Instant::now() < deadline,
            "Prism supervisor did not become ready"
        );
        std::thread::sleep(STARTUP_POLL);
    };
    let mut command = Command::new(&options.codex);
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

fn run_ui(path: &Path) -> Result<()> {
    let binding = read_binding(path)?;
    let key = WorkspaceKey {
        project: binding
            .project
            .to_str()
            .context("project path must be UTF-8")?
            .to_owned(),
        agent: AgentKind::Codex,
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
            cli.agent.context("--agent codex is required")?,
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
