//! Launcher-owned credentials and exact tmux pane identities.
use crate::model::{AgentKind, WorkspaceKey};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use uuid::Uuid;

const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_BINDING_BYTES: u64 = 64 * 1024;
const PANE_FORMAT: &str = "#{pane_id}\t#{pane_pid}\t#{window_id}\t#{session_id}\t#{pane_dead}\t#{pane_left}\t#{pane_top}\t#{pane_width}\t#{pane_height}\t#{@prism_binding}\t#{@prism_generation}\t#{@prism_role}\t#{window_zoomed_flag}";
const PANE_FIELDS: usize = 13;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxIdentity {
    pub socket_path: PathBuf,
    pub pid: u32,
    pub start_time: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneIdentity {
    pub pane_id: String,
    pub pid: u32,
    pub start_time: String,
    pub window_id: String,
    pub session_id: String,
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
    pub tmux: TmuxIdentity,
    pub cli: PaneIdentity,
    pub prism: Option<PaneIdentity>,
    pub token: String,
    pub generation: u64,
    pub retired: bool,
}

impl LiveBinding {
    /// The recorded native process is the launcher-owned process whose birth identity
    /// authorizes this binding. Codex records its private app-server (`unix://<socket>`).
    /// Claude has no server: the supervisor records the CLI pane process, because
    /// `prism native-cli` execs `claude` and keeps the same PID and birth time, with the
    /// endpoint `claude-cli://<pane_id>`.
    pub fn new(
        project: &Path,
        agent: AgentKind,
        server_endpoint: String,
        server_pid: u32,
        tmux: TmuxIdentity,
        cli: PaneIdentity,
    ) -> io::Result<Self> {
        Ok(Self {
            binding_id: Uuid::new_v4(),
            project: project.canonicalize()?,
            agent,
            conversation: None,
            server_endpoint,
            server_pid,
            server_start_time: process_start_time(server_pid)?,
            tmux,
            cli,
            prism: None,
            token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
            generation: 1,
            retired: false,
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
        let run = prism.join("run");
        create_private_directory(&run)?;
        let path = run.join(Uuid::new_v4().to_string());
        fs::DirBuilder::new()
            .mode(PRIVATE_DIRECTORY_MODE)
            .create(&path)?;
        validate_private(&path, true)?;
        Ok(Self { path })
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
        && left.tmux == right.tmux
        && left.cli == right.cli
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
                updated.conversation = stored.conversation;
            }
            // UI ownership changes only through ensure_ui's serialized transaction.
            updated.prism = stored.prism;
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
        Ok(_) => validate_private(path, false)?,
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

pub fn read_binding(path: &Path) -> io::Result<LiveBinding> {
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

#[derive(Clone, Default)]
pub struct TmuxClient {
    selector: Vec<OsString>,
}

struct PaneState {
    identity: PaneIdentity,
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    binding: String,
    generation: String,
    role: String,
    zoomed: bool,
}

impl TmuxClient {
    pub fn socket(path: &Path) -> Self {
        Self {
            selector: vec!["-S".into(), path.as_os_str().to_owned()],
        }
    }

    pub fn named(name: &str) -> Self {
        Self {
            selector: vec!["-L".into(), name.into()],
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        command.args(&self.selector);
        command
    }

    fn output(&self, arguments: &[&OsStr]) -> io::Result<String> {
        let result = self.command().args(arguments).output()?;
        if !result.status.success() {
            // tmux diagnostics may echo shell commands; never expose them through bridge errors.
            return Err(io::Error::other("tmux command failed"));
        }
        String::from_utf8(result.stdout)
            .map(|text| text.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(io::Error::other)
    }

    pub fn identity(&self) -> io::Result<TmuxIdentity> {
        let output = self.output(&[
            OsStr::new("display-message"),
            OsStr::new("-p"),
            OsStr::new("#{socket_path}\t#{pid}"),
        ])?;
        let (socket, pid) = output
            .split_once('\t')
            .ok_or_else(|| invalid("invalid tmux server identity"))?;
        let pid = pid
            .parse()
            .map_err(|_| invalid("invalid tmux server PID"))?;
        Ok(TmuxIdentity {
            socket_path: PathBuf::from(socket),
            pid,
            start_time: process_start_time(pid)?,
        })
    }

    fn pane(&self, id: &str) -> io::Result<PaneState> {
        if !id.starts_with('%') || id[1..].parse::<u64>().is_err() {
            return Err(invalid("invalid pane ID"));
        }
        let output = self.output(&[
            OsStr::new("display-message"),
            OsStr::new("-p"),
            OsStr::new("-t"),
            OsStr::new(id),
            OsStr::new(PANE_FORMAT),
        ])?;
        let fields: Vec<_> = output.split('\t').collect();
        if fields.len() != PANE_FIELDS || fields[0] != id || fields[4] != "0" {
            return Err(invalid("tmux pane is missing or dead"));
        }
        let number = |index: usize| {
            fields[index]
                .parse::<u32>()
                .map_err(|_| invalid("invalid tmux pane field"))
        };
        let pid = number(1)?;
        Ok(PaneState {
            identity: PaneIdentity {
                pane_id: fields[0].to_owned(),
                pid,
                start_time: process_start_time(pid)?,
                window_id: fields[2].to_owned(),
                session_id: fields[3].to_owned(),
            },
            left: number(5)?,
            top: number(6)?,
            width: number(7)?,
            height: number(8)?,
            binding: fields[9].to_owned(),
            generation: fields[10].to_owned(),
            role: fields[11].to_owned(),
            zoomed: fields[12] == "1",
        })
    }

    /// True only for a dead pane that still carries this binding's UI tags.
    fn owned_dead_pane(&self, id: &str, binding: &LiveBinding) -> io::Result<bool> {
        let output = self.output(&[
            OsStr::new("display-message"),
            OsStr::new("-p"),
            OsStr::new("-t"),
            OsStr::new(id),
            OsStr::new(PANE_FORMAT),
        ])?;
        let fields: Vec<_> = output.split('\t').collect();
        Ok(fields.len() == PANE_FIELDS
            && fields[0] == id
            && fields[4] == "1"
            && fields[9] == binding.binding_id.to_string()
            && fields[10] == binding.generation.to_string()
            && fields[11] == "ui")
    }

    pub fn launch_cli(
        &self,
        project: &Path,
        executable: &Path,
        arguments: &[OsString],
        inside_pane: Option<&str>,
    ) -> io::Result<PaneIdentity> {
        let project = project.canonicalize()?;
        let executable = executable.canonicalize()?;
        let shell = shell_command(&executable, arguments)?;
        let name = format!("prism-{}", Uuid::new_v4().simple());
        let mut args: Vec<&OsStr> = Vec::new();
        if let Some(pane) = inside_pane {
            let current = self.pane(pane)?;
            args.extend([
                OsStr::new("new-window"),
                OsStr::new("-d"),
                OsStr::new("-P"),
                OsStr::new("-F"),
                OsStr::new("#{pane_id}"),
                OsStr::new("-t"),
            ]);
            // Target the exact existing session, never a recently active window.
            let session = format!("{}:", current.identity.session_id);
            args.push(OsStr::new(&session));
            args.extend([
                OsStr::new("-n"),
                OsStr::new(&name),
                OsStr::new("-c"),
                project.as_os_str(),
                OsStr::new(&shell),
            ]);
            let id = self.output(&args)?;
            return Ok(self.pane(&id)?.identity);
        }
        args.extend([
            OsStr::new("new-session"),
            OsStr::new("-d"),
            OsStr::new("-P"),
            OsStr::new("-F"),
            OsStr::new("#{pane_id}"),
            OsStr::new("-s"),
            OsStr::new(&name),
            OsStr::new("-x"),
            OsStr::new("160"),
            OsStr::new("-y"),
            OsStr::new("48"),
            OsStr::new("-c"),
            project.as_os_str(),
            OsStr::new(&shell),
        ]);
        let id = self.output(&args)?;
        Ok(self.pane(&id)?.identity)
    }

    pub fn tag_cli(&self, binding: &LiveBinding) -> io::Result<()> {
        self.verify_identity(binding)?;
        let pane = self.pane(&binding.cli.pane_id)?;
        if pane.identity != binding.cli
            || (!pane.binding.is_empty() && pane.binding != binding.binding_id.to_string())
        {
            return Err(invalid("CLI pane belongs to a different binding"));
        }
        let condition = self.pane_condition(binding, &binding.cli.pane_id, binding.cli.pid, None);
        let command = tag_commands(&binding.cli.pane_id, binding, "cli");
        self.guarded_output(&binding.cli.pane_id, &condition, &command)?;
        self.verify_binding(binding)
    }

    fn pane_condition(
        &self,
        binding: &LiveBinding,
        pane: &str,
        pid: u32,
        role: Option<&str>,
    ) -> String {
        let mut comparisons = vec![
            format!("#{{==:#{{pid}},{}}}", binding.tmux.pid),
            format!("#{{==:#{{pane_id}},{pane}}}"),
            format!("#{{==:#{{pane_pid}},{pid}}}"),
            format!("#{{==:#{{window_id}},{}}}", binding.cli.window_id),
            format!("#{{==:#{{session_id}},{}}}", binding.cli.session_id),
            "#{==:#{pane_dead},0}".to_owned(),
        ];
        if let Some(role) = role {
            comparisons.extend([
                format!("#{{==:#{{@prism_binding}},{}}}", binding.binding_id),
                format!("#{{==:#{{@prism_generation}},{}}}", binding.generation),
                format!("#{{==:#{{@prism_role}},{role}}}"),
            ]);
        } else {
            comparisons.push(format!(
                "#{{||:#{{==:#{{@prism_binding}},}},#{{==:#{{@prism_binding}},{}}}}}",
                binding.binding_id
            ));
        }
        comparisons
            .into_iter()
            .reduce(|left, right| format!("#{{&&:{left},{right}}}"))
            .expect("identity comparisons are nonempty")
    }

    fn guarded_output(&self, pane: &str, condition: &str, commands: &str) -> io::Result<String> {
        // The predicate and its command queue execute in the same tmux server connection.
        // A server restart before this connection cannot run commands on recycled pane IDs.
        let output = self.output(&[
            OsStr::new("if-shell"),
            OsStr::new("-F"),
            OsStr::new("-t"),
            OsStr::new(pane),
            OsStr::new(condition),
            OsStr::new(commands),
            OsStr::new("display-message -p PRISM_IDENTITY_CHANGED"),
        ])?;
        if output == "PRISM_IDENTITY_CHANGED" {
            return Err(invalid("tmux identity changed before mutation"));
        }
        Ok(output)
    }

    fn verify_identity(&self, binding: &LiveBinding) -> io::Result<()> {
        if binding.retired
            || self.identity()? != binding.tmux
            || process_start_time(binding.server_pid)? != binding.server_start_time
        {
            return Err(invalid("live server identity changed or binding retired"));
        }
        Ok(())
    }

    fn verify_pane(
        &self,
        expected: &PaneIdentity,
        binding: &LiveBinding,
        role: &str,
    ) -> io::Result<PaneState> {
        let pane = self.pane(&expected.pane_id)?;
        if pane.identity != *expected
            || pane.binding != binding.binding_id.to_string()
            || pane.generation != binding.generation.to_string()
            || pane.role != role
        {
            return Err(invalid("pane identity or binding generation changed"));
        }
        Ok(pane)
    }

    /// Native CLI/server liveness is independent of the optional UI pane.
    pub fn verify_binding(&self, binding: &LiveBinding) -> io::Result<()> {
        self.verify_identity(binding)?;
        self.verify_pane(&binding.cli, binding, "cli")?;
        Ok(())
    }

    pub fn ensure_ui(
        &self,
        binding: &mut LiveBinding,
        binding_path: &Path,
        executable: &Path,
    ) -> io::Result<PaneIdentity> {
        let _lock = lock_binding(binding_path)?;
        let stored = read_binding(binding_path)?;
        if !same_authority(&stored, binding)
            || stored.generation != binding.generation
            || stored.token != binding.token
            || stored.retired
            || binding
                .conversation
                .as_ref()
                .is_some_and(|conversation| stored.conversation.as_ref() != Some(conversation))
        {
            return Err(invalid("credential file does not match live binding"));
        }
        // A bridge may have bound the conversation or another request may have opened the UI.
        *binding = stored;
        self.verify_binding(binding)?;
        if let Some(expected) = &binding.prism {
            // Missing panes may be recreated; existing panes with changed identity must never be reused.
            let ids = self.output(&[
                OsStr::new("list-panes"),
                OsStr::new("-a"),
                OsStr::new("-F"),
                OsStr::new("#{pane_id}"),
            ])?;
            if ids.lines().any(|id| id == expected.pane_id) {
                if self.owned_dead_pane(&expected.pane_id, binding)? {
                    // remain-on-exit keeps the exited UI pane; only our own tagged dead pane is
                    // removed so a fresh one can take its place.
                    let condition = format!(
                        "#{{&&:#{{==:#{{pane_dead}},1}},#{{&&:#{{==:#{{@prism_binding}},{}}},#{{==:#{{@prism_role}},ui}}}}}}",
                        binding.binding_id
                    );
                    let command = format!("kill-pane -t {}", tmux_quote(&expected.pane_id));
                    self.guarded_output(&expected.pane_id, &condition, &command)?;
                } else {
                    let pane = self.verify_pane(expected, binding, "ui")?;
                    let cli = self.verify_pane(&binding.cli, binding, "cli")?;
                    verify_adjacent(&cli, &pane)?;
                    return Ok(pane.identity);
                }
            }
            binding.prism = None;
        }
        let executable = executable.canonicalize()?;
        let arguments = vec![
            OsString::from("ui"),
            OsString::from("--binding"),
            binding_path.as_os_str().to_owned(),
        ];
        let shell = shell_command(&executable, &arguments)?;
        let condition =
            self.pane_condition(binding, &binding.cli.pane_id, binding.cli.pid, Some("cli"));
        let command = format!(
            "split-window -h -p 45 -P -F '#{{pane_id}}\t#{{pane_pid}}' -t {} -c {} {} ; {} ; select-pane -t {}",
            tmux_quote(&binding.cli.pane_id),
            tmux_quote(
                binding
                    .project
                    .to_str()
                    .ok_or_else(|| invalid("project path is not UTF-8"))?
            ),
            tmux_quote(&shell),
            tag_commands("", binding, "ui"),
            tmux_quote(&binding.cli.pane_id),
        );
        let created = self.guarded_output(&binding.cli.pane_id, &condition, &command)?;
        let (id, pid) = created
            .split_once('\t')
            .ok_or_else(|| invalid("invalid created pane identity"))?;
        let pid = pid
            .parse()
            .map_err(|_| invalid("invalid created pane PID"))?;
        let result = (|| {
            self.verify_binding(binding)?;
            let pane = self.pane(id)?;
            if pane.identity.pid != pid {
                return Err(invalid("created pane process changed"));
            }
            self.verify_pane(&pane.identity, binding, "ui")?;
            let cli = self.verify_pane(&binding.cli, binding, "cli")?;
            verify_adjacent(&cli, &pane)?;
            binding.prism = Some(pane.identity.clone());
            write_binding_locked(binding_path, binding)?;
            Ok(pane.identity)
        })();
        if result.is_err() {
            let condition = self.pane_condition(binding, id, pid, Some("ui"));
            let command = format!("kill-pane -t {}", tmux_quote(id));
            let _ = self.guarded_output(id, &condition, &command);
            binding.prism = None;
        }
        result
    }

    /// The caller owns the client process and must reap it when attachment ends.
    pub fn attach(&self, binding: &LiveBinding) -> io::Result<Child> {
        self.verify_binding(binding)?;
        let condition =
            self.pane_condition(binding, &binding.cli.pane_id, binding.cli.pid, Some("cli"));
        let attach = format!("attach-session -t {}", tmux_quote(&binding.cli.session_id));
        self.command()
            .args(["if-shell", "-F"])
            .arg("-t")
            .arg(&binding.cli.pane_id)
            .arg(condition)
            .arg(attach)
            .arg("display-message -p 'Prism binding changed; attachment refused' ; run-shell 'exit 1'")
            .spawn()
    }

    pub fn select(&self, binding: &LiveBinding) -> io::Result<()> {
        self.verify_binding(binding)?;
        let condition =
            self.pane_condition(binding, &binding.cli.pane_id, binding.cli.pid, Some("cli"));
        let commands = format!(
            "select-window -t {} ; select-pane -t {}",
            tmux_quote(&binding.cli.window_id),
            tmux_quote(&binding.cli.pane_id)
        );
        self.guarded_output(&binding.cli.pane_id, &condition, &commands)?;
        Ok(())
    }

    /// Cleanup remains possible after the native server exits, but never adopts another pane.
    pub fn cleanup_cli(&self, binding: &LiveBinding) -> io::Result<()> {
        if self.identity()? != binding.tmux {
            return Err(invalid("tmux server identity changed"));
        }
        self.verify_pane(&binding.cli, binding, "cli")?;
        let condition =
            self.pane_condition(binding, &binding.cli.pane_id, binding.cli.pid, Some("cli"));
        self.guarded_output(
            &binding.cli.pane_id,
            &condition,
            &format!("kill-pane -t {}", tmux_quote(&binding.cli.pane_id)),
        )?;
        Ok(())
    }
}

fn verify_adjacent(cli: &PaneState, ui: &PaneState) -> io::Result<()> {
    if ui.identity.window_id != cli.identity.window_id
        || ui.identity.session_id != cli.identity.session_id
    {
        return Err(invalid("Prism pane is not in the bound CLI's window"));
    }
    // A zoomed window reports full-size geometry for the zoomed pane; ownership tags still hold.
    if cli.zoomed {
        return Ok(());
    }
    if ui.left != cli.left + cli.width + 1 || ui.top != cli.top || ui.height != cli.height {
        return Err(invalid(
            "Prism pane is not adjacent to the right of the bound CLI",
        ));
    }
    Ok(())
}

fn shell_command(executable: &Path, arguments: &[OsString]) -> io::Result<String> {
    let mut result = String::from("exec");
    for argument in
        std::iter::once(executable.as_os_str()).chain(arguments.iter().map(OsString::as_os_str))
    {
        let argument = argument
            .to_str()
            .ok_or_else(|| invalid("command argument is not UTF-8"))?;
        if argument.contains('\0') {
            return Err(invalid("command argument contains NUL"));
        }
        result.push_str(" '");
        result.push_str(&argument.replace('\'', "'\\''"));
        result.push('\'');
    }
    Ok(result)
}

fn tmux_quote(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "'\\''"))
}

fn tag_commands(pane: &str, binding: &LiveBinding, role: &str) -> String {
    let target = if pane.is_empty() {
        String::new()
    } else {
        format!(" -t {}", tmux_quote(pane))
    };
    [
        ("@prism_binding", binding.binding_id.to_string()),
        ("@prism_generation", binding.generation.to_string()),
        ("@prism_role", role.to_owned()),
    ]
    .into_iter()
    .map(|(key, value)| format!("set-option -p{target} {key} {}", tmux_quote(&value)))
    .collect::<Vec<_>>()
    .join(" ; ")
}
