//! Integration tests for Extension SDK V2
//!
//! These tests verify the V2 manifest format (aleph.plugin.toml) and all its features:
//! - TOML priority over JSON
//! - [[tools]] section parsing
//! - [[hooks]] section with kind and priority
//! - [prompt] section with file and scope
//! - [permissions] section with all permission types
//! - [capabilities] section for dynamic tools/hooks
//! - [[services]] and [[commands]] sections

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

use alephcore::extension::manifest::{
    parse_aleph_plugin_toml_content, parse_manifest_from_dir_sync,
    FilesystemPermission,
};
use alephcore::extension::{PluginKind, PluginPermission};

// =============================================================================
// Test 1: TOML Priority over JSON
// =============================================================================

#[test]
fn test_v2_manifest_priority_over_json() {
    let dir = TempDir::new().unwrap();

    // Write both TOML and JSON manifests
    fs::write(
        dir.path().join("aleph.plugin.toml"),
        r#"
[plugin]
id = "toml-plugin"
name = "TOML Plugin"
version = "2.0.0"
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("aleph.plugin.json"),
        r#"{"id": "json-plugin", "name": "JSON Plugin", "version": "1.0.0"}"#,
    )
    .unwrap();

    // TOML should win
    let manifest = parse_manifest_from_dir_sync(dir.path()).unwrap();
    assert_eq!(manifest.id, "toml-plugin");
    assert_eq!(manifest.name, "TOML Plugin");
    assert_eq!(manifest.version, Some("2.0.0".to_string()));
}

#[test]
fn test_v2_manifest_priority_over_all_formats() {
    let dir = TempDir::new().unwrap();

    // Write all manifest formats
    fs::write(
        dir.path().join("aleph.plugin.toml"),
        r#"
[plugin]
id = "toml-version"
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("aleph.plugin.json"),
        r#"{"id": "json-version"}"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("package.json"),
        r#"{
            "name": "npm-version",
            "aleph": {"id": "npm-version"}
        }"#,
    )
    .unwrap();

    // Create legacy .claude-plugin format
    let claude_dir = dir.path().join(".claude-plugin");
    fs::create_dir(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("plugin.json"),
        r#"{"name": "Claude Version"}"#,
    )
    .unwrap();

    // TOML should win over all
    let manifest = parse_manifest_from_dir_sync(dir.path()).unwrap();
    assert_eq!(manifest.id, "toml-version");
}

// =============================================================================
// Test 2: [[tools]] Section Parsing
// =============================================================================

#[test]
fn test_v2_tools_parsing() {
    let content = r#"
[plugin]
id = "tools-test"

[[tools]]
name = "hello-tool"
description = "Says hello to someone"
handler = "handle_hello"
instruction_file = "tools/hello.md"

[[tools]]
name = "calculate"
description = "Performs calculations"
handler = "handle_calculate"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    assert_eq!(manifest.id, "tools-test");

    let tools = manifest.tools_v2.expect("tools_v2 should be Some");
    assert_eq!(tools.len(), 2);

    // Check first tool
    assert_eq!(tools[0].name, "hello-tool");
    assert_eq!(
        tools[0].description,
        Some("Says hello to someone".to_string())
    );
    assert_eq!(tools[0].handler, Some("handle_hello".to_string()));
    assert_eq!(
        tools[0].instruction_file,
        Some("tools/hello.md".to_string())
    );

    // Check second tool
    assert_eq!(tools[1].name, "calculate");
    assert_eq!(
        tools[1].description,
        Some("Performs calculations".to_string())
    );
    assert_eq!(tools[1].handler, Some("handle_calculate".to_string()));
    assert!(tools[1].instruction_file.is_none());
}

#[test]
fn test_v2_tools_with_parameters() {
    let content = r#"
[plugin]
id = "tools-params-test"

[[tools]]
name = "greet"
description = "Greets a person"
handler = "handle_greet"

[tools.parameters]
type = "object"
required = ["name"]

[tools.parameters.properties.name]
type = "string"
description = "The name of the person to greet"

[tools.parameters.properties.formal]
type = "boolean"
description = "Whether to use formal greeting"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let tools = manifest.tools_v2.expect("tools_v2 should be Some");
    assert_eq!(tools.len(), 1);

    let params = tools[0].parameters.as_ref().expect("parameters should exist");
    assert_eq!(params["type"], "object");
    assert!(params["required"].as_array().unwrap().contains(&serde_json::json!("name")));
}

// =============================================================================
// Test 3: [[hooks]] Section with Kind and Priority
// =============================================================================

