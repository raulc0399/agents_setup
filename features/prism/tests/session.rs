use prism::model::AgentKind;
use prism::session::{LiveBinding, RuntimeDirectory, TmuxClient, read_binding};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use uuid::Uuid;

struct Server {
    name: String,
    client: TmuxClient,
}

impl Server {
    fn new() -> Self {
        let name = format!("prism-test-{}", Uuid::new_v4());
        Self {
            client: TmuxClient::named(&name),
            name,
        }
    }

    fn command(&self, arguments: &[&str]) -> String {
        let result = Command::new("tmux")
            .arg("-L")
            .arg(&self.name)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    }

    fn binding(&self, project: &Path) -> LiveBinding {
        let cli = self
            .client
            .launch_cli(project, Path::new("/usr/bin/cat"), &[], None)
            .unwrap();
        let binding = LiveBinding::new(
            project,
            AgentKind::Codex,
            "unix:///test-owned-native-server".into(),
            std::process::id(),
            self.client.identity().unwrap(),
            cli,
        )
        .unwrap();
        self.client.tag_cli(&binding).unwrap();
        binding
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-L")
            .arg(&self.name)
            .arg("kill-server")
            .output();
    }
}

fn ui_executable(project: &Path) -> std::path::PathBuf {
    let executable = project.join("ui executable ' quoted");
    fs::write(&executable, "#!/bin/sh\nexec /usr/bin/cat\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    executable
}

#[test]
fn credentials_are_private_atomic_and_bound_to_the_canonical_project() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    assert_eq!(binding.token.len(), 64);
    assert!(binding.workspace_key().is_err());
    runtime.write_binding(&binding).unwrap();
    assert_eq!(fs::metadata(runtime.path()).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(runtime.binding_path()).unwrap().mode() & 0o777,
        0o600
    );
    let original_inode = fs::metadata(runtime.binding_path()).unwrap().ino();
    binding.bind_conversation("thread-exact").unwrap();
    runtime.write_binding(&binding).unwrap();
    assert_ne!(
        fs::metadata(runtime.binding_path()).unwrap().ino(),
        original_inode
    );
    let restored = read_binding(&runtime.binding_path()).unwrap();
    assert_eq!(
        restored.workspace_key().unwrap().conversation,
        "thread-exact"
    );
    assert_eq!(restored.token, binding.token);
    fs::set_permissions(runtime.binding_path(), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_binding(&runtime.binding_path()).is_err());
    assert!(runtime.write_binding(&binding).is_err());
}

#[test]
fn runtime_rejects_symlinks_and_shared_permissions() {
    let project = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    symlink(target.path(), project.path().join(".prism")).unwrap();
    assert!(RuntimeDirectory::create(project.path()).is_err());
    let project = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".prism")).unwrap();
    fs::set_permissions(
        project.path().join(".prism"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(RuntimeDirectory::create(project.path()).is_err());
}

#[test]
fn conversation_switch_retires_generation_without_rebinding() {
    let project = TempDir::new().unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    binding.bind_conversation("thread-first").unwrap();
    binding.bind_conversation("thread-first").unwrap();
    assert!(binding.bind_conversation("thread-second").is_err());
    assert_eq!(binding.conversation.as_deref(), Some("thread-first"));
    assert!(binding.retired);
    assert_eq!(binding.generation, 2);
    assert!(binding.token.is_empty());
    assert!(binding.workspace_key().is_err());
    assert!(server.client.verify_binding(&binding).is_err());
}

#[test]
fn ui_is_reused_only_in_its_owned_adjacent_pane() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    let ui = ui_executable(project.path());
    runtime.write_binding(&binding).unwrap();
    let pane = server
        .client
        .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
        .unwrap();
    assert_eq!(
        pane,
        server
            .client
            .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
            .unwrap()
    );
    server.command(&[
        "set-option",
        "-p",
        "-t",
        &pane.pane_id,
        "@prism_binding",
        &Uuid::new_v4().to_string(),
    ]);
    assert!(
        server
            .client
            .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
            .is_err()
    );
    assert!(server.client.verify_binding(&binding).is_ok());
    server.command(&["kill-pane", "-t", &pane.pane_id]);
    assert!(server.client.verify_binding(&binding).is_ok());
    let replacement = server
        .client
        .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
        .unwrap();
    assert_ne!(replacement.pane_id, pane.pane_id);
    server.command(&[
        "swap-pane",
        "-s",
        &binding.cli.pane_id,
        "-t",
        &replacement.pane_id,
    ]);
    assert!(
        server
            .client
            .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
            .is_err()
    );
}

