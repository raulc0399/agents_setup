//! Protected binding credentials and the identity of the CLI process they belong to.
use crate::model::{AgentKind, WorkspaceKey};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const SOCKET_DIRECTORY_PREFIX: &str = "/tmp/prism-";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_BINDING_BYTES: u64 = 64 * 1024;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Linux process birth identity prevents accepting a recycled PID.
pub fn process_start_time(pid: u32) -> io::Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields = stat
        .rsplit_once(')')
        .ok_or_else(|| invalid("invalid process stat"))?
        .1;
    let mut fields = fields.split_whitespace();
    if matches!(fields.next(), Some("Z" | "X") | None) {
        return Err(invalid("process is no longer alive"));
    }
    fields
        .nth(18)
        .map(str::to_owned)
        .ok_or_else(|| invalid("missing process start time"))
}

/// Milliseconds since the Unix epoch; never 0, which marks a binding without a bound time.
pub fn unix_time_ms() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(elapsed.as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

/// Contains a secret: deliberately does not implement Debug.
#[derive(Clone, Serialize, Deserialize)]
pub struct LiveBinding {
    pub binding_id: Uuid,
    pub project: PathBuf,
    pub agent: AgentKind,
    pub conversation: Option<String>,
    pub server_endpoint: String,
    pub server_pid: u32,
    pub server_start_time: String,
    pub token: String,
    pub generation: u64,
    pub retired: bool,
    /// When the conversation was bound to this CLI, in milliseconds since the Unix epoch. 0 in
    /// bindings written before it was recorded; the bridge fills those in.
    #[serde(default)]
    pub bound_at_ms: u64,
}

impl LiveBinding {
    /// The recorded native process is the attached CLI process whose birth identity
    /// authorizes this binding: the Claude CLI, or the Codex MCP process, with the endpoint
    /// `<cli>-cli://<pid>`.
    pub fn new(
        project: &Path,
        agent: AgentKind,
        server_endpoint: String,
        server_pid: u32,
    ) -> io::Result<Self> {
        Ok(Self {
            binding_id: Uuid::new_v4(),
            project: project.canonicalize()?,
            agent,
            conversation: None,
            server_endpoint,
            server_pid,
            server_start_time: process_start_time(server_pid)?,
            token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            generation: 1,
            retired: false,
            bound_at_ms: 0,
        })
    }

    /// Only the authenticated bridge may supply native server-injected identity.
    pub fn bind_conversation(&mut self, conversation: &str) -> io::Result<()> {
        if self.retired || conversation.is_empty() || conversation.len() > 512 {
            return Err(invalid("binding is retired or conversation is invalid"));
        }
        if let Some(current) = &self.conversation {
            if current != conversation {
                self.retire()?;
                return Err(invalid("native conversation changed; binding retired"));
            }
            return Ok(());
        }
        self.conversation = Some(conversation.to_owned());
        self.bound_at_ms = unix_time_ms();
        Ok(())
    }

    pub fn workspace_key(&self) -> io::Result<WorkspaceKey> {
        if self.retired {
            return Err(invalid("binding is retired"));
        }
        Ok(WorkspaceKey {
            project: self
                .project
                .to_str()
                .ok_or_else(|| invalid("project path is not UTF-8"))?
                .to_owned(),
            agent: self.agent.clone(),
            conversation: self
                .conversation
                .clone()
                .ok_or_else(|| invalid("native conversation is not bound yet"))?,
        })
    }

    pub fn retire(&mut self) -> io::Result<()> {
        self.retired = true;
        self.token.clear();
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("binding generation exhausted"))?;
        Ok(())
    }
}

pub struct RuntimeDirectory {
    path: PathBuf,
}