#[test]
fn test_v2_hooks_parsing() {
    let content = r#"
[plugin]
id = "hooks-test"

[[hooks]]
event = "PreToolUse"
kind = "interceptor"
priority = "high"
handler = "on_pre_tool"
filter = "Bash"

[[hooks]]
event = "PostToolUse"
kind = "observer"
priority = "low"
handler = "on_post_tool"

[[hooks]]
event = "SessionStart"
handler = "on_session_start"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let hooks = manifest.hooks_v2.expect("hooks_v2 should be Some");
    assert_eq!(hooks.len(), 3);

    // Check first hook - interceptor with high priority
    assert_eq!(hooks[0].event, "PreToolUse");
    assert_eq!(hooks[0].kind, "interceptor");
    assert_eq!(hooks[0].priority, "high");
    assert_eq!(hooks[0].handler, Some("on_pre_tool".to_string()));
    assert_eq!(hooks[0].filter, Some("Bash".to_string()));

    // Check second hook - observer with low priority
    assert_eq!(hooks[1].event, "PostToolUse");
    assert_eq!(hooks[1].kind, "observer");
    assert_eq!(hooks[1].priority, "low");
    assert_eq!(hooks[1].handler, Some("on_post_tool".to_string()));
    assert!(hooks[1].filter.is_none());

    // Check third hook - defaults applied
    assert_eq!(hooks[2].event, "SessionStart");
    assert_eq!(hooks[2].kind, "observer"); // default
    assert_eq!(hooks[2].priority, "normal"); // default
}

#[test]
fn test_v2_hook_kind_values() {
    let content = r#"
[plugin]
id = "hook-kinds-test"

[[hooks]]
event = "PreToolUse"
kind = "observer"

[[hooks]]
event = "PostToolUse"
kind = "interceptor"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let hooks = manifest.hooks_v2.expect("hooks_v2 should be Some");
    assert_eq!(hooks[0].kind, "observer");
    assert_eq!(hooks[1].kind, "interceptor");
}

#[test]
fn test_v2_hook_priority_values() {
    let content = r#"
[plugin]
id = "hook-priorities-test"

[[hooks]]
event = "Event1"
priority = "low"

[[hooks]]
event = "Event2"
priority = "normal"

[[hooks]]
event = "Event3"
priority = "high"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let hooks = manifest.hooks_v2.expect("hooks_v2 should be Some");
    assert_eq!(hooks[0].priority, "low");
    assert_eq!(hooks[1].priority, "normal");
    assert_eq!(hooks[2].priority, "high");
}

// =============================================================================
// Test 4: [prompt] Section with File and Scope
// =============================================================================

#[test]
fn test_v2_prompt_parsing() {
    let content = r#"
[plugin]
id = "prompt-test"

[prompt]
file = "SYSTEM.md"
scope = "system"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let prompt = manifest.prompt_v2.expect("prompt_v2 should be Some");
    assert_eq!(prompt.file, "SYSTEM.md");
    assert_eq!(prompt.scope, "system");
}

#[test]
fn test_v2_prompt_scope_user() {
    let content = r#"
[plugin]
id = "prompt-user-test"

[prompt]
file = "prompts/user-context.md"
scope = "user"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let prompt = manifest.prompt_v2.expect("prompt_v2 should be Some");
    assert_eq!(prompt.file, "prompts/user-context.md");
    assert_eq!(prompt.scope, "user");
}

#[test]
fn test_v2_prompt_default_scope() {
    let content = r#"
[plugin]
id = "prompt-default-test"

[prompt]
file = "PROMPT.md"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let prompt = manifest.prompt_v2.expect("prompt_v2 should be Some");
    assert_eq!(prompt.file, "PROMPT.md");
    assert_eq!(prompt.scope, "system"); // default value
}

// =============================================================================
// Test 5: [permissions] Section
// =============================================================================

#[test]
fn test_v2_permissions_all_types() {
    let content = r#"
[plugin]
id = "permissions-test"

[permissions]
network = true
filesystem = "read"
env = true
shell = true
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    // Check that permissions are converted correctly
    assert!(manifest.permissions.contains(&PluginPermission::Network));
    assert!(manifest.permissions.contains(&PluginPermission::FilesystemRead));
    assert!(manifest.permissions.contains(&PluginPermission::Env));
    assert!(manifest
        .permissions
        .contains(&PluginPermission::Custom("shell".to_string())));
}