#[test]
fn restarted_server_cannot_reuse_old_pane_identity() {
    let project = TempDir::new().unwrap();
    let server = Server::new();
    let binding = server.binding(project.path());
    server.command(&["kill-server"]);
    // Wait for the owned old tmux process to exit before reusing its socket name.
    for _ in 0..100 {
        if prism::session::process_start_time(binding.tmux.pid).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let replacement = server.binding(project.path());
    assert_eq!(replacement.cli.pane_id, binding.cli.pane_id);
    assert_ne!(replacement.tmux, binding.tmux);
    assert!(server.client.verify_binding(&binding).is_err());
    assert!(server.client.verify_binding(&replacement).is_ok());
}

#[test]
fn launch_preserves_shell_metacharacters_and_creates_a_separate_window_inside_tmux() {
    let project = TempDir::new().unwrap();
    let server = Server::new();
    let binding = server.binding(project.path());
    let executable = project.path().join("capture ' executable");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' \"$1\" > argument.txt\nexec /usr/bin/cat\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let argument = "literal ' ; $(touch BAD) `touch BAD2` \"\nsecond line";
    let pane = server
        .client
        .launch_cli(
            project.path(),
            &executable,
            &[OsString::from(argument)],
            Some(&binding.cli.pane_id),
        )
        .unwrap();
    for _ in 0..100 {
        if project.path().join("argument.txt").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(project.path().join("argument.txt")).unwrap(),
        argument
    );
    assert!(!project.path().join("BAD").exists());
    assert!(!project.path().join("BAD2").exists());
    assert_eq!(pane.session_id, binding.cli.session_id);
    assert_ne!(pane.window_id, binding.cli.window_id);
}

#[test]
fn stale_generation_and_recycled_cli_pid_are_rejected() {
    let project = TempDir::new().unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    binding.generation += 1;
    assert!(server.client.verify_binding(&binding).is_err());
    binding.generation -= 1;
    binding.cli.start_time = "old-process".into();
    assert!(server.client.verify_binding(&binding).is_err());
}

#[test]
fn stale_ui_snapshot_preserves_conversation_and_reuses_persisted_pane() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    let mut stale = binding.clone();
    let ui = ui_executable(project.path());
    binding.bind_conversation("native-thread-bound").unwrap();
    runtime.write_binding(&binding).unwrap();
    let first = server
        .client
        .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
        .unwrap();
    let second = server
        .client
        .ensure_ui(&mut stale, &runtime.binding_path(), &ui)
        .unwrap();
    assert_eq!(
        first, second,
        "a stale client must reuse the persisted UI pane"
    );
    assert_eq!(
        read_binding(&runtime.binding_path())
            .unwrap()
            .conversation
            .as_deref(),
        Some("native-thread-bound")
    );
    assert_eq!(
        server
            .command(&["list-panes", "-a", "-F", "#{pane_id}"])
            .lines()
            .count(),
        2
    );
}

#[test]
fn concurrent_ui_requests_create_one_pane() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let server = Server::new();
    let binding = server.binding(project.path());
    runtime.write_binding(&binding).unwrap();
    let ui = ui_executable(project.path());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let mut binding = binding.clone();
            let client = server.client.clone();
            let path = runtime.binding_path();
            let ui = ui.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                client.ensure_ui(&mut binding, &path, &ui).unwrap()
            })
        })
        .collect();
    let panes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(panes[0], panes[1]);
    assert_eq!(
        server
            .command(&["list-panes", "-a", "-F", "#{pane_id}"])
            .lines()
            .count(),
        2
    );
}

