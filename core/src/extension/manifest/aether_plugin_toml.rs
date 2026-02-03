//! TOML manifest parser for Aether plugins (V2)
//!
//! This module parses the `aether.plugin.toml` manifest format, which provides
//! a more ergonomic and feature-rich way to define Aether plugins.
//!
//! # Example TOML Manifest
//!
//! ```toml
//! [plugin]
//! id = "my-plugin"
//! name = "My Plugin"
//! version = "1.0.0"
//! description = "A sample plugin"
//! kind = "wasm"
//! entry = "plugin.wasm"
//!
//! [permissions]
//! network = true
//! filesystem = "read"
//! env = false
//!
//! [prompt]
//! file = "SYSTEM.md"
//! scope = "system"
//!
//! [[tools]]
//! name = "my-tool"
//! description = "Does something useful"
//! handler = "handle_my_tool"
//!
//! [[hooks]]
//! event = "PreToolUse"
//! handler = "on_pre_tool_use"
//! ```

use super::types::{AuthorInfo, ConfigUiHint, PluginManifest, PluginPermission};
use super::{sanitize_plugin_id, validate_plugin_id};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::types::PluginKind;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

// =============================================================================
// TOML Manifest File Name
// =============================================================================

/// TOML manifest filename
pub const AETHER_PLUGIN_TOML: &str = "aether.plugin.toml";

// =============================================================================
// TOML Manifest Types
// =============================================================================

/// Root structure for aether.plugin.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AetherPluginToml {
    /// Plugin metadata section (required)
    pub plugin: PluginSection,

    /// Permissions section (optional)
    #[serde(default)]
    pub permissions: PermissionsSection,

    /// System prompt section (optional)
    #[serde(default)]
    pub prompt: Option<PromptSection>,

    /// Tool definitions (optional)
    #[serde(default)]
    pub tools: Vec<ToolSection>,

    /// Hook definitions (optional)
    #[serde(default)]
    pub hooks: Vec<HookSection>,

    /// Command definitions (optional)
    #[serde(default)]
    pub commands: Vec<CommandSection>,

    /// Service definitions (optional)
    #[serde(default)]
    pub services: Vec<ServiceSection>,

    /// Advanced capabilities (optional)
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
}

/// Plugin metadata section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSection {
    /// Unique plugin identifier (required)
    pub id: String,

    /// Human-readable name (optional, defaults to id)
    #[serde(default)]
    pub name: Option<String>,

    /// Plugin version (semver format)
    #[serde(default)]
    pub version: Option<String>,

    /// Plugin description
    #[serde(default)]
    pub description: Option<String>,

    /// Plugin kind (wasm, nodejs, static)
    #[serde(default)]
    pub kind: Option<PluginKind>,

    /// Entry point relative to plugin root
    #[serde(default)]
    pub entry: Option<String>,

    /// Author information
    #[serde(default)]
    pub author: Option<PluginAuthorToml>,

    /// Configuration schema (JSON Schema as TOML inline table or file reference)
    #[serde(default)]
    pub config_schema: Option<JsonValue>,

    /// UI hints for configuration fields
    #[serde(default)]
    pub config_ui_hints: Option<HashMap<String, ConfigUiHint>>,

    /// Homepage URL
    #[serde(default)]
    pub homepage: Option<String>,

    /// Repository URL
    #[serde(default)]
    pub repository: Option<String>,

    /// License identifier (SPDX)
    #[serde(default)]
    pub license: Option<String>,

    /// Search keywords
    #[serde(default)]
    pub keywords: Option<Vec<String>>,

    /// Supported file extensions (for static plugins)
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
}

/// Plugin author information (TOML format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthorToml {
    /// Author name
    #[serde(default)]
    pub name: Option<String>,

    /// Author email
    #[serde(default)]
    pub email: Option<String>,

    /// Author URL (homepage, profile, etc.)
    #[serde(default)]
    pub url: Option<String>,
}

impl From<PluginAuthorToml> for AuthorInfo {
    fn from(author: PluginAuthorToml) -> Self {
        AuthorInfo {
            name: author.name,
            email: author.email,
            url: author.url,
        }
    }
}

/// Permissions section
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsSection {
    /// Network access (HTTP, WebSocket, etc.)
    #[serde(default)]
    pub network: bool,

    /// Filesystem access: true = full, "read" = read-only, "write" = write, false = none
    #[serde(default)]
    pub filesystem: FilesystemPermission,

    /// Environment variable access
    #[serde(default)]
    pub env: bool,

    /// Shell execution permission
    #[serde(default)]
    pub shell: bool,
}

/// Filesystem permission level
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilesystemPermission {
    /// Boolean: true = full access, false = no access
    Bool(bool),
    /// String: "read", "write", or "full"
    Level(String),
}

impl Default for FilesystemPermission {
    fn default() -> Self {
        FilesystemPermission::Bool(false)
    }
}