#[test]
fn test_v2_permissions_filesystem_levels() {
    // Test "read" level
    let content_read = r#"
[plugin]
id = "fs-read-test"

[permissions]
filesystem = "read"
"#;
    let manifest = parse_aleph_plugin_toml_content(content_read, std::path::Path::new("/test")).unwrap();
    assert!(manifest.permissions.contains(&PluginPermission::FilesystemRead));
    assert!(!manifest.permissions.contains(&PluginPermission::FilesystemWrite));
    assert!(!manifest.permissions.contains(&PluginPermission::Filesystem));

    // Test "write" level
    let content_write = r#"
[plugin]
id = "fs-write-test"

[permissions]
filesystem = "write"
"#;
    let manifest = parse_aleph_plugin_toml_content(content_write, std::path::Path::new("/test")).unwrap();
    assert!(manifest.permissions.contains(&PluginPermission::FilesystemWrite));
    assert!(!manifest.permissions.contains(&PluginPermission::FilesystemRead));

    // Test "full" level
    let content_full = r#"
[plugin]
id = "fs-full-test"

[permissions]
filesystem = "full"
"#;
    let manifest = parse_aleph_plugin_toml_content(content_full, std::path::Path::new("/test")).unwrap();
    assert!(manifest.permissions.contains(&PluginPermission::Filesystem));

    // Test boolean true
    let content_bool = r#"
[plugin]
id = "fs-bool-test"

[permissions]
filesystem = true
"#;
    let manifest = parse_aleph_plugin_toml_content(content_bool, std::path::Path::new("/test")).unwrap();
    assert!(manifest.permissions.contains(&PluginPermission::Filesystem));
}

#[test]
fn test_v2_permissions_empty() {
    let content = r#"
[plugin]
id = "no-permissions-test"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();
    assert!(manifest.permissions.is_empty());
}

#[test]
fn test_filesystem_permission_can_read_write() {
    // Test can_read()
    assert!(FilesystemPermission::Bool(true).can_read());
    assert!(!FilesystemPermission::Bool(false).can_read());
    assert!(FilesystemPermission::Level("read".to_string()).can_read());
    assert!(FilesystemPermission::Level("write".to_string()).can_read());
    assert!(FilesystemPermission::Level("full".to_string()).can_read());
    assert!(!FilesystemPermission::Level("none".to_string()).can_read());

    // Test can_write()
    assert!(FilesystemPermission::Bool(true).can_write());
    assert!(!FilesystemPermission::Bool(false).can_write());
    assert!(!FilesystemPermission::Level("read".to_string()).can_write());
    assert!(FilesystemPermission::Level("write".to_string()).can_write());
    assert!(FilesystemPermission::Level("full".to_string()).can_write());
}

// =============================================================================
// Test 6: [capabilities] Section
// =============================================================================

#[test]
fn test_v2_capabilities_parsing() {
    let content = r#"
[plugin]
id = "capabilities-test"

[capabilities]
dynamic_tools = true
dynamic_hooks = true
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let caps = manifest.capabilities_v2.expect("capabilities_v2 should be Some");
    assert!(caps.dynamic_tools);
    assert!(caps.dynamic_hooks);
}

#[test]
fn test_v2_capabilities_defaults() {
    let content = r#"
[plugin]
id = "capabilities-default-test"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let caps = manifest.capabilities_v2.expect("capabilities_v2 should be Some");
    assert!(!caps.dynamic_tools); // default false
    assert!(!caps.dynamic_hooks); // default false
}

#[test]
fn test_v2_capabilities_partial() {
    let content = r#"
[plugin]
id = "capabilities-partial-test"

[capabilities]
dynamic_tools = true
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let caps = manifest.capabilities_v2.expect("capabilities_v2 should be Some");
    assert!(caps.dynamic_tools);
    assert!(!caps.dynamic_hooks); // default false
}

// =============================================================================
// Test 7: [[services]] Section
// =============================================================================

#[test]
fn test_v2_services_parsing() {
    let content = r#"
[plugin]
id = "services-test"

[[services]]
name = "background-worker"
description = "Runs background tasks"
start_handler = "start_worker"
stop_handler = "stop_worker"

[[services]]
name = "file-watcher"
description = "Watches for file changes"
start_handler = "start_watcher"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let services = manifest.services_v2.expect("services_v2 should be Some");
    assert_eq!(services.len(), 2);

    // Check first service
    assert_eq!(services[0].name, "background-worker");
    assert_eq!(
        services[0].description,
        Some("Runs background tasks".to_string())
    );
    assert_eq!(services[0].start_handler, Some("start_worker".to_string()));
    assert_eq!(services[0].stop_handler, Some("stop_worker".to_string()));

    // Check second service (no stop_handler)
    assert_eq!(services[1].name, "file-watcher");
    assert_eq!(services[1].start_handler, Some("start_watcher".to_string()));
    assert!(services[1].stop_handler.is_none());
}

// =============================================================================
// Test 8: [[commands]] Section
// =============================================================================

#[test]
fn test_v2_commands_parsing() {
    let content = r#"
[plugin]
id = "commands-test"

[[commands]]
name = "greet"
description = "Greets someone"
handler = "handle_greet"
prompt_file = "commands/greet.md"

[[commands]]
name = "help"
description = "Shows help"
handler = "handle_help"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    let commands = manifest.commands_v2.expect("commands_v2 should be Some");
    assert_eq!(commands.len(), 2);

    // Check first command
    assert_eq!(commands[0].name, "greet");
    assert_eq!(commands[0].description, Some("Greets someone".to_string()));
    assert_eq!(commands[0].handler, Some("handle_greet".to_string()));
    assert_eq!(
        commands[0].prompt_file,
        Some("commands/greet.md".to_string())
    );

    // Check second command (no prompt_file)
    assert_eq!(commands[1].name, "help");
    assert!(commands[1].prompt_file.is_none());
}

