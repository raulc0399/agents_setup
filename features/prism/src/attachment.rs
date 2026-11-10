//! Attach native plugin calls to their running CLI without owning it.
use crate::bridge;
use crate::model::AgentKind;
use crate::session::{
    LiveBinding, RuntimeDirectory, process_start_time, read_binding, remove_socket_directory,
    write_binding,
};
use crate::store::Store;
use anyhow::{Context, Result, bail, ensure};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const STARTUP_POLL: Duration = Duration::from_millis(50);
const STALE_SESSION: &str = "native conversation changed";

struct PendingRuntime(PathBuf);

impl Drop for PendingRuntime {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = remove_socket_directory(&self.0.join("binding.json"));
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

/// The caller supplies only native session identity; the CLI is the owner process, identified
/// by its PID and start time.
pub fn ensure_attached(agent: AgentKind, conversation: &str, owner_pid: u32) -> Result<PathBuf> {
    let thread = Uuid::parse_str(conversation).context("native conversation ID is invalid")?;
    ensure!(
        !thread.is_nil() && thread.to_string() == conversation,
        "native conversation ID is not canonical"
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
    // Every runtime is visited, so pruning does not depend on the directory order.
    let mut existing = None;
    for entry in fs::read_dir(root)? {
        let directory = entry?.path();
        let path = directory.join("binding.json");
        let Ok(binding) = read_binding(&path) else {
            continue;
        };
        if binding.retired {
            prune_retired(&directory);
            continue;
        }
        // The owner is alive, so a binding with its PID and start time belongs to this CLI.
        if binding.server_pid != owner_pid || binding.server_start_time != owner_birth {
            continue;
        }
        if binding.agent == agent && binding.conversation.as_deref() == Some(conversation) {
            existing = Some(path);
            continue;
        }
        // A Claude process runs one session at a time, so its other binding belongs to an
        // earlier session whose `session.end` never arrived. A Codex MCP process serves one
        // thread; another thread must not take over its binding.
        ensure!(
            agent == AgentKind::Claude && binding.agent == AgentKind::Claude,
            "This CLI is already attached to another live conversation"
        );
        let mut store = Store::open(bridge::store_path(&binding))?;
        bridge::retire(&path, &mut store, STALE_SESSION)?;
    }
    if let Some(path) = existing {
        if !bridge::is_ready(&path) {
            // The bridge ended without retiring this binding. Its lease lets only one
            // bridge serve a binding, so starting another is safe if the old one hangs.
            start_bridge(&path)
                .context("Prism bridge is not responding and could not be restarted")?;
        }
        return Ok(path);
    }
    let mut binding = LiveBinding::new(
        &project,
        agent.clone(),
        format!("{}-cli://{owner_pid}", agent.cli_name()),
        owner_pid,
    )?;
    binding.bind_conversation(conversation)?;
    let path = runtime.binding_path();
    write_binding(&path, &binding)?;
    // Keep the binding and log for diagnosis once bridge startup has been attempted.
    pending.0 = PathBuf::new();
    let result = start_bridge(&path);
    if result.is_err() {
        let _ = binding.retire();
        let _ = write_binding(&path, &binding);
        let _ = remove_socket_directory(&path);
    }
    result?;
    Ok(path)
}

/// Removes a retired runtime directory once no bridge holds its lease; a bridge that is still
/// ending keeps it, and so does a runtime without a lease file. Pruning is housekeeping, so a
/// failure leaves the directory for a later attachment.
fn prune_retired(directory: &Path) {
    let Ok(lease) = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("bridge.lock"))
    else {
        return;
    };
    if lease.try_lock().is_err() {
        return;
    }
    let _ = remove_socket_directory(&directory.join("binding.json"));
    let _ = fs::remove_dir_all(directory);
}

fn start_bridge(path: &Path) -> Result<()> {
    // A restarted bridge appends to the log of the one that ended.
    let log = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.with_file_name("bridge.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["bridge", "--binding"])
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
