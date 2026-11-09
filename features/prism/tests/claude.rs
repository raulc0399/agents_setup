use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const UNREGISTERED_INSTRUCTION: &str = "Prism needs tmux. Start codex or claude inside tmux, then ask to open Prism in that conversation.";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/claude")
}

fn json(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(root().join(path)).unwrap()).unwrap()
}

#[test]
fn native_package_has_the_supported_shape() {
    let manifest = json(".claude-plugin/plugin.json");
    assert_eq!(manifest["name"], "prism");
    assert_eq!(manifest["version"], "0.1.0");
    assert!(manifest["description"].is_string());
    let hooks = json("hooks/hooks.json");
    assert_eq!(hooks["modules"], serde_json::json!(["./prism.js"]));
    let module = fs::read_to_string(root().join("hooks/prism.js")).unwrap();
    assert!(module.contains("export function register("));
    assert!(root().join("skills/prism-workspace/SKILL.md").is_file());
    let marketplace = json(".claude-plugin/marketplace.json");
    assert_eq!(marketplace["name"], "prism-local");
    assert_eq!(marketplace["plugins"][0]["name"], "prism");
    assert_eq!(marketplace["plugins"][0]["source"], "./");
}

#[test]
fn package_never_embeds_binding_values_or_credentials() {
    for path in [
        "hooks/prism.js",
        "hooks/tools.json",
        "hooks/hooks.json",
        ".claude-plugin/plugin.json",
        ".claude-plugin/marketplace.json",
        "skills/prism-workspace/SKILL.md",
        "README.md",
    ] {
        let text = fs::read_to_string(root().join(path)).unwrap();
        for line in text
            .lines()
            .filter(|line| line.contains("PRISM_BINDING_FILE"))
        {
            assert!(
                !line.contains("PRISM_BINDING_FILE="),
                "{path} assigns a binding path"
            );
        }
        // A binding token is 64 hex characters; no file may carry one.
        let longest_hex_run = text
            .split(|character: char| !character.is_ascii_hexdigit())
            .map(str::len)
            .max()
            .unwrap_or(0);
        assert!(longest_hex_run < 64, "{path} embeds a token literal");
        assert!(
            !text.contains(".prism/run/"),
            "{path} hardcodes a binding path"
        );
    }
}

#[test]
fn tool_schemas_match_the_shared_mcp_definitions() {
    let shared = prism::mcp::tools();
    let shared = shared["tools"].as_array().unwrap();
    let package = json("hooks/tools.json");
    let package = package.as_array().unwrap();
    assert_eq!(package.len(), shared.len());
    for tool in package {
        let name = tool["name"].as_str().unwrap();
        let expected = shared
            .iter()
            .find(|entry| entry["name"] == format!("prism_{name}"))
            .unwrap_or_else(|| panic!("no shared tool for {name}"));
        assert_eq!(tool["inputSchema"], expected["inputSchema"], "{name}");
        assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
        assert!(!tool["description"].as_str().unwrap().contains("Codex"));
    }
}

#[test]
fn module_wires_every_tool_the_command_and_the_exact_instruction() {
    let module = fs::read_to_string(root().join("hooks/prism.js")).unwrap();
    for name in ["publish", "inspect", "action", "questions", "answer"] {
        assert!(
            module.contains(&format!("{{tool: 'mcp__prism__{name}'}}")),
            "{name} hook missing"
        );
    }
    assert!(module.contains("{command: 'prism'}"));
    assert!(module.contains("'session.end'"));
    assert!(module.contains(&format!("'{UNREGISTERED_INSTRUCTION}'")));
    let skill = fs::read_to_string(root().join("skills/prism-workspace/SKILL.md")).unwrap();
    for name in ["publish", "inspect", "action", "questions", "answer"] {
        assert!(skill.contains(&format!("mcp__prism__{name}")));
    }
    assert!(skill.contains("/prism"));
    assert!(skill.contains("inside tmux"));
    // A skill named `prism` claims `/prism:prism` and the CLI refuses the mod's `/prism` command.
    assert!(skill.starts_with("---\nname: prism-workspace\n"));
    assert!(module.contains("e.agentId !== undefined"));
}
