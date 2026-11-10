//! On-demand attachment through the real `prism` binary: `prism mcp` for Codex and
//! `prism attach` for Claude, each in a temporary project; no model requests.
use prism::model::AgentKind;
use prism::session::{LiveBinding, RuntimeDirectory, read_binding};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use uuid::Uuid;

const PRISM: &str = env!("CARGO_BIN_EXE_prism");
const WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(50);

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(POLL);
    }
}

/// Live `prism bridge` processes serving the binding at `path`.
fn bridge_pids(path: &Path) -> Vec<i32> {
    let mut pids = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(command) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let arguments: Vec<&[u8]> = command.split(|byte| *byte == 0).collect();
        if arguments.contains(&b"bridge".as_slice())
            && arguments.contains(&path.as_os_str().as_encoded_bytes())
            && prism::session::process_start_time(pid as u32).is_ok()
        {
            pids.push(pid);
        }
    }
    pids
}

fn signal(pid: i32, signal: i32) {
    // SAFETY: kill takes plain integers; the PID belongs to a bridge this test started.
    unsafe {
        libc::kill(pid, signal);
    }
}

/// A temporary project whose bridges never dial a desktop app on the host.
struct Project {
    directory: TempDir,
    path: PathBuf,
}

impl Project {
    fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().canonicalize().unwrap();
        Self { directory, path }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(PRISM);
        command
            .args(arguments)
            .current_dir(&self.path)
            .env("PRISM_APP_FILE", self.directory.path().join("no-app.json"));
        command
    }

    fn bindings(&self) -> Vec<(PathBuf, LiveBinding)> {
        let Ok(entries) = fs::read_dir(self.path.join(".prism/run")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path().join("binding.json");
                read_binding(&path).ok().map(|binding| (path, binding))
            })
            .collect()
    }

    fn live_bindings(&self) -> Vec<(PathBuf, LiveBinding)> {
        self.bindings()
            .into_iter()
            .filter(|(_, binding)| !binding.retired)
            .collect()
    }

    /// A retired runtime as an ended bridge leaves it, with its lease file.
    fn retired_runtime(&self) -> (PathBuf, File) {
        let runtime = RuntimeDirectory::create(&self.path).unwrap();
        let mut binding = LiveBinding::new(
            &self.path,
            AgentKind::Codex,
            format!("codex-cli://{}", std::process::id()),
            std::process::id(),
        )
        .unwrap();
        binding.retire().unwrap();
        runtime.write_binding(&binding).unwrap();
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(runtime.path().join("bridge.lock"))
            .unwrap();
        (runtime.path().to_owned(), lease)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        // SIGTERM lets each bridge retire its binding and remove its sockets.
        let pids: Vec<_> = self
            .bindings()
            .iter()
            .flat_map(|(path, _)| bridge_pids(path))
            .collect();
        for pid in &pids {
            signal(*pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + WAIT;
        while pids
            .iter()
            .any(|pid| prism::session::process_start_time(*pid as u32).is_ok())
            && Instant::now() < deadline
        {
            std::thread::sleep(POLL);
        }
        for (path, _) in self.bindings() {
            let _ = prism::session::remove_socket_directory(&path);
        }
    }
}

/// The Codex MCP server as the Codex CLI runs it: one process, line-delimited JSON-RPC.
struct Mcp {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(project: &Project) -> Self {
        let mut child = project
            .command(&["mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Self {
            child,
            input,
            output,
            next_id: 0,
        };
        mcp.request("initialize", json!({}));
        mcp
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let request = json!({"jsonrpc":"2.0","id":self.next_id,"method":method,"params":params});
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        response["result"].clone()
    }

    /// The tool's text and whether it is an error.
    fn tool(&mut self, thread: &str, name: &str, arguments: Value) -> (bool, String) {
        let result = self.request(
            "tools/call",
            json!({"name":name,"arguments":arguments,"_meta":{"threadId":thread}}),
        );
        (
            result["isError"].as_bool().unwrap(),
            result["content"][0]["text"].as_str().unwrap().to_owned(),
        )
    }

    fn inspect(&mut self, thread: &str) -> Value {
        let (error, text) = self.tool(thread, "prism_inspect", json!({}));
        assert!(!error, "{text}");
        serde_json::from_str(&text).unwrap()
    }

    fn exit(&mut self) {
        drop(self.input.take());
        self.child.wait().unwrap();
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn codex_mcp_attaches_lazily_reuses_restarts_prunes_and_retires() {
    let project = Project::new();
    let (pruned, lease) = project.retired_runtime();
    drop(lease);
    let (kept, held) = project.retired_runtime();
    held.lock().unwrap();
    let mut mcp = Mcp::start(&project);
    let thread = Uuid::new_v4().to_string();
    assert!(project.live_bindings().is_empty());

    // The first tool call attaches the MCP process and prunes the free retired runtime.
    let (error, text) = mcp.tool(
        &thread,
        "prism_publish",
        json!({"request_id":Uuid::new_v4(),"document_id":null,"expected_revision":null,"title":"Existing conversation","elements":[],"relationships":[],"groups":[]}),
    );
    assert!(!error, "{text}");
    let published: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(published["data"]["revision"], 1);
    assert!(published.get("pane").is_none());
    assert!(!pruned.exists());
    assert!(kept.exists(), "a held lease keeps its runtime");
    let [(path, binding)] = project.live_bindings().try_into().ok().unwrap();
    assert_eq!(binding.conversation.as_deref(), Some(thread.as_str()));
    assert_eq!(binding.server_pid, mcp.child.id());

    // Later calls reuse the binding; a second bridge for it is refused.
    assert_eq!(mcp.inspect(&thread)["data"]["total"], 1);
    assert_eq!(project.live_bindings()[0].0, path);
    let duplicate = project
        .command(&["bridge", "--binding", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already owns"));

    // A bridge that dies without retiring its binding is restarted by the next call.
    for pid in bridge_pids(&path) {
        signal(pid, libc::SIGKILL);
    }
    wait_until("the bridge did not die", || bridge_pids(&path).is_empty());
    assert_eq!(mcp.inspect(&thread)["data"]["total"], 1);
    assert_eq!(project.live_bindings()[0].0, path);
    assert!(!bridge_pids(&path).is_empty());

    // Another thread through the same MCP process is refused without retiring the binding.
    let (error, text) = mcp.tool(&Uuid::new_v4().to_string(), "prism_inspect", json!({}));
    assert!(error);
    assert!(
        text.contains("already attached to another live conversation"),
        "{text}"
    );
    assert!(!read_binding(&path).unwrap().retired);

    // The binding retires when the MCP process exits.
    mcp.exit();
    wait_until("the binding did not retire", || {
        read_binding(&path).unwrap().retired
    });
    drop(held);
}

#[test]
fn claude_attach_retires_a_stale_session_of_the_same_process() {
    let project = Project::new();
    let attach = |conversation: &str| -> PathBuf {
        // `prism attach` records its parent, this test process, as the Claude CLI.
        let output = project
            .command(&[
                "attach",
                "--agent",
                "claude",
                "--conversation",
                conversation,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        PathBuf::from(value["binding"].as_str().unwrap())
    };
    let first_session = Uuid::new_v4().to_string();
    let first = attach(&first_session);
    let binding = read_binding(&first).unwrap();
    assert_eq!(binding.server_pid, std::process::id());
    assert_eq!(binding.agent, AgentKind::Claude);

    // `session.end` of the first session never arrived: the next session retires it.
    let second_session = Uuid::new_v4().to_string();
    let second = attach(&second_session);
    assert_ne!(second, first);
    let stale = read_binding(&first).unwrap();
    assert!(stale.retired);
    assert_eq!(stale.generation, 2);
    let current = read_binding(&second).unwrap();
    assert!(!current.retired);
    assert_eq!(
        current.conversation.as_deref(),
        Some(second_session.as_str())
    );

    // Once its bridge ended, the next attachment prunes the stale runtime.
    wait_until("the stale bridge did not end", || {
        bridge_pids(&first).is_empty()
    });
    assert_eq!(attach(&second_session), second);
    assert!(!first.parent().unwrap().exists());
}