#[test]
fn restart_between_verification_and_mutation_never_runs_or_kills_a_replacement_pane() {
    // Each child gets its own PATH; no process-global environment mutation races other tests.
    for mode in ["split", "cleanup"] {
        let scripts = TempDir::new().unwrap();
        let shim = scripts.path().join("tmux");
        fs::write(
            &shim,
            r#"#!/bin/sh
case "$*" in
  *split-window*)
    if [ "$PRISM_TEST_RACE" = split ]; then
      /usr/bin/tmux "$1" "$2" kill-server
      /usr/bin/sleep 0.1
      /usr/bin/tmux "$1" "$2" new-session -d -s replacement -x 160 -y 48 /usr/bin/cat
    fi
    /usr/bin/tmux "$@"
    result=$?
    if [ "$PRISM_TEST_RACE" = cleanup ]; then
      /usr/bin/chmod 644 "$(/usr/bin/cat "$PRISM_TEST_BINDING_RECORD")"
    fi
    exit "$result"
    ;;
  *kill-pane*)
    if [ "$PRISM_TEST_RACE" = cleanup ]; then
      /usr/bin/tmux "$1" "$2" kill-server
      /usr/bin/sleep 0.1
      /usr/bin/tmux "$1" "$2" new-session -d -s replacement -x 160 -y 48 /usr/bin/cat
      /usr/bin/tmux "$1" "$2" split-window -hd -t %0 /usr/bin/cat
    fi
    ;;
esac
exec /usr/bin/tmux "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
        let path = format!(
            "{}:{}",
            scripts.path().display(),
            std::env::var("PATH").unwrap()
        );
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "restart_race_child", "--nocapture"])
            .env("PATH", path)
            .env("PRISM_TEST_RACE", mode)
            .env(
                "PRISM_TEST_BINDING_RECORD",
                scripts.path().join("binding-path"),
            )
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn restart_race_child() {
    let Ok(mode) = std::env::var("PRISM_TEST_RACE") else {
        return;
    };
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    runtime.write_binding(&binding).unwrap();
    let executable = project.path().join("ui");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf ran > UI_RAN\nexec /usr/bin/cat\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        std::env::var("PRISM_TEST_BINDING_RECORD").unwrap(),
        runtime.binding_path().as_os_str().as_encoded_bytes(),
    )
    .unwrap();
    assert!(
        server
            .client
            .ensure_ui(&mut binding, &runtime.binding_path(), &executable)
            .is_err()
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_ne!(
        server.client.identity().unwrap(),
        binding.tmux,
        "race shim must replace the original server"
    );
    if mode == "split" {
        assert!(
            !project.path().join("UI_RAN").exists(),
            "UI executed on replacement server"
        );
        assert_eq!(
            server
                .command(&["list-panes", "-a", "-F", "#{pane_id}"])
                .lines()
                .count(),
            1
        );
    } else {
        assert_eq!(
            server
                .command(&["list-panes", "-a", "-F", "#{pane_id}"])
                .lines()
                .count(),
            2,
            "cleanup killed a replacement pane"
        );
    }
}

#[test]
fn stale_bridge_writes_preserve_conversation_and_ui_and_cannot_revive_retirement() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    let stale = binding.clone();
    runtime.write_binding(&binding).unwrap();
    binding.bind_conversation("native-thread-bound").unwrap();
    runtime.write_binding(&binding).unwrap();
    let ui = ui_executable(project.path());
    let pane = server
        .client
        .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
        .unwrap();
    runtime.write_binding(&stale).unwrap();
    let stored = read_binding(&runtime.binding_path()).unwrap();
    assert_eq!(stored.conversation.as_deref(), Some("native-thread-bound"));
    assert_eq!(stored.prism, Some(pane));
    binding.retire().unwrap();
    runtime.write_binding(&binding).unwrap();
    assert!(runtime.write_binding(&stale).is_err());
    assert!(read_binding(&runtime.binding_path()).unwrap().retired);
}