// =============================================================================
// Test 9: Complete V2 Manifest
// =============================================================================

#[test]
fn test_v2_complete_manifest() {
    let content = r#"
[plugin]
id = "complete-v2-plugin"
name = "Complete V2 Plugin"
version = "2.0.0"
description = "A fully-featured V2 plugin"
kind = "nodejs"
entry = "dist/index.js"
homepage = "https://example.com"
repository = "https://github.com/user/repo"
license = "MIT"
keywords = ["test", "example", "v2"]

[plugin.author]
name = "Test Author"
email = "test@example.com"
url = "https://author.example.com"

[permissions]
network = true
filesystem = "read"
env = true
shell = false

[prompt]
file = "SYSTEM.md"
scope = "system"

[[tools]]
name = "main-tool"
description = "The main tool"
handler = "handle_main"

[[hooks]]
event = "PreToolUse"
kind = "interceptor"
priority = "high"
handler = "on_pre_tool"

[[commands]]
name = "init"
description = "Initializes the plugin"
handler = "handle_init"

[[services]]
name = "daemon"
description = "Background daemon"
start_handler = "start_daemon"

[capabilities]
dynamic_tools = true
dynamic_hooks = false
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    // Plugin section
    assert_eq!(manifest.id, "complete-v2-plugin");
    assert_eq!(manifest.name, "Complete V2 Plugin");
    assert_eq!(manifest.version, Some("2.0.0".to_string()));
    assert_eq!(manifest.description, Some("A fully-featured V2 plugin".to_string()));
    assert_eq!(manifest.kind, PluginKind::NodeJs);
    assert_eq!(manifest.entry, PathBuf::from("dist/index.js"));
    assert_eq!(manifest.homepage, Some("https://example.com".to_string()));
    assert_eq!(manifest.repository, Some("https://github.com/user/repo".to_string()));
    assert_eq!(manifest.license, Some("MIT".to_string()));
    assert_eq!(manifest.keywords, vec!["test", "example", "v2"]);

    // Author
    let author = manifest.author.as_ref().expect("author should exist");
    assert_eq!(author.name, Some("Test Author".to_string()));
    assert_eq!(author.email, Some("test@example.com".to_string()));

    // Permissions
    assert!(manifest.permissions.contains(&PluginPermission::Network));
    assert!(manifest.permissions.contains(&PluginPermission::FilesystemRead));
    assert!(manifest.permissions.contains(&PluginPermission::Env));
    assert!(!manifest.permissions.iter().any(|p| matches!(p, PluginPermission::Custom(s) if s == "shell")));

    // Prompt
    let prompt = manifest.prompt_v2.as_ref().expect("prompt should exist");
    assert_eq!(prompt.file, "SYSTEM.md");
    assert_eq!(prompt.scope, "system");

    // Tools
    let tools = manifest.tools_v2.as_ref().expect("tools should exist");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "main-tool");

    // Hooks
    let hooks = manifest.hooks_v2.as_ref().expect("hooks should exist");
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].event, "PreToolUse");
    assert_eq!(hooks[0].kind, "interceptor");
    assert_eq!(hooks[0].priority, "high");

    // Commands
    let commands = manifest.commands_v2.as_ref().expect("commands should exist");
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].name, "init");

    // Services
    let services = manifest.services_v2.as_ref().expect("services should exist");
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].name, "daemon");

    // Capabilities
    let caps = manifest.capabilities_v2.as_ref().expect("capabilities should exist");
    assert!(caps.dynamic_tools);
    assert!(!caps.dynamic_hooks);
}

// =============================================================================
// Test 10: Plugin Kind Defaults
// =============================================================================

#[test]
fn test_v2_plugin_kind_defaults() {
    // Default to WASM
    let content_minimal = r#"
[plugin]
id = "minimal"
"#;
    let manifest = parse_aleph_plugin_toml_content(content_minimal, std::path::Path::new("/test")).unwrap();
    assert_eq!(manifest.kind, PluginKind::Wasm);
    assert_eq!(manifest.entry, PathBuf::from("plugin.wasm"));

    // NodeJS kind
    let content_nodejs = r#"
[plugin]
id = "nodejs-plugin"
kind = "nodejs"
"#;
    let manifest = parse_aleph_plugin_toml_content(content_nodejs, std::path::Path::new("/test")).unwrap();
    assert_eq!(manifest.kind, PluginKind::NodeJs);
    assert_eq!(manifest.entry, PathBuf::from("index.js"));

    // Static kind
    let content_static = r#"
[plugin]
id = "static-plugin"
kind = "static"
"#;
    let manifest = parse_aleph_plugin_toml_content(content_static, std::path::Path::new("/test")).unwrap();
    assert_eq!(manifest.kind, PluginKind::Static);
    assert_eq!(manifest.entry, PathBuf::from("."));
}

