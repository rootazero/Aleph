//! Tests for aleph.plugin.toml parsing

use super::*;
use crate::extension::manifest::types::PluginPermission;
use crate::extension::types::PluginKind;
use std::path::{Path, PathBuf};

#[test]
fn test_parse_minimal_toml() {
    let content = r#"
[plugin]
id = "my-plugin"
"#;

    let manifest =
        parse_aleph_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

    assert_eq!(manifest.id, "my-plugin");
    assert_eq!(manifest.name, "my-plugin"); // defaults to id
    assert_eq!(manifest.kind, PluginKind::Wasm); // default
    assert_eq!(manifest.entry, PathBuf::from("plugin.wasm")); // default for wasm
    assert!(manifest.permissions.is_empty()); // no permissions by default
    assert_eq!(manifest.root_dir, PathBuf::from("/test/plugin"));
}

#[test]
fn test_parse_full_toml() {
    let content = r#"
[plugin]
id = "complete-plugin"
name = "Complete Plugin"
version = "2.0.0"
description = "A fully specified plugin"
kind = "wasm"
entry = "dist/plugin.wasm"
homepage = "https://example.com"
repository = "https://github.com/user/repo"
license = "MIT"
keywords = ["test", "example"]

[plugin.author]
name = "Test Author"
email = "author@example.com"
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
name = "hello-tool"
description = "Says hello"
handler = "handle_hello"

[[tools]]
name = "world-tool"
description = "Says world"
handler = "handle_world"

[[hooks]]
event = "PreToolUse"
kind = "observer"
handler = "on_pre_tool"
priority = "high"
filter = "Bash"

[[commands]]
name = "greet"
description = "Greet someone"
handler = "handle_greet"
prompt_file = "commands/greet.md"

[[services]]
name = "background-worker"
description = "Background processing"
start_handler = "start_worker"
stop_handler = "stop_worker"

[capabilities]
dynamic_tools = true
dynamic_hooks = false
"#;

    let manifest =
        parse_aleph_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

    // Plugin section
    assert_eq!(manifest.id, "complete-plugin");
    assert_eq!(manifest.name, "Complete Plugin");
    assert_eq!(manifest.version, Some("2.0.0".to_string()));
    assert_eq!(
        manifest.description,
        Some("A fully specified plugin".to_string())
    );
    assert_eq!(manifest.kind, PluginKind::Wasm);
    assert_eq!(manifest.entry, PathBuf::from("dist/plugin.wasm"));
    assert_eq!(manifest.homepage, Some("https://example.com".to_string()));
    assert_eq!(
        manifest.repository,
        Some("https://github.com/user/repo".to_string())
    );
    assert_eq!(manifest.license, Some("MIT".to_string()));
    assert_eq!(manifest.keywords, vec!["test", "example"]);

    // Author
    let author = manifest.author.as_ref().unwrap();
    assert_eq!(author.name, Some("Test Author".to_string()));
    assert_eq!(author.email, Some("author@example.com".to_string()));
    assert_eq!(author.url, Some("https://author.example.com".to_string()));

    // Permissions
    assert!(manifest.permissions.contains(&PluginPermission::Network));
    assert!(manifest.permissions.contains(&PluginPermission::FilesystemRead));
    assert!(manifest.permissions.contains(&PluginPermission::Env));
    // shell = false, so no shell permission
    assert!(!manifest.permissions.iter().any(|p| matches!(p, PluginPermission::Custom(s) if s == "shell")));
}

#[test]
fn test_parse_toml_missing_id() {
    let content = r#"
[plugin]
name = "No ID Plugin"
"#;

    let result = parse_aleph_plugin_toml_content(content, Path::new("/test/plugin"));
    assert!(result.is_err());

    // When the id field is missing, toml parser fails with InvalidManifest
    // because `id` is a required field in PluginSection struct
    let err = result.unwrap_err();
    assert!(
        matches!(err, crate::extension::error::ExtensionError::InvalidManifest { .. }),
        "Expected InvalidManifest error, got: {:?}",
        err
    );
}