impl RuntimeDirectory {
    pub fn create(project: &Path) -> io::Result<Self> {
        let project = project.canonicalize()?;
        let prism = project.join(".prism");
        create_private_directory(&prism)?;
        ignore_in_git(&prism)?;
        let run = prism.join("run");
        create_private_directory(&run)?;
        let path = run.join(Uuid::new_v4().to_string());
        fs::DirBuilder::new()
            .mode(PRIVATE_DIRECTORY_MODE)
            .create(&path)?;
        validate_private(&path, true)?;
        let runtime = Self { path };
        create_private_directory(&socket_directory(&runtime.binding_path())?)?;
        Ok(runtime)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn binding_path(&self) -> PathBuf {
        self.path.join("binding.json")
    }

    pub fn write_binding(&self, binding: &LiveBinding) -> io::Result<()> {
        write_binding(&self.binding_path(), binding)
    }
}

/// Socket paths stay short and use the local temporary filesystem.
pub fn socket_directory(binding_path: &Path) -> io::Result<PathBuf> {
    validate_binding_location(binding_path)?;
    let name = binding_path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .ok_or_else(|| invalid("binding directory missing"))?;
    let directory = PathBuf::from(format!("{SOCKET_DIRECTORY_PREFIX}{name}"));
    match fs::symlink_metadata(&directory) {
        Ok(_) => validate_private(&directory, true)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(directory)
}

pub fn remove_socket_directory(binding_path: &Path) -> io::Result<()> {
    let directory = socket_directory(binding_path)?;
    if !directory.try_exists()? {
        return Ok(());
    }
    validate_private(&directory, true)?;
    fs::remove_dir_all(directory)
}

const GIT_IGNORE_FILE: &str = ".gitignore";
const GIT_IGNORE_EVERYTHING: &[u8] = b"*\n";

/// Keeps `.prism/` out of version control in every project without per-project setup. A
/// file that already exists is left as the user wrote it.
fn ignore_in_git(prism: &Path) -> io::Result<()> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(prism.join(GIT_IGNORE_FILE))
    {
        Ok(mut file) => file.write_all(GIT_IGNORE_EVERYTHING),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new()
        .mode(PRIVATE_DIRECTORY_MODE)
        .create(path)
    {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    validate_private(path, true)
}

fn validate_private(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no arguments and no memory preconditions.
    let uid = unsafe { libc::geteuid() };
    let mode = if directory {
        PRIVATE_DIRECTORY_MODE
    } else {
        PRIVATE_FILE_MODE
    };
    if metadata.uid() != uid
        || metadata.mode() & 0o7777 != mode
        || metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && (!metadata.is_file() || metadata.nlink() != 1))
    {
        return Err(invalid(
            "runtime path must be owned by the current user with private permissions",
        ));
    }
    Ok(())
}

fn validate_binding_location(path: &Path) -> io::Result<PathBuf> {
    if path.file_name() != Some(OsStr::new("binding.json")) {
        return Err(invalid("unexpected binding filename"));
    }
    let directory = path
        .parent()
        .ok_or_else(|| invalid("binding directory missing"))?;
    let run = directory
        .parent()
        .ok_or_else(|| invalid("runtime root missing"))?;
    let prism = run
        .parent()
        .ok_or_else(|| invalid("Prism directory missing"))?;
    let project = prism
        .parent()
        .ok_or_else(|| invalid("project directory missing"))?;
    if run.file_name() != Some(OsStr::new("run"))
        || prism.file_name() != Some(OsStr::new(".prism"))
        || directory
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| Uuid::parse_str(name).ok())
            .is_none()
    {
        return Err(invalid(
            "binding must reside in a per-launch Prism runtime directory",
        ));
    }
    for component in [prism, run, directory] {
        validate_private(component, true)?;
    }
    if project.canonicalize()? != project {
        return Err(invalid("binding project path must be canonical"));
    }
    Ok(project.to_path_buf())
}

fn lock_binding(path: &Path) -> io::Result<File> {
    validate_binding_location(path)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(
            path.parent()
                .ok_or_else(|| invalid("binding directory missing"))?,
        )?;
    // Lock the stable directory inode: binding.json is replaced atomically on every write.
    directory.lock()?;
    Ok(directory)
}

fn same_authority(left: &LiveBinding, right: &LiveBinding) -> bool {
    left.binding_id == right.binding_id
        && left.project == right.project
        && left.agent == right.agent
        && left.server_endpoint == right.server_endpoint
        && left.server_pid == right.server_pid
        && left.server_start_time == right.server_start_time
}

pub fn write_binding(path: &Path, binding: &LiveBinding) -> io::Result<()> {
    let _lock = lock_binding(path)?;
    let mut updated = binding.clone();
    match read_binding(path) {
        Ok(stored) => {
            if !same_authority(&stored, binding)
                || stored.generation > binding.generation
                || (stored.retired && !binding.retired)
                || (stored.generation == binding.generation && stored.token != binding.token)
                || (stored.generation < binding.generation && !binding.retired)
            {
                return Err(invalid("cannot overwrite a newer or different binding"));
            }
            if let Some(conversation) = &stored.conversation {
                if binding
                    .conversation
                    .as_ref()
                    .is_some_and(|current| current != conversation)
                {
                    return Err(invalid("cannot replace the bound native conversation"));
                }
                // The bound time belongs to the stored conversation; a stale writer keeps it.
                if stored.bound_at_ms != 0 {
                    updated.bound_at_ms = stored.bound_at_ms;
                }
                updated.conversation = stored.conversation;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write_binding_locked(path, &updated)
}

fn write_binding_locked(path: &Path, binding: &LiveBinding) -> io::Result<()> {
    if validate_binding_location(path)? != binding.project {
        return Err(invalid(
            "binding project does not match credential location",
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => settled(|| validate_private(path, false))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = path.with_file_name(format!(".binding-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE_MODE)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        serde_json::to_writer(&mut file, binding).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(
            path.parent()
                .ok_or_else(|| invalid("binding directory missing"))?,
        )?
        .sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// A writer replaces the binding by rename. A reader can catch the old file an instant
/// before it is unlinked, and network or virtual mounts can serve its stale attributes for a
/// moment afterwards. Both look like a file that fails validation, so a failed check is
/// repeated briefly before it is believed.
const SETTLE_RETRIES: usize = 10;
const SETTLE_INTERVAL: Duration = Duration::from_millis(5);

fn settled<T>(mut check: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        match check() {
            Err(error)
                if error.kind() == io::ErrorKind::InvalidData && attempt < SETTLE_RETRIES =>
            {
                attempt += 1;
                std::thread::sleep(SETTLE_INTERVAL);
            }
            result => return result,
        }
    }
}

pub fn read_binding(path: &Path) -> io::Result<LiveBinding> {
    settled(|| read_binding_once(path))
}

fn read_binding_once(path: &Path) -> io::Result<LiveBinding> {
    let project = validate_binding_location(path)?;
    validate_private(path, false)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    // Recheck the opened descriptor, not only the pre-open path.
    if metadata.len() > MAX_BINDING_BYTES
        || metadata.mode() & 0o7777 != PRIVATE_FILE_MODE
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || !metadata.is_file()
    {
        return Err(invalid("invalid binding file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BINDING_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BINDING_BYTES {
        return Err(invalid("binding file exceeds size limit"));
    }
    let binding: LiveBinding = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if binding.project != project
        || binding.generation == 0
        || (!binding.retired && binding.token.len() != 64)
    {
        return Err(invalid("invalid binding identity"));
    }
    Ok(binding)
}

/// A binding is live while it is not retired and its CLI process runs with the recorded
/// start time, so a recycled PID never counts as the bound CLI.
pub fn verify_binding(binding: &LiveBinding) -> io::Result<()> {
    if binding.retired || process_start_time(binding.server_pid)? != binding.server_start_time {
        return Err(invalid("CLI process identity changed or binding retired"));
    }
    Ok(())
}