// =============================================================================
// Test 11: Error Cases
// =============================================================================

#[test]
fn test_v2_missing_plugin_id() {
    let content = r#"
[plugin]
name = "No ID Plugin"
"#;

    let result = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test"));
    assert!(result.is_err());
}

#[test]
fn test_v2_empty_plugin_id() {
    let content = r#"
[plugin]
id = ""
name = "Empty ID Plugin"
"#;

    let result = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test"));
    assert!(result.is_err());
}

#[test]
fn test_v2_invalid_toml_syntax() {
    let content = r#"
[plugin
id = "broken"
"#;

    let result = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test"));
    assert!(result.is_err());
}

#[test]
fn test_v2_id_sanitization() {
    let content = r#"
[plugin]
id = "Invalid ID With Spaces"
"#;

    // ID should be sanitized
    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();
    assert_eq!(manifest.id, "invalid-id-with-spaces");
}

// =============================================================================
// Test 12: Empty Sections
// =============================================================================

#[test]
fn test_v2_no_tools() {
    let content = r#"
[plugin]
id = "no-tools"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();
    assert!(manifest.tools_v2.is_none());
}

#[test]
fn test_v2_no_hooks() {
    let content = r#"
[plugin]
id = "no-hooks"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();
    assert!(manifest.hooks_v2.is_none());
}

#[test]
fn test_v2_no_services() {
    let content = r#"
[plugin]
id = "no-services"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();
    assert!(manifest.services_v2.is_none());
}

#[test]
fn test_v2_no_commands() {
    let content = r#"
[plugin]
id = "no-commands"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();
    assert!(manifest.commands_v2.is_none());
}

#[test]
fn test_v2_no_prompt() {
    let content = r#"
[plugin]
id = "no-prompt"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();
    assert!(manifest.prompt_v2.is_none());
}

// =============================================================================
// Test 12.6: P2 Channels Parsing
// =============================================================================

#[test]
fn test_v2_channels_parsing() {
    let content = r#"
[plugin]
id = "test-channels"
kind = "nodejs"

[[channels]]
id = "slack"
label = "Slack"
handler = "handleSlackChannel"

[channels.config_schema]
token = { type = "string" }

[[channels]]
id = "telegram"
label = "Telegram"
handler = "handleTelegramChannel"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let channels = manifest.channels_v2.expect("channels_v2 should be Some");
    assert_eq!(channels.len(), 2);

    // Check first channel
    assert_eq!(channels[0].id, "slack");
    assert_eq!(channels[0].label, "Slack");
    assert_eq!(channels[0].handler, Some("handleSlackChannel".to_string()));
    assert!(channels[0].config_schema.is_some());

    // Check second channel
    assert_eq!(channels[1].id, "telegram");
    assert_eq!(channels[1].label, "Telegram");
    assert_eq!(channels[1].handler, Some("handleTelegramChannel".to_string()));
    assert!(channels[1].config_schema.is_none());
}

#[test]
fn test_v2_channels_empty() {
    let content = r#"
[plugin]
id = "no-channels"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();
    assert!(manifest.channels_v2.is_none());
}

// =============================================================================
// Test 12.7: P2 Providers Parsing
// =============================================================================

#[test]
fn test_v2_providers_parsing() {
    let content = r#"
[plugin]
id = "test-providers"
kind = "nodejs"

[[providers]]
id = "custom-llm"
name = "Custom LLM"
models = ["model-fast", "model-quality"]
handler = "handleChat"

[providers.config_schema]
api_key = { type = "string" }

[[providers]]
id = "local-llm"
name = "Local LLM"
models = ["llama-7b", "llama-13b"]
handler = "handleLocalChat"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let providers = manifest.providers_v2.expect("providers_v2 should be Some");
    assert_eq!(providers.len(), 2);

    // Check first provider
    assert_eq!(providers[0].id, "custom-llm");
    assert_eq!(providers[0].name, "Custom LLM");
    assert_eq!(providers[0].models, vec!["model-fast", "model-quality"]);
    assert_eq!(providers[0].handler, Some("handleChat".to_string()));
    assert!(providers[0].config_schema.is_some());

    // Check second provider
    assert_eq!(providers[1].id, "local-llm");
    assert_eq!(providers[1].name, "Local LLM");
    assert_eq!(providers[1].models.len(), 2);
    assert_eq!(providers[1].handler, Some("handleLocalChat".to_string()));
    assert!(providers[1].config_schema.is_none());
}

#[test]
fn test_v2_providers_empty() {
    let content = r#"
[plugin]
id = "no-providers"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();
    assert!(manifest.providers_v2.is_none());
}

#[test]
fn test_v2_providers_no_models() {
    let content = r#"
[plugin]
id = "provider-no-models"

[[providers]]
id = "empty-models"
name = "Empty Models Provider"
handler = "handleChat"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let providers = manifest.providers_v2.expect("providers_v2 should be Some");
    assert_eq!(providers[0].models.len(), 0);
}

// =============================================================================
// Test 12.8: P2 HTTP Routes Parsing
// =============================================================================

#[test]
fn test_v2_http_routes_parsing() {
    let content = r#"
[plugin]
id = "test-http"
kind = "nodejs"

[[http_routes]]
path = "/api/data"
methods = ["GET", "POST"]
handler = "handleData"

[[http_routes]]
path = "/api/items/{id}"
methods = ["GET", "PUT", "DELETE"]
handler = "handleItem"

[[http_routes]]
path = "/api/health"
methods = ["GET"]
handler = "handleHealth"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let routes = manifest.http_routes_v2.expect("http_routes_v2 should be Some");
    assert_eq!(routes.len(), 3);

    // Check first route
    assert_eq!(routes[0].path, "/api/data");
    assert_eq!(routes[0].methods, vec!["GET", "POST"]);
    assert_eq!(routes[0].handler, "handleData");

    // Check second route (with path parameter)
    assert_eq!(routes[1].path, "/api/items/{id}");
    assert_eq!(routes[1].methods, vec!["GET", "PUT", "DELETE"]);
    assert_eq!(routes[1].handler, "handleItem");

    // Check third route
    assert_eq!(routes[2].path, "/api/health");
    assert_eq!(routes[2].methods, vec!["GET"]);
    assert_eq!(routes[2].handler, "handleHealth");
}

#[test]
fn test_v2_http_routes_empty() {
    let content = r#"
[plugin]
id = "no-http-routes"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();
    assert!(manifest.http_routes_v2.is_none());
}

#[test]
fn test_v2_http_routes_no_methods() {
    let content = r#"
[plugin]
id = "route-no-methods"

[[http_routes]]
path = "/api/test"
handler = "handleTest"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let routes = manifest.http_routes_v2.expect("http_routes_v2 should be Some");
    assert_eq!(routes[0].methods.len(), 0);
}

// =============================================================================
// Test 12.9: HTTP Path Matching
// =============================================================================

#[test]
fn test_http_path_matching() {
    use alephcore::extension::match_path;

    // Exact match
    let params = match_path("/api/users", "/api/users");
    assert!(params.is_some());
    let params = params.unwrap();
    assert!(params.is_empty());

    // Parameter match
    let params = match_path("/api/users/{id}", "/api/users/123");
    assert!(params.is_some());
    let params = params.unwrap();
    assert_eq!(params.get("id"), Some(&"123".to_string()));

    // Multiple parameters
    let params = match_path("/api/{org}/repos/{repo}", "/api/acme/repos/widgets");
    assert!(params.is_some());
    let params = params.unwrap();
    assert_eq!(params.get("org"), Some(&"acme".to_string()));
    assert_eq!(params.get("repo"), Some(&"widgets".to_string()));

    // No match - different path
    assert!(match_path("/api/users", "/api/posts").is_none());

    // No match - different segment count
    assert!(match_path("/api/users/{id}", "/api/users/123/posts").is_none());

    // No match - different literal segment
    assert!(match_path("/api/users/{id}", "/api/posts/123").is_none());
}

#[test]
fn test_http_path_matching_edge_cases() {
    use alephcore::extension::match_path;

    // Root path
    let params = match_path("/", "/");
    assert!(params.is_some());
    assert!(params.unwrap().is_empty());

    // Trailing slash handling
    let params = match_path("/api/users", "/api/users/");
    assert!(params.is_some());

    let params = match_path("/api/users/", "/api/users");
    assert!(params.is_some());

    // Single segment parameter
    let params = match_path("/{id}", "/123");
    assert!(params.is_some());
    assert_eq!(params.unwrap().get("id"), Some(&"123".to_string()));

    // Complex path with multiple parameters
    let params = match_path(
        "/v1/{version}/users/{user_id}/posts/{post_id}",
        "/v1/2024/users/alice/posts/42",
    );
    assert!(params.is_some());
    let params = params.unwrap();
    assert_eq!(params.get("version"), Some(&"2024".to_string()));
    assert_eq!(params.get("user_id"), Some(&"alice".to_string()));
    assert_eq!(params.get("post_id"), Some(&"42".to_string()));
}

// =============================================================================
// Test 12.10: P2 Complete Manifest with All Features
// =============================================================================

#[test]
fn test_v2_complete_manifest_with_p2_features() {
    let content = r#"
[plugin]
id = "full-p2-plugin"
name = "Full P2 Plugin"
version = "2.0.0"
kind = "nodejs"
entry = "dist/index.js"

[permissions]
network = true
filesystem = "read"

[[tools]]
name = "my-tool"
description = "A custom tool"
handler = "handleTool"

[[hooks]]
event = "PreToolUse"
kind = "interceptor"
handler = "onPreTool"

[[services]]
name = "background-worker"
start_handler = "startWorker"
stop_handler = "stopWorker"

[[commands]]
name = "status"
handler = "handleStatus"

[[channels]]
id = "custom-channel"
label = "Custom Channel"
handler = "handleChannel"

[[providers]]
id = "custom-provider"
name = "Custom Provider"
models = ["model-a", "model-b"]
handler = "handleProvider"

[[http_routes]]
path = "/api/webhook"
methods = ["POST"]
handler = "handleWebhook"

[capabilities]
dynamic_tools = true
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    // Verify plugin basics
    assert_eq!(manifest.id, "full-p2-plugin");
    assert_eq!(manifest.name, "Full P2 Plugin");

    // Verify P0/P1 features
    assert!(manifest.tools_v2.is_some());
    assert!(manifest.hooks_v2.is_some());
    assert!(manifest.services_v2.is_some());
    assert!(manifest.commands_v2.is_some());

    // Verify P2 features
    let channels = manifest.channels_v2.expect("channels should be present");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, "custom-channel");

    let providers = manifest.providers_v2.expect("providers should be present");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "custom-provider");
    assert_eq!(providers[0].models.len(), 2);

    let routes = manifest.http_routes_v2.expect("http_routes should be present");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "/api/webhook");

    // Verify capabilities
    let caps = manifest.capabilities_v2.expect("capabilities should be present");
    assert!(caps.dynamic_tools);
}