impl FilesystemPermission {
    /// Check if read access is granted
    pub fn can_read(&self) -> bool {
        match self {
            FilesystemPermission::Bool(true) => true,
            FilesystemPermission::Bool(false) => false,
            FilesystemPermission::Level(s) => matches!(s.as_str(), "read" | "write" | "full"),
        }
    }

    /// Check if write access is granted
    pub fn can_write(&self) -> bool {
        match self {
            FilesystemPermission::Bool(true) => true,
            FilesystemPermission::Bool(false) => false,
            FilesystemPermission::Level(s) => matches!(s.as_str(), "write" | "full"),
        }
    }
}


/// System prompt section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSection {
    /// Path to the prompt file (relative to plugin root)
    pub file: String,

    /// Scope of the prompt: "system" or "user"
    #[serde(default = "default_prompt_scope")]
    pub scope: String,
}

fn default_prompt_scope() -> String {
    "system".to_string()
}

/// Tool definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSection {
    /// Tool name (required)
    pub name: String,

    /// Tool description
    #[serde(default)]
    pub description: Option<String>,

    /// Handler function name in the plugin
    #[serde(default)]
    pub handler: Option<String>,

    /// Path to instruction file (markdown)
    #[serde(default)]
    pub instruction_file: Option<String>,

    /// Parameter definitions (JSON Schema format)
    #[serde(default)]
    pub parameters: Option<JsonValue>,
}

/// Hook definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSection {
    /// Event to hook (required)
    pub event: String,

    /// Hook kind: "observer" (read-only) or "interceptor" (can modify)
    #[serde(default = "default_hook_kind")]
    pub kind: String,

    /// Handler function name in the plugin
    #[serde(default)]
    pub handler: Option<String>,

    /// Priority: "low", "normal", "high"
    #[serde(default = "default_hook_priority")]
    pub priority: String,

    /// Filter pattern (regex for tool-based events)
    #[serde(default)]
    pub filter: Option<String>,
}

fn default_hook_kind() -> String {
    "observer".to_string()
}

fn default_hook_priority() -> String {
    "normal".to_string()
}

/// Command definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSection {
    /// Command name (required)
    pub name: String,

    /// Command description
    #[serde(default)]
    pub description: Option<String>,

    /// Handler function name in the plugin
    #[serde(default)]
    pub handler: Option<String>,

    /// Path to prompt file (markdown with $ARGUMENTS placeholder)
    #[serde(default)]
    pub prompt_file: Option<String>,
}

/// Service definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSection {
    /// Service name (required)
    pub name: String,

    /// Service description
    #[serde(default)]
    pub description: Option<String>,

    /// Handler for service start
    #[serde(default)]
    pub start_handler: Option<String>,

    /// Handler for service stop
    #[serde(default)]
    pub stop_handler: Option<String>,
}

/// Advanced capabilities section
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesSection {
    /// Plugin can dynamically register tools at runtime
    #[serde(default)]
    pub dynamic_tools: bool,

    /// Plugin can dynamically register hooks at runtime
    #[serde(default)]
    pub dynamic_hooks: bool,
}

// =============================================================================
// Permission Conversion
// =============================================================================

/// Convert TOML permissions section to PluginPermission list
pub fn convert_permissions(perms: &PermissionsSection) -> Vec<PluginPermission> {
    let mut permissions = Vec::new();

    if perms.network {
        permissions.push(PluginPermission::Network);
    }

    match &perms.filesystem {
        FilesystemPermission::Bool(true) => {
            permissions.push(PluginPermission::Filesystem);
        }
        FilesystemPermission::Bool(false) => {}
        FilesystemPermission::Level(level) => match level.as_str() {
            "read" => permissions.push(PluginPermission::FilesystemRead),
            "write" => permissions.push(PluginPermission::FilesystemWrite),
            "full" => permissions.push(PluginPermission::Filesystem),
            _ => {}
        },
    }

    if perms.env {
        permissions.push(PluginPermission::Env);
    }

    if perms.shell {
        permissions.push(PluginPermission::Custom("shell".to_string()));
    }

    permissions
}

// =============================================================================
// Parsers
// =============================================================================

/// Parse an aether.plugin.toml file into a PluginManifest (async)
///
/// # Arguments
/// * `dir` - Path to the plugin directory containing aether.plugin.toml
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest with root_dir set
/// * `Err(ExtensionError)` - If parsing fails or required fields are missing
pub async fn parse_aether_plugin_toml(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(AETHER_PLUGIN_TOML);
    let content = tokio::fs::read_to_string(&toml_path).await?;
    parse_aether_plugin_toml_content(&content, dir)
}

