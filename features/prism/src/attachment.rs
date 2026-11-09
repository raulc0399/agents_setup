//! Attach native plugin calls to their existing tmux conversation without owning the CLI.
use crate::bridge;
use crate::model::AgentKind;
use crate::session::{
    LiveBinding, RuntimeDirectory, TmuxClient, process_start_time, read_binding,
    remove_socket_directory, write_binding,
};
use anyhow::{Context, Result, bail, ensure};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const TMUX_INSTRUCTION: &str = "Prism needs tmux. Start codex or claude inside tmux, then ask to open Prism in that conversation.";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const STARTUP_POLL: Duration = Duration::from_millis(50);
const MAX_PROCESS_ANCESTORS: usize = 128;

fn belongs_to_pane(mut pid: u32, pane_pid: u32) -> Result<bool> {
    for _ in 0..MAX_PROCESS_ANCESTORS {
        if pid == pane_pid {
            return Ok(true);
        }
        if pid <= 1 {
            return Ok(false);
        }
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        pid = stat
            .rsplit_once(')')
            .context("invalid process ancestry")?
            .1
            .split_whitespace()
            .nth(1)
            .context("missing parent process")?
            .parse()?;
    }
    bail!("process ancestry exceeds limit")
}

struct PendingRuntime(PathBuf);

impl Drop for PendingRuntime {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = remove_socket_directory(&self.0.join("binding.json"));
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

/// The caller supplies only native session identity; pane authority comes from process ancestry.
pub fn ensure_attached(agent: AgentKind, conversation: &str, owner_pid: u32) -> Result<PathBuf> {
    let thread = Uuid::parse_str(conversation).context("native conversation ID is invalid")?;
    ensure!(
        !thread.is_nil() && thread.to_string() == conversation,
        "native conversation ID is not canonical"
    );
    let pane = std::env::var("TMUX_PANE").context(TMUX_INSTRUCTION)?;
    let environment = std::env::var("TMUX").context(TMUX_INSTRUCTION)?;
    let mut fields = environment.rsplitn(3, ',');
    let _session = fields.next().context(TMUX_INSTRUCTION)?;
    let server_pid: u32 = fields.next().context(TMUX_INSTRUCTION)?.parse()?;
    let socket = Path::new(fields.next().context(TMUX_INSTRUCTION)?);
    let tmux = TmuxClient::socket(socket);
    let identity = tmux.identity()?;
    ensure!(identity.pid == server_pid, "tmux server identity changed");
    let cli = tmux.pane_identity(&pane)?;
    ensure!(
        belongs_to_pane(std::process::id(), cli.pid)? && belongs_to_pane(owner_pid, cli.pid)?,
        "Prism caller does not belong to the supplied tmux pane"
    );
    let owner_birth = process_start_time(owner_pid)?;
    let project = std::env::current_dir()?.canonicalize()?;
    let runtime = RuntimeDirectory::create(&project)?;
    let mut pending = PendingRuntime(runtime.path().to_owned());
    let root = runtime.path().parent().context("runtime root missing")?;
    let lock = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(root)?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(STARTUP_POLL)
            }
            Err(error) => return Err(error.into()),
        }
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path().join("binding.json");
        let Ok(binding) = read_binding(&path) else {
            continue;
        };
        if binding.tmux != identity || binding.cli != cli {
            continue;
        }
        let alive = process_start_time(binding.server_pid)
            .is_ok_and(|birth| birth == binding.server_start_time);
        if !binding.retired && alive {
            ensure!(
                binding.attached
                    && binding.agent == agent
                    && binding.server_pid == owner_pid
                    && binding.server_start_time == owner_birth
                    && binding.conversation.as_deref() == Some(conversation),
                "This tmux pane is already attached to another live conversation"
            );
            ensure!(
                bridge::is_ready(&path),
                "Prism bridge is not responding; close its connection before reopening Prism"
            );
            return Ok(path);
        }
        if binding.attached {
            // Retirement increments the generation; the pane retains the preceding tag.
            let mut tagged = binding.clone();
            if tagged.retired {
                tagged.generation = tagged.generation.saturating_sub(1);
            }
            let _ = tmux.release_attached(&tagged);
        }
    }
    let mut binding = LiveBinding::new(
        &project,
        agent.clone(),
        format!("{}-cli://{owner_pid}", agent.cli_name()),
        owner_pid,
        identity,
        cli,
    )?;
    binding.attached = true;
    binding.bind_conversation(conversation)?;
    let path = runtime.binding_path();
    write_binding(&path, &binding)?;
    tmux.tag_cli(&binding)?;
    // Keep the binding and log for diagnosis once bridge startup has been attempted.
    pending.0 = PathBuf::new();
    let result = start_bridge(&path);
    if result.is_err() {
        let _ = tmux.release_attached(&binding);
        let _ = binding.retire();
        let _ = write_binding(&path, &binding);
        let _ = remove_socket_directory(&path);
    }
    result?;
    Ok(path)
}

fn start_bridge(path: &Path) -> Result<()> {
    let log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.with_file_name("bridge.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["attached-bridge", "--binding"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe and does not access Rust memory after fork.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("start Prism bridge")?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "Prism bridge exited ({status}); inspect {}",
                path.with_file_name("bridge.log").display()
            );
        }
        if bridge::is_ready(path) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Prism bridge startup timed out");
        }
        std::thread::sleep(STARTUP_POLL);
    }
}

pub fn run_bridge(path: &Path) -> Result<()> {
    let binding = read_binding(path)?;
    ensure!(binding.attached, "expected an attached conversation");
    bridge::run(path)
}