// =============================================================================
// Test 12.5: Services Full Lifecycle (P1.5)
// =============================================================================

#[test]
fn test_v2_services_full() {
    let content = r#"
[plugin]
id = "test-services"
kind = "nodejs"
entry = "dist/index.js"

[[services]]
name = "file-watcher"
description = "Watches files for changes"
start_handler = "startWatcher"
stop_handler = "stopWatcher"

[[services]]
name = "sync-daemon"
start_handler = "startSync"
stop_handler = "stopSync"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let services = manifest.services_v2.unwrap();
    assert_eq!(services.len(), 2);
    assert_eq!(services[0].name, "file-watcher");
    assert_eq!(services[0].start_handler, Some("startWatcher".to_string()));
    assert_eq!(services[0].stop_handler, Some("stopWatcher".to_string()));
    assert_eq!(services[0].description, Some("Watches files for changes".to_string()));

    assert_eq!(services[1].name, "sync-daemon");
    assert_eq!(services[1].start_handler, Some("startSync".to_string()));
    assert_eq!(services[1].stop_handler, Some("stopSync".to_string()));
    assert!(services[1].description.is_none());
}

#[test]
fn test_service_state_serialization() {
    use alephcore::extension::ServiceState;

    // Test serialization
    let running = ServiceState::Running;
    let json = serde_json::to_string(&running).unwrap();
    assert_eq!(json, "\"running\"");

    let stopped_json = serde_json::to_string(&ServiceState::Stopped).unwrap();
    assert_eq!(stopped_json, "\"stopped\"");

    let starting_json = serde_json::to_string(&ServiceState::Starting).unwrap();
    assert_eq!(starting_json, "\"starting\"");

    let stopping_json = serde_json::to_string(&ServiceState::Stopping).unwrap();
    assert_eq!(stopping_json, "\"stopping\"");

    let failed_json = serde_json::to_string(&ServiceState::Failed).unwrap();
    assert_eq!(failed_json, "\"failed\"");

    // Test deserialization
    let stopped: ServiceState = serde_json::from_str("\"stopped\"").unwrap();
    assert_eq!(stopped, ServiceState::Stopped);

    let running_parsed: ServiceState = serde_json::from_str("\"running\"").unwrap();
    assert_eq!(running_parsed, ServiceState::Running);

    let failed_parsed: ServiceState = serde_json::from_str("\"failed\"").unwrap();
    assert_eq!(failed_parsed, ServiceState::Failed);
}