#[test]
fn select_and_cleanup_only_mutate_the_tagged_cli_pane() {
    let project = TempDir::new().unwrap();
    let server = Server::new();
    let binding = server.binding(project.path());
    server.client.select(&binding).unwrap();
    assert_eq!(
        server.command(&["display-message", "-p", "#{pane_id}"]),
        binding.cli.pane_id
    );
    server.command(&[
        "set-option",
        "-p",
        "-t",
        &binding.cli.pane_id,
        "@prism_role",
        "unrelated",
    ]);
    assert!(server.client.select(&binding).is_err());
    assert!(server.client.cleanup_cli(&binding).is_err());
    assert_eq!(
        server.command(&["list-panes", "-a", "-F", "#{pane_id}"]),
        binding.cli.pane_id
    );
    server.command(&[
        "set-option",
        "-p",
        "-t",
        &binding.cli.pane_id,
        "@prism_role",
        "cli",
    ]);
    server.client.cleanup_cli(&binding).unwrap();
}

#[test]
fn cleanup_does_not_require_the_native_server_to_remain_alive() {
    let project = TempDir::new().unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    binding.server_pid = u32::MAX;
    assert!(server.client.select(&binding).is_err());
    server.client.cleanup_cli(&binding).unwrap();
}

#[test]
fn dead_or_zoomed_owned_ui_pane_is_recovered() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let server = Server::new();
    let mut binding = server.binding(project.path());
    let ui = ui_executable(project.path());
    runtime.write_binding(&binding).unwrap();
    let pane = server
        .client
        .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
        .unwrap();
    // Zooming the CLI pane changes geometry but not ownership; the pane is reused.
    server.command(&["resize-pane", "-Z", "-t", &binding.cli.pane_id]);
    assert_eq!(
        pane,
        server
            .client
            .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
            .unwrap()
    );
    server.command(&["resize-pane", "-Z", "-t", &binding.cli.pane_id]);
    // With remain-on-exit the pane survives its process as a dead pane; it must be replaced.
    server.command(&[
        "set-option",
        "-p",
        "-t",
        &pane.pane_id,
        "remain-on-exit",
        "on",
    ]);
    server.command(&["send-keys", "-t", &pane.pane_id, "C-d"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server.command(&["display-message", "-p", "-t", &pane.pane_id, "#{pane_dead}"]) != "1" {
        assert!(std::time::Instant::now() < deadline, "pane never died");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let replacement = server
        .client
        .ensure_ui(&mut binding, &runtime.binding_path(), &ui)
        .unwrap();
    assert_ne!(replacement.pane_id, pane.pane_id);
    assert_eq!(binding.prism, Some(replacement.clone()));
    assert_eq!(
        read_binding(&runtime.binding_path()).unwrap().prism,
        Some(replacement)
    );
}

#[test]
fn bridge_socket_uses_tmp_even_for_long_project_paths() {
    let root = TempDir::new().unwrap();
    let project = root.path().join("p".repeat(120));
    fs::create_dir(&project).unwrap();
    let runtime = RuntimeDirectory::create(&project).unwrap();
    let socket = prism::bridge::socket_path(&runtime.binding_path()).unwrap();
    assert!(socket.starts_with("/tmp"));
    assert!(!socket.starts_with(&project));
    assert!(socket.as_os_str().len() < 108);
    let directory = socket.parent().unwrap();
    assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    drop(listener);
    fs::remove_file(socket).unwrap();
    prism::session::remove_socket_directory(&runtime.binding_path()).unwrap();
}

#[test]
fn socket_directory_rejects_symlinks_and_public_permissions() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let directory = prism::session::socket_directory(&runtime.binding_path()).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(prism::bridge::socket_path(&runtime.binding_path()).is_err());
    fs::remove_dir(&directory).unwrap();
    symlink(project.path(), &directory).unwrap();
    assert!(prism::bridge::socket_path(&runtime.binding_path()).is_err());
    assert!(prism::session::remove_socket_directory(&runtime.binding_path()).is_err());
    fs::remove_file(directory).unwrap();
}
