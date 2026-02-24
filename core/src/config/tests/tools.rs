//! Unified tools configuration tests

use super::super::*;
use std::collections::HashMap;

#[test]
fn test_unified_tools_config_defaults() {
    let config = UnifiedToolsConfig::default();

    // Default enabled is true
    assert!(config.enabled);

    // Native tools should have empty defaults
    assert!(config.native.fs.is_none());
    assert!(config.native.git.is_none());
    assert!(config.native.shell.is_none());
    assert!(config.native.system_info.is_none());

    // MCP servers should be empty
    assert!(config.mcp.is_empty());
}

#[test]
fn test_unified_tools_config_helper_methods() {
    let mut config = UnifiedToolsConfig::default();

    // By default (None), native tools fall back to defaults:
    // fs, git, system_info default to true when not specified
    // shell defaults to false when not specified (for security)
    assert!(config.is_fs_enabled()); // defaults to true
    assert!(config.is_git_enabled()); // defaults to true
    assert!(!config.is_shell_enabled()); // defaults to false (security)
    assert!(config.is_system_info_enabled()); // defaults to true

    // Explicitly disable fs tool
    config.native.fs = Some(FsToolConfig {
        enabled: false,
        allowed_roots: vec![],
    });
    assert!(!config.is_fs_enabled());

    // Re-enable fs tool
    config.native.fs = Some(FsToolConfig {
        enabled: true,
        allowed_roots: vec!["~".to_string()],
    });
    assert!(config.is_fs_enabled());

    // Explicitly enable shell tool (disabled by default)
    config.native.shell = Some(ShellToolConfig {
        enabled: true,
        timeout_seconds: 30,
        allowed_commands: vec![],
    });
    assert!(config.is_shell_enabled());

    // Test master switch - disable all
    config.enabled = false;
    assert!(!config.is_fs_enabled()); // master switch off
    assert!(!config.is_shell_enabled()); // master switch off

    // Re-enable master switch
    config.enabled = true;
    assert!(config.is_fs_enabled());
    assert!(config.is_shell_enabled());
}

#[test]
fn test_unified_tools_config_from_legacy() {
    // Create legacy ToolsConfig
    let tools = ToolsConfig {
        fs_enabled: true,
        allowed_roots: vec![],
        git_enabled: false,
        allowed_repos: vec![],
        shell_enabled: true,
        allowed_commands: vec![],
        shell_timeout_seconds: 30,
        system_info_enabled: false,
    };

    // Create legacy McpConfig with some servers
    let mut mcp = McpConfig {
        enabled: true,
        ..McpConfig::default()
    };
    mcp.external_servers.push(McpExternalServerConfig {
        name: "github".to_string(),
        command: "node".to_string(),
        args: vec!["~/.mcp/github/index.js".to_string()],
        env: HashMap::new(),
        cwd: None,
        requires_runtime: Some("node".to_string()),
        timeout_seconds: 30,
    });

    // Convert to unified config
    let unified = UnifiedToolsConfig::from_legacy(&tools, &mcp);

    // Verify enabled is inherited from MCP
    assert!(unified.enabled);

    // Verify native tools are converted correctly
    assert!(unified.is_fs_enabled());
    assert!(!unified.is_git_enabled());
    assert!(unified.is_shell_enabled());
    assert!(!unified.is_system_info_enabled());

    // Verify MCP servers are copied
    assert_eq!(unified.mcp.len(), 1);
    assert!(unified.mcp.contains_key("github"));
}

#[test]
fn test_unified_tools_config_toml_parsing() {
    let toml_str = r#"
[unified_tools]
enabled = true

[unified_tools.native.fs]
enabled = true
allowed_roots = ["~", "/tmp"]

[unified_tools.native.git]
enabled = false
allowed_repos = []

[unified_tools.native.shell]
enabled = true
timeout_seconds = 60
allowed_commands = []

[unified_tools.mcp.github]
command = "node"
args = ["~/.mcp/github/index.js"]
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    let unified = config.unified_tools.expect("Should have unified_tools");
    assert!(unified.enabled);

    // Native tools
    let fs = unified.native.fs.expect("Should have fs config");
    assert!(fs.enabled);
    assert_eq!(fs.allowed_roots, vec!["~", "/tmp"]);

    let git = unified.native.git.expect("Should have git config");
    assert!(!git.enabled);

    let shell = unified.native.shell.expect("Should have shell config");
    assert!(shell.enabled);
    assert_eq!(shell.timeout_seconds, 60);

    // MCP servers
    assert_eq!(unified.mcp.len(), 1);
    let github = unified
        .mcp
        .get("github")
        .expect("Should have github server");
    assert_eq!(github.command, "node");
    assert_eq!(github.args, vec!["~/.mcp/github/index.js"]);
}

#[test]
fn test_get_effective_tools_config_uses_unified_when_present() {
    let toml_str = r#"
[tools]
fs_enabled = false
git_enabled = false

[unified_tools]
enabled = true

[unified_tools.native.fs]
enabled = true
allowed_roots = ["~"]
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");
    let effective = config.get_effective_tools_config();

    // Should use unified_tools (fs enabled), not legacy tools (fs disabled)
    assert!(effective.enabled);
    assert!(effective.is_fs_enabled());
}

#[test]
fn test_get_effective_tools_config_falls_back_to_legacy() {
    let toml_str = r#"
[tools]
fs_enabled = true
git_enabled = false
shell_enabled = true
system_info_enabled = false

[mcp]
enabled = true

[[mcp.external_servers]]
name = "github"
command = "node"
args = ["~/.mcp/github/index.js"]
"#;

    let config: Config = toml::from_str(toml_str).expect("Should parse");

    // No unified_tools section, should fall back to legacy
    assert!(config.unified_tools.is_none());

    let effective = config.get_effective_tools_config();

    // Should convert legacy to unified format
    assert!(effective.enabled);
    assert!(effective.is_fs_enabled());
    assert!(!effective.is_git_enabled());
    assert!(effective.is_shell_enabled());
    assert!(!effective.is_system_info_enabled());

    // MCP servers should be copied
    assert_eq!(effective.mcp.len(), 1);
    assert!(effective.mcp.contains_key("github"));
}

#[test]
fn test_unified_tools_config_serialization_round_trip() {
    let mut config = UnifiedToolsConfig {
        enabled: true,
        ..UnifiedToolsConfig::default()
    };
    config.native.fs = Some(FsToolConfig {
        enabled: true,
        allowed_roots: vec!["~".to_string(), "/home".to_string()],
    });
    config.native.shell = Some(ShellToolConfig {
        enabled: true,
        timeout_seconds: 45,
        allowed_commands: vec![],
    });
    config.mcp.insert(
        "test-server".to_string(),
        McpServerConfig {
            command: "/usr/local/bin/test".to_string(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            requires_runtime: None,
            timeout_seconds: 30,
            enabled: true,
            triggers: None,
        },
    );

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(&config).expect("Should serialize");

    // Deserialize back
    let deserialized: UnifiedToolsConfig = toml::from_str(&toml_str).expect("Should deserialize");

    // Verify round-trip
    assert_eq!(deserialized.enabled, config.enabled);
    assert!(deserialized.is_fs_enabled());
    assert!(deserialized.is_shell_enabled());
    assert_eq!(deserialized.mcp.len(), 1);

    let fs = deserialized.native.fs.expect("Should have fs");
    assert_eq!(fs.allowed_roots, vec!["~", "/home"]);

    let shell = deserialized.native.shell.expect("Should have shell");
    assert_eq!(shell.timeout_seconds, 45);
}