#[test]
fn test_service_info_serialization() {
    use alephcore::extension::{ServiceInfo, ServiceState};

    let info = ServiceInfo {
        id: "svc-test-123".to_string(),
        plugin_id: "test-services".to_string(),
        name: "file-watcher".to_string(),
        state: ServiceState::Running,
        started_at: None,
        error: None,
    };

    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("svc-test-123"));
    assert!(json.contains("test-services"));
    assert!(json.contains("file-watcher"));
    assert!(json.contains("running"));

    let parsed: ServiceInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "svc-test-123");
    assert_eq!(parsed.plugin_id, "test-services");
    assert_eq!(parsed.name, "file-watcher");
    assert_eq!(parsed.state, ServiceState::Running);
}

#[test]
fn test_service_result_serialization() {
    use alephcore::extension::ServiceResult;

    let ok_result = ServiceResult::ok();
    assert!(ok_result.success);
    assert!(ok_result.message.is_none());

    let ok_with_msg = ServiceResult::ok_with_message("Service started");
    assert!(ok_with_msg.success);
    assert_eq!(ok_with_msg.message, Some("Service started".to_string()));

    let error_result = ServiceResult::error("Connection refused");
    assert!(!error_result.success);
    assert_eq!(error_result.message, Some("Connection refused".to_string()));

    // Test serialization round-trip
    let json = serde_json::to_string(&ok_with_msg).unwrap();
    let parsed: ServiceResult = serde_json::from_str(&json).unwrap();
    assert!(parsed.success);
    assert_eq!(parsed.message, Some("Service started".to_string()));
}