/// Parse an aether.plugin.toml file into a PluginManifest (sync)
///
/// # Arguments
/// * `dir` - Path to the plugin directory containing aether.plugin.toml
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest with root_dir set
/// * `Err(ExtensionError)` - If parsing fails or required fields are missing
pub fn parse_aether_plugin_toml_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(AETHER_PLUGIN_TOML);
    let content = std::fs::read_to_string(&toml_path)?;
    parse_aether_plugin_toml_content(&content, dir)
}

/// Parse TOML content into a PluginManifest
///
/// This is the core parsing function that converts TOML content to PluginManifest.
///
/// # Arguments
/// * `content` - TOML content string
/// * `plugin_dir` - Path to the plugin directory (for root_dir)
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest
/// * `Err(ExtensionError)` - If parsing fails or validation fails
pub fn parse_aether_plugin_toml_content(
    content: &str,
    plugin_dir: &Path,
) -> ExtensionResult<PluginManifest> {
    let toml_path = plugin_dir.join(AETHER_PLUGIN_TOML);

    // Parse TOML
    let toml: AetherPluginToml = toml::from_str(content)
        .map_err(|e| ExtensionError::invalid_manifest(&toml_path, format!("TOML parse error: {}", e)))?;

    // Validate plugin ID
    let plugin_id = if toml.plugin.id.is_empty() {
        return Err(ExtensionError::missing_field(&toml_path, "plugin.id"));
    } else {
        // Sanitize the ID if needed
        let sanitized = sanitize_plugin_id(&toml.plugin.id);
        validate_plugin_id(&sanitized)
            .map_err(|reason| ExtensionError::invalid_plugin_name(&toml.plugin.id, reason))?;
        sanitized
    };

    // Determine display name
    let name = toml.plugin.name.unwrap_or_else(|| plugin_id.clone());

    // Determine plugin kind (default to Wasm)
    let kind = toml.plugin.kind.unwrap_or(PluginKind::Wasm);

    // Determine entry point based on kind
    let entry = toml.plugin.entry.unwrap_or_else(|| match kind {
        PluginKind::Wasm => "plugin.wasm".to_string(),
        PluginKind::NodeJs => "index.js".to_string(),
        PluginKind::Static => ".".to_string(),
    });

    // Convert permissions
    let permissions = convert_permissions(&toml.permissions);

    // Build manifest
    let manifest = PluginManifest {
        id: plugin_id,
        name,
        version: toml.plugin.version,
        description: toml.plugin.description,
        kind,
        entry: entry.into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: toml.plugin.config_schema,
        config_ui_hints: toml.plugin.config_ui_hints.unwrap_or_default(),
        permissions,
        author: toml.plugin.author.map(AuthorInfo::from),
        homepage: toml.plugin.homepage,
        repository: toml.plugin.repository,
        license: toml.plugin.license,
        keywords: toml.plugin.keywords.unwrap_or_default(),
        extensions: toml.plugin.extensions.unwrap_or_default(),
        // V2 fields from TOML
        tools_v2: if toml.tools.is_empty() {
            None
        } else {
            Some(toml.tools)
        },
        hooks_v2: if toml.hooks.is_empty() {
            None
        } else {
            Some(toml.hooks)
        },
        commands_v2: if toml.commands.is_empty() {
            None
        } else {
            Some(toml.commands)
        },
        services_v2: if toml.services.is_empty() {
            None
        } else {
            Some(toml.services)
        },
        prompt_v2: toml.prompt,
        capabilities_v2: Some(toml.capabilities),
    };

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_minimal_toml() {
        let content = r#"
[plugin]
id = "my-plugin"
"#;

        let manifest =
            parse_aether_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

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
            parse_aether_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

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

        let result = parse_aether_plugin_toml_content(content, Path::new("/test/plugin"));
        assert!(result.is_err());

        // When the id field is missing, toml parser fails with InvalidManifest
        // because `id` is a required field in PluginSection struct
        let err = result.unwrap_err();
        assert!(
            matches!(err, ExtensionError::InvalidManifest { .. }),
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

        let result = parse_aether_plugin_toml_content(content, Path::new("/test/plugin"));
        assert!(result.is_err());

        // When id is empty string, we check it explicitly and return MissingField
        let err = result.unwrap_err();
        assert!(
            matches!(err, ExtensionError::MissingField { .. }),
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
        let result = parse_aether_plugin_toml_content(content, Path::new("/test/plugin"));
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
            parse_aether_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

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
            parse_aether_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

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
            parse_aether_plugin_toml_content(content, Path::new("/test/plugin")).unwrap();

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

        let toml: AetherPluginToml = toml::from_str(content).unwrap();
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

        let toml: AetherPluginToml = toml::from_str(content).unwrap();
        let hook = &toml.hooks[0];

        assert_eq!(hook.event, "SessionStart");
        assert_eq!(hook.kind, "observer"); // default
        assert_eq!(hook.priority, "normal"); // default
        assert_eq!(hook.handler, Some("on_session_start".to_string()));
        assert!(hook.filter.is_none());
    }
}