#[test]
fn test_parse_toml_empty_id() {
    let content = r#"
[plugin]
id = ""
name = "Empty ID Plugin"
"#;

    let result = parse_aleph_plugin_toml_content(content, Path::new("/test/plugin"));
    assert!(result.is_err());

    // When id is empty string, we check it explicitly and return MissingField
    let err = result.unwrap_err();
    assert!(
        matches!(err, crate::extension::error::ExtensionError::MissingField { .. }),
        "Expected MissingField error, got: {:?}",
        err
    );
}

#[test]
fn test_parse_toml_invalid_id() {
    let content = r#"
[plugin]
id = "Invalid ID With Spaces"
"#;

    // The ID should be sanitized, so this should work
    let result = parse_aleph_plugin_toml_content(content, Path::new("/test/plugin"));
    assert!(result.is_ok());
    let manifest = result.unwrap();
    assert_eq!(manifest.id, "invalid-id-with-spaces");
}

#[test]
fn test_parse_toml_nodejs_plugin() {
    let content = r#"
[plugin]
id = "nodejs-plugin"
kind = "nodejs"
"#;

    let manifest =
        parse_aleph_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

    assert_eq!(manifest.kind, PluginKind::NodeJs);
    assert_eq!(manifest.entry, PathBuf::from("index.js")); // default for nodejs
}

#[test]
fn test_parse_toml_static_plugin() {
    let content = r#"
[plugin]
id = "static-plugin"
kind = "static"
extensions = [".md", ".txt"]
"#;

    let manifest =
        parse_aleph_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

    assert_eq!(manifest.kind, PluginKind::Static);
    assert_eq!(manifest.entry, PathBuf::from(".")); // default for static
    assert_eq!(manifest.extensions, vec![".md", ".txt"]);
}

#[test]
fn test_convert_permissions_full_filesystem() {
    let perms = PermissionsSection {
        network: true,
        filesystem: FilesystemPermission::Bool(true),
        env: true,
        shell: true,
    };

    let result = convert_permissions(&perms);

    assert!(result.contains(&PluginPermission::Network));
    assert!(result.contains(&PluginPermission::Filesystem));
    assert!(result.contains(&PluginPermission::Env));
    assert!(result.contains(&PluginPermission::Custom("shell".to_string())));
}

#[test]
fn test_convert_permissions_read_only_filesystem() {
    let perms = PermissionsSection {
        network: false,
        filesystem: FilesystemPermission::Level("read".to_string()),
        env: false,
        shell: false,
    };

    let result = convert_permissions(&perms);

    assert!(!result.contains(&PluginPermission::Network));
    assert!(result.contains(&PluginPermission::FilesystemRead));
    assert!(!result.contains(&PluginPermission::Filesystem));
    assert!(!result.contains(&PluginPermission::Env));
}

#[test]
fn test_convert_permissions_write_filesystem() {
    let perms = PermissionsSection {
        network: false,
        filesystem: FilesystemPermission::Level("write".to_string()),
        env: false,
        shell: false,
    };

    let result = convert_permissions(&perms);
    assert!(result.contains(&PluginPermission::FilesystemWrite));
}

#[test]
fn test_filesystem_permission_can_read() {
    assert!(FilesystemPermission::Bool(true).can_read());
    assert!(!FilesystemPermission::Bool(false).can_read());
    assert!(FilesystemPermission::Level("read".to_string()).can_read());
    assert!(FilesystemPermission::Level("write".to_string()).can_read());
    assert!(FilesystemPermission::Level("full".to_string()).can_read());
    assert!(!FilesystemPermission::Level("none".to_string()).can_read());
}

#[test]
fn test_filesystem_permission_can_write() {
    assert!(FilesystemPermission::Bool(true).can_write());
    assert!(!FilesystemPermission::Bool(false).can_write());
    assert!(!FilesystemPermission::Level("read".to_string()).can_write());
    assert!(FilesystemPermission::Level("write".to_string()).can_write());
    assert!(FilesystemPermission::Level("full".to_string()).can_write());
}