// =============================================================================
// Test 13: Directory-based Parsing
// =============================================================================

#[test]
fn test_v2_parse_from_directory() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("aleph.plugin.toml"),
        r#"
[plugin]
id = "dir-plugin"
name = "Directory Plugin"
version = "1.2.3"

[[tools]]
name = "dir-tool"

[[hooks]]
event = "SessionStart"
"#,
    )
    .unwrap();

    let manifest = parse_manifest_from_dir_sync(dir.path()).unwrap();

    assert_eq!(manifest.id, "dir-plugin");
    assert_eq!(manifest.name, "Directory Plugin");
    assert_eq!(manifest.version, Some("1.2.3".to_string()));
    assert_eq!(manifest.root_dir, dir.path());

    assert!(manifest.tools_v2.is_some());
    assert!(manifest.hooks_v2.is_some());
}

// =============================================================================
// Test 14: Config Schema and UI Hints
// =============================================================================

// =============================================================================
// Test 14.5: Direct Commands with Handler
// =============================================================================

#[test]
fn test_v2_commands_with_handler() {
    let content = r#"
[plugin]
id = "test-commands"
kind = "nodejs"
entry = "dist/index.js"

[[commands]]
name = "status"
description = "Show status"
handler = "handleStatus"

[[commands]]
name = "clear"
description = "Clear screen"
handler = "handleClear"
"#;

    let manifest = parse_aleph_plugin_toml_content(content, &PathBuf::from("/tmp")).unwrap();

    let commands = manifest.commands_v2.unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].name, "status");
    assert_eq!(commands[0].handler, Some("handleStatus".to_string()));
    assert_eq!(commands[1].name, "clear");
    assert_eq!(commands[1].handler, Some("handleClear".to_string()));
}

#[test]
fn test_direct_command_result() {
    use alephcore::extension::DirectCommandResult;

    let success = DirectCommandResult::success("Done!");
    assert!(success.success);
    assert_eq!(success.content, "Done!");
    assert!(success.data.is_none());

    let with_data = DirectCommandResult::with_data("Result", serde_json::json!({"count": 42}));
    assert!(with_data.success);
    assert!(with_data.data.is_some());
    assert_eq!(with_data.data.unwrap()["count"], 42);

    let error = DirectCommandResult::error("Failed");
    assert!(!error.success);
    assert_eq!(error.content, "Failed");
}

// =============================================================================
// Test 15: Config Schema and UI Hints
// =============================================================================

#[test]
fn test_v2_config_schema() {
    let content = r#"
[plugin]
id = "config-plugin"

[plugin.config_schema]
type = "object"

[plugin.config_schema.properties.api_key]
type = "string"
description = "The API key"

[plugin.config_schema.properties.timeout]
type = "number"
default = 30

[plugin.config_ui_hints.api_key]
label = "API Key"
help = "Your API key for authentication"
sensitive = true

[plugin.config_ui_hints.timeout]
label = "Timeout"
help = "Request timeout in seconds"
advanced = true
"#;

    let manifest = parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")).unwrap();

    assert!(manifest.config_schema.is_some());
    assert!(manifest.has_config());

    let schema = manifest.config_schema.as_ref().unwrap();
    assert_eq!(schema["type"], "object");

    // Check UI hints
    let api_key_hint = manifest.config_ui_hints.get("api_key").unwrap();
    assert_eq!(api_key_hint.label, Some("API Key".to_string()));
    assert_eq!(api_key_hint.sensitive, Some(true));

    let timeout_hint = manifest.config_ui_hints.get("timeout").unwrap();
    assert_eq!(timeout_hint.advanced, Some(true));
}
