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
    assert_eq!(
        mcp["mcpServers"]["prism"]["env_vars"],
        serde_json::json!(["PRISM_BINDING_FILE"])
    );
    assert!(manifest.get("hooks").is_none());
    assert!(root.join("skills/prism/SKILL.md").is_file());
}