#[test]
fn test_parse_toml_with_config_schema() {
    let content = r#"
[plugin]
id = "config-plugin"

[plugin.config_schema]
type = "object"
properties = { api_key = { type = "string" } }

[plugin.config_ui_hints.api_key]
label = "API Key"
help = "Your API key"
sensitive = true
"#;

    let manifest =
        parse_aleph_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

    assert!(manifest.config_schema.is_some());
    assert!(manifest.has_config());

    let hint = manifest.config_ui_hints.get("api_key").unwrap();
    assert_eq!(hint.label, Some("API Key".to_string()));
    assert_eq!(hint.help, Some("Your API key".to_string()));
    assert_eq!(hint.sensitive, Some(true));
}

#[test]
fn test_default_values() {
    // Test that defaults work correctly
    let perms = PermissionsSection::default();
    assert!(!perms.network);
    assert_eq!(perms.filesystem, FilesystemPermission::Bool(false));
    assert!(!perms.env);
    assert!(!perms.shell);

    let caps = CapabilitiesSection::default();
    assert!(!caps.dynamic_tools);
    assert!(!caps.dynamic_hooks);
}

#[test]
fn test_prompt_section_defaults() {
    let content = r#"
[plugin]
id = "prompt-plugin"

[prompt]
file = "SYSTEM.md"
"#;

    let toml: AlephPluginToml = toml::from_str(content).unwrap();
    let prompt = toml.prompt.unwrap();

    assert_eq!(prompt.file, "SYSTEM.md");
    assert_eq!(prompt.scope, "system"); // default value
}

#[test]
fn test_hook_section_defaults() {
    let content = r#"
[plugin]
id = "hook-plugin"

[[hooks]]
event = "SessionStart"
handler = "on_session_start"
"#;

    let toml: AlephPluginToml = toml::from_str(content).unwrap();
    let hook = &toml.hooks[0];

    assert_eq!(hook.event, "SessionStart");
    assert_eq!(hook.kind, "observer"); // default
    assert_eq!(hook.priority, "normal"); // default
    assert_eq!(hook.handler, Some("on_session_start".to_string()));
    assert!(hook.filter.is_none());
}

#[test]
fn test_parse_wasm_capabilities() {
    let content = r#"
[plugin]
id = "test-wasm"
name = "Test WASM"
kind = "wasm"
entry = "plugin.wasm"

[capabilities.workspace]
allowed_prefixes = ["docs/", "config/"]

[capabilities.http]
timeout_secs = 30

[[capabilities.http.allowlist]]
host = "api.slack.com"
path_prefix = "/api/"
methods = ["GET", "POST"]

[[capabilities.http.credentials]]
secret_name = "slack_token"
host_patterns = ["api.slack.com"]

[capabilities.http.credentials.inject]
type = "bearer"

[capabilities.tool_invoke]
max_per_execution = 10

[capabilities.tool_invoke.aliases]
search = "brave_search"

[capabilities.secrets]
allowed_patterns = ["slack_*"]
"#;

    let manifest = parse_aleph_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
    let caps = manifest.wasm_capabilities.as_ref().unwrap();
    assert!(caps.workspace.is_some());
    assert_eq!(caps.workspace.as_ref().unwrap().allowed_prefixes.len(), 2);
    assert!(caps.http.is_some());
    assert_eq!(caps.http.as_ref().unwrap().allowlist.len(), 1);
    assert_eq!(caps.http.as_ref().unwrap().credentials.len(), 1);
    assert!(caps.tool_invoke.is_some());
    assert_eq!(caps.tool_invoke.as_ref().unwrap().aliases.len(), 1);
    assert_eq!(
        caps.tool_invoke
            .as_ref()
            .unwrap()
            .aliases
            .get("search")
            .unwrap(),
        "brave_search"
    );
    assert!(caps.secrets.is_some());
}

#[test]
fn test_parse_no_capabilities_gives_none() {
    let content = r#"
[plugin]
id = "simple"
name = "Simple"
kind = "wasm"
entry = "plugin.wasm"
"#;
    let manifest = parse_aleph_plugin_toml_content(content, Path::new("/tmp/test")).unwrap();
    assert!(manifest.wasm_capabilities.is_none());
}
