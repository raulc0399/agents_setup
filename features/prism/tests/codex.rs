use serde_json::Value;
use std::fs;
use std::path::Path;

#[test]
fn native_package_uses_bound_mcp_and_supported_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/codex");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".codex-plugin/plugin.json")).unwrap())
            .unwrap();
    let mcp: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(manifest["mcpServers"], "./.mcp.json");
    assert_eq!(mcp["mcpServers"]["prism"]["command"], "prism");
    // The MCP process attaches by its own PID; only the app link overrides reach its bridge.
    assert_eq!(
        mcp["mcpServers"]["prism"]["env_vars"],
        serde_json::json!(["PRISM_APP_FILE", "PRISM_APP_HOST"])
    );
    assert!(manifest.get("hooks").is_none());
    assert!(root.join("skills/prism/SKILL.md").is_file());
}
