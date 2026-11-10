use prism::model::AgentKind;
use prism::session::{LiveBinding, RuntimeDirectory, read_binding, verify_binding};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// The test process stands in for the attached CLI.
fn binding(project: &Path) -> LiveBinding {
    LiveBinding::new(
        project,
        AgentKind::Codex,
        format!("codex-cli://{}", std::process::id()),
        std::process::id(),
    )
    .unwrap()
}

#[test]
fn credentials_are_private_atomic_and_bound_to_the_canonical_project() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let mut binding = binding(project.path());
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
fn runtime_directory_ignores_itself_in_git_and_keeps_a_user_file() {
    let project = TempDir::new().unwrap();
    RuntimeDirectory::create(project.path()).unwrap();
    let ignore = project.path().join(".prism/.gitignore");
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "*\n");
    fs::write(&ignore, "*\n!export/\n").unwrap();
    RuntimeDirectory::create(project.path()).unwrap();
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "*\n!export/\n");
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
    let mut binding = binding(project.path());
    binding.bind_conversation("thread-first").unwrap();
    binding.bind_conversation("thread-first").unwrap();
    assert!(binding.bind_conversation("thread-second").is_err());
    assert_eq!(binding.conversation.as_deref(), Some("thread-first"));
    assert!(binding.retired);
    assert_eq!(binding.generation, 2);
    assert!(binding.token.is_empty());
    assert!(binding.workspace_key().is_err());
    assert!(verify_binding(&binding).is_err());
}

#[test]
fn recycled_or_exited_cli_and_retirement_are_rejected() {
    let project = TempDir::new().unwrap();
    let mut binding = binding(project.path());
    assert!(verify_binding(&binding).is_ok());
    binding.server_start_time = "old-process".into();
    assert!(verify_binding(&binding).is_err());
    let mut exited = Command::new("true").spawn().unwrap();
    let pid = exited.id();
    exited.wait().unwrap();
    binding.server_pid = pid;
    assert!(verify_binding(&binding).is_err());
    let mut retired = LiveBinding::new(
        project.path(),
        AgentKind::Claude,
        format!("claude-cli://{}", std::process::id()),
        std::process::id(),
    )
    .unwrap();
    retired.retire().unwrap();
    assert!(verify_binding(&retired).is_err());
}

#[test]
fn binding_records_when_its_conversation_was_bound() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let mut binding = binding(project.path());
    assert_eq!(binding.bound_at_ms, 0);
    let before = prism::session::unix_time_ms();
    binding.bind_conversation("thread-timed").unwrap();
    let bound_at_ms = binding.bound_at_ms;
    assert!(bound_at_ms >= before);
    binding.bind_conversation("thread-timed").unwrap();
    assert_eq!(binding.bound_at_ms, bound_at_ms);
    runtime.write_binding(&binding).unwrap();
    // A writer that never saw the bound time cannot reset it.
    let mut stale = binding.clone();
    stale.bound_at_ms = 0;
    runtime.write_binding(&stale).unwrap();
    assert_eq!(
        read_binding(&runtime.binding_path()).unwrap().bound_at_ms,
        bound_at_ms
    );
    // Bindings written before the field existed still load, without a bound time.
    let mut older = serde_json::to_value(&binding).unwrap();
    older.as_object_mut().unwrap().remove("bound_at_ms");
    let older: LiveBinding = serde_json::from_value(older).unwrap();
    assert_eq!(older.bound_at_ms, 0);
}

#[test]
fn stale_bridge_writes_preserve_conversation_and_cannot_revive_retirement() {
    let project = TempDir::new().unwrap();
    let runtime = RuntimeDirectory::create(project.path()).unwrap();
    let mut binding = binding(project.path());
    let stale = binding.clone();
    runtime.write_binding(&binding).unwrap();
    binding.bind_conversation("native-thread-bound").unwrap();
    runtime.write_binding(&binding).unwrap();
    runtime.write_binding(&stale).unwrap();
    let stored = read_binding(&runtime.binding_path()).unwrap();
    assert_eq!(stored.conversation.as_deref(), Some("native-thread-bound"));
    binding.retire().unwrap();
    runtime.write_binding(&binding).unwrap();
    assert!(runtime.write_binding(&stale).is_err());
    assert!(read_binding(&runtime.binding_path()).unwrap().retired);
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
