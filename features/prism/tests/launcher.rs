use std::io::Write;
use std::process::{Command, Stdio};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_prism")
}

#[test]
fn launcher_requires_explicit_supported_agent_and_exact_resume_uuid() {
    for args in [
        vec![],
        vec!["--agent", "gemini"],
        vec!["--agent", "codex", "--resume", "latest"],
        vec!["--resume", "00000000-0000-0000-0000-000000000001"],
    ] {
        assert!(
            !Command::new(binary())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let output = Command::new(binary()).arg("--help").output().unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success());
    assert!(help.contains("--detach"));
    assert!(help.contains("codex") && help.contains("claude"));
    assert!(!help.contains("supervise"));
    assert!(!help.contains("native-cli"));
}

#[test]
fn mcp_routes_without_launcher_registration_and_reports_missing_identity() {
    let mut child = Command::new(binary())
        .arg("mcp")
        .env_remove("PRISM_BINDING_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-03-26"}}}}"#).unwrap();
    writeln!(input, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"prism_inspect","arguments":{{}},"_meta":{{"threadId":"00000000-0000-0000-0000-000000000001"}}}}}}"#).unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let replies: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(replies[1]["result"]["isError"], true);
    assert!(
        replies[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("prism --agent codex --resume")
    );
}

#[test]
fn supervisor_rejects_arbitrary_binding_locations_without_starting_native_processes() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args(["supervise", "--binding"])
        .arg(directory.path().join("binding.json"))
        .args(["--codex", "/does/not/exist"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing")
            || String::from_utf8_lossy(&output.stderr).contains("invalid launcher binding path")
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
