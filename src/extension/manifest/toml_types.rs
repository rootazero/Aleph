//! TOML manifest types, parsing, and conversion
//!
//! This module consolidates all TOML-specific types and parsing from the
//! former `aleph_plugin_toml/` directory into a single file.
//!
//! It provides:
//! - `AlephPluginToml` and section structs for deserialization
//! - WASM capability TOML types and conversion
//! - `convert_permissions` for permission section handling
//! - `parse_aleph_plugin_toml*` functions for aleph.plugin.toml parsing

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::types::{AuthorInfo, ConfigUiHint, PluginManifest};
use crate::extension::manifest::{sanitize_plugin_id, validate_plugin_id};
use crate::extension::runtime::wasm::{
    CredentialBinding, CredentialInject, EndpointPattern, HttpCapability, RateLimit,
    SecretsCapability, WasmCapabilities, WorkspaceCapability,
};
use crate::extension::types::PluginKind;
use crate::memory::extensions::manifest::MemoryManifestSection;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

use super::types::{FilesystemAccess, PluginPermission};

// =============================================================================
// Constants
// =============================================================================

/// TOML manifest filename
pub const ALEPH_PLUGIN_TOML: &str = "aleph.plugin.toml";

// =============================================================================
// Root TOML Structure
// =============================================================================

/// Root structure for aleph.plugin.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlephPluginToml {
    pub plugin: PluginSection,
    #[serde(default)]
    pub permissions: PermissionsSection,
    #[serde(default)]
    pub prompt: Option<PromptSection>,
    #[serde(default)]
    pub tools: Vec<ToolSection>,
    #[serde(default)]
    pub hooks: Vec<HookSection>,
    #[serde(default)]
    pub commands: Vec<CommandSection>,
    #[serde(default)]
    pub services: Vec<ServiceSection>,
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
    /// Optional [memory] section — declares which memory extension hooks this plugin implements.
    #[serde(default)]
    pub memory: Option<MemoryManifestSection>,
}

// =============================================================================
// Section Types
// =============================================================================

/// Plugin metadata section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSection {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kind: Option<PluginKind>,
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub author: Option<PluginAuthorToml>,
    #[serde(default)]
    pub config_schema: Option<JsonValue>,
    #[serde(default)]
    pub config_ui_hints: Option<HashMap<String, ConfigUiHint>>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
}

/// Plugin author information (TOML format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthorToml {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl From<PluginAuthorToml> for AuthorInfo {
    fn from(author: PluginAuthorToml) -> Self {
        Self {
            name: author.name,
            email: author.email,
            url: author.url,
        }
    }
}

/// Permissions section
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsSection {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub filesystem: FilesystemPermission,
    #[serde(default)]
    pub env: bool,
    #[serde(default)]
    pub shell: bool,
    /// Allow the plugin to run declared `[[services]]` in the background.
    #[serde(default)]
    pub background: bool,
}

/// Filesystem permission level
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilesystemPermission {
    Bool(bool),
    Level(String),
}

impl Default for FilesystemPermission {
    fn default() -> Self {
        Self::Bool(false)
    }
}

impl FilesystemPermission {
    #[must_use]
    pub fn can_read(&self) -> bool {
        match self {
            Self::Bool(true) => true,
            Self::Bool(false) => false,
            Self::Level(s) => matches!(s.as_str(), "read" | "write" | "full"),
        }
    }

    #[must_use]
    pub fn can_write(&self) -> bool {
        match self {
            Self::Bool(true) => true,
            Self::Bool(false) => false,
            Self::Level(s) => matches!(s.as_str(), "write" | "full"),
        }
    }
}

/// System prompt section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSection {
    pub file: String,
    #[serde(default = "default_prompt_scope")]
    pub scope: String,
}

fn default_prompt_scope() -> String {
    "system".to_string()
}

/// Tool definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSection {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub handler: Option<String>,
    #[serde(default)]
    pub instruction_file: Option<String>,
    #[serde(default)]
    pub parameters: Option<JsonValue>,
}

/// Hook definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSection {
    pub event: String,
    /// Execution kind (`observer` | `interceptor`). `None` (omitted) defers
    /// to the per-event default at registration time — hard-defaulting to
    /// `"observer"` here would silently kill hooks on interceptor-only
    /// events (`before_tool_call`, `stop`, …) whose authors omitted the key.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub handler: Option<String>,
    #[serde(default = "default_hook_priority")]
    pub priority: String,
    #[serde(default)]
    pub filter: Option<String>,
}

fn default_hook_priority() -> String {
    "normal".to_string()
}

/// Command definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSection {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub handler: Option<String>,
    #[serde(default)]
    pub prompt_file: Option<String>,
}

/// Service definition section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSection {
    /// The service's identifier.
    ///
    /// `id` is accepted as an input spelling because that is what the plugins
    /// Aleph itself ships write, and a required field under a name the author
    /// did not use is not a missing field — serde fails the **whole document**,
    /// so `diagnostics` and `voice-call` (every bundled plugin that declares a
    /// service) could not be installed at all. Their manifests parse as valid
    /// TOML; it is this struct that rejected them.
    ///
    /// Widening the input rather than renaming this field, and rather than
    /// editing the manifests: the plugins live in a separate repository on a
    /// separate release cadence, so a parser that accepts only `name` also
    /// bricks every third-party plugin already published with `id`. The
    /// vocabulary still has one owner — this field — and `plugin.toml` is
    /// never written back (the only manifest Aleph serialises is
    /// `plugins.toml`, the operator's own document), so an author's chosen
    /// spelling is never rewritten underneath them.
    #[serde(alias = "id")]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub start_handler: Option<String>,
    #[serde(default)]
    pub stop_handler: Option<String>,
    /// Start automatically when the plugin is loaded (mirrors the MCP
    /// transient-server `auto_start(true)` precedent).
    #[serde(default = "default_service_auto_start")]
    pub auto_start: bool,
}

const fn default_service_auto_start() -> bool {
    true
}

/// Advanced capabilities section
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesSection {
    #[serde(default)]
    pub dynamic_tools: bool,
    #[serde(default)]
    pub dynamic_hooks: bool,
    #[serde(default)]
    pub workspace: Option<WasmWorkspaceToml>,
    #[serde(default)]
    pub http: Option<WasmHttpToml>,
    #[serde(default)]
    pub secrets: Option<WasmSecretsToml>,
}

// =============================================================================
// WASM Capability TOML Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmWorkspaceToml {
    #[serde(default)]
    pub allowed_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmHttpToml {
    #[serde(default)]
    pub allowlist: Vec<WasmEndpointToml>,
    #[serde(default)]
    pub credentials: Vec<WasmCredentialToml>,
    #[serde(default)]
    pub rate_limit: Option<WasmRateLimitToml>,
    #[serde(default = "default_http_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
}

const fn default_http_timeout() -> u64 {
    30
}
const fn default_max_request_bytes() -> usize {
    1_048_576
}
const fn default_max_response_bytes() -> usize {
    10_485_760
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmEndpointToml {
    pub host: String,
    #[serde(default = "default_toml_path_prefix")]
    pub path_prefix: String,
    #[serde(default)]
    pub methods: Vec<String>,
}

fn default_toml_path_prefix() -> String {
    "/".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmCredentialToml {
    pub secret_name: String,
    pub inject: WasmCredentialInjectToml,
    #[serde(default)]
    pub host_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WasmCredentialInjectToml {
    Bearer,
    Basic {
        username: String,
    },
    Header {
        name: String,
        #[serde(default)]
        prefix: Option<String>,
    },
    Query {
        param_name: String,
    },
    UrlPath {
        placeholder: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmRateLimitToml {
    #[serde(default)]
    pub requests_per_minute: u32,
    #[serde(default)]
    pub requests_per_hour: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmSecretsToml {
    #[serde(default)]
    pub allowed_patterns: Vec<String>,
}

// =============================================================================
// WASM Capability Conversion
// =============================================================================

/// Convert TOML capabilities section to runtime `WasmCapabilities`
#[must_use]
pub fn convert_wasm_capabilities(caps: &CapabilitiesSection) -> Option<WasmCapabilities> {
    if caps.workspace.is_none() && caps.http.is_none() && caps.secrets.is_none() {
        return None;
    }

    Some(WasmCapabilities {
        workspace: caps.workspace.as_ref().map(|w| WorkspaceCapability {
            allowed_prefixes: w.allowed_prefixes.clone(),
        }),
        http: caps.http.as_ref().map(|h| HttpCapability {
            allowlist: h
                .allowlist
                .iter()
                .map(|e| EndpointPattern {
                    host: e.host.clone(),
                    path_prefix: e.path_prefix.clone(),
                    methods: e.methods.clone(),
                })
                .collect(),
            credentials: h
                .credentials
                .iter()
                .map(|c| CredentialBinding {
                    secret_name: c.secret_name.clone(),
                    inject: convert_credential_inject(&c.inject),
                    host_patterns: c.host_patterns.clone(),
                })
                .collect(),
            rate_limit: h.rate_limit.as_ref().map(|r| RateLimit {
                requests_per_minute: r.requests_per_minute,
                requests_per_hour: r.requests_per_hour,
            }),
            timeout_secs: h.timeout_secs,
            max_request_bytes: h.max_request_bytes,
            max_response_bytes: h.max_response_bytes,
        }),
        secrets: caps.secrets.as_ref().map(|s| SecretsCapability {
            allowed_patterns: s.allowed_patterns.clone(),
        }),
    })
}

fn convert_credential_inject(inject: &WasmCredentialInjectToml) -> CredentialInject {
    match inject {
        WasmCredentialInjectToml::Bearer => CredentialInject::Bearer,
        WasmCredentialInjectToml::Basic { username } => CredentialInject::Basic {
            username: username.clone(),
        },
        WasmCredentialInjectToml::Header { name, prefix } => CredentialInject::Header {
            name: name.clone(),
            prefix: prefix.clone(),
        },
        WasmCredentialInjectToml::Query { param_name } => CredentialInject::Query {
            param_name: param_name.clone(),
        },
        WasmCredentialInjectToml::UrlPath { placeholder } => CredentialInject::UrlPath {
            placeholder: placeholder.clone(),
        },
    }
}

// =============================================================================
// Permission Conversion
// =============================================================================

/// Convert TOML permissions section to `PluginPermission` list
pub fn convert_permissions(perms: &PermissionsSection) -> Vec<PluginPermission> {
    let mut permissions = Vec::new();
    if perms.network {
        permissions.push(PluginPermission::Network);
    }
    match &perms.filesystem {
        FilesystemPermission::Bool(true) => {
            permissions.push(PluginPermission::Filesystem(FilesystemAccess::Full))
        }
        FilesystemPermission::Bool(false) => {}
        FilesystemPermission::Level(level) => match level.as_str() {
            "read" => permissions.push(PluginPermission::Filesystem(FilesystemAccess::Read)),
            "write" => permissions.push(PluginPermission::Filesystem(FilesystemAccess::Write)),
            "full" => permissions.push(PluginPermission::Filesystem(FilesystemAccess::Full)),
            _ => {
                tracing::warn!("Unknown filesystem permission level '{}', ignoring", level);
            }
        },
    }
    if perms.env {
        permissions.push(PluginPermission::Env);
    }
    if perms.shell {
        permissions.push(PluginPermission::Shell);
    }
    if perms.background {
        permissions.push(PluginPermission::Background);
    }
    permissions
}

// =============================================================================
// Parsing Functions
// =============================================================================

/// Parse an aleph.plugin.toml file into a `PluginManifest` (async)
pub async fn parse_aleph_plugin_toml(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(ALEPH_PLUGIN_TOML);
    let content = tokio::fs::read_to_string(&toml_path).await?;
    parse_aleph_plugin_toml_content(&content, dir)
}

/// Parse an aleph.plugin.toml file into a `PluginManifest` (sync)
pub fn parse_aleph_plugin_toml_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(ALEPH_PLUGIN_TOML);
    let content = std::fs::read_to_string(&toml_path)?;
    parse_aleph_plugin_toml_content(&content, dir)
}

/// Parse TOML content into a `PluginManifest`
pub fn parse_aleph_plugin_toml_content(
    content: &str,
    plugin_dir: &Path,
) -> ExtensionResult<PluginManifest> {
    let toml_path = plugin_dir.join(ALEPH_PLUGIN_TOML);

    let toml: AlephPluginToml = toml::from_str(content).map_err(|e| {
        ExtensionError::invalid_manifest(&toml_path, format!("TOML parse error: {e}"))
    })?;

    let plugin_id = if toml.plugin.id.is_empty() {
        return Err(ExtensionError::missing_field(&toml_path, "plugin.id"));
    } else {
        let sanitized = sanitize_plugin_id(&toml.plugin.id);
        validate_plugin_id(&sanitized)
            .map_err(|reason| ExtensionError::invalid_plugin_name(&toml.plugin.id, reason))?;
        sanitized
    };

    let name = toml.plugin.name.unwrap_or_else(|| plugin_id.clone());
    let kind = toml.plugin.kind.unwrap_or(PluginKind::Wasm);

    let entry = toml.plugin.entry.unwrap_or_else(|| match kind {
        PluginKind::Wasm => "plugin.wasm".to_string(),
        PluginKind::Mcp => ".mcp.json".to_string(),
        PluginKind::Static => ".".to_string(),
    });

    let permissions = convert_permissions(&toml.permissions);

    Ok(PluginManifest {
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
        wasm_capabilities: convert_wasm_capabilities(&toml.capabilities),
        wasm_resource_limits: None,
        capabilities_v2: Some(toml.capabilities),
        aleph_extensions: None,
        memory_manifest: toml.memory,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::extensions::manifest::MemoryHook;

    #[test]
    fn plugin_manifest_with_memory_section_parses() {
        let toml_str = r#"
[plugin]
id = "memory-obsidian"
name = "Memory Obsidian"
version = "1.0.0"

[memory]
hooks = ["on_retrieve"]
priority = 50
"#;
        let parsed: AlephPluginToml = toml::from_str(toml_str).unwrap();
        let mem = parsed
            .memory
            .as_ref()
            .expect("memory section should be present");
        assert_eq!(mem.hooks.len(), 1);
        assert_eq!(mem.hooks[0], MemoryHook::OnRetrieve);
        assert_eq!(mem.priority, 50);
        assert!(mem.produce_interval_seconds.is_none());
    }

    #[test]
    fn plugin_manifest_without_memory_section_is_none() {
        let toml_str = r#"
[plugin]
id = "plain-plugin"
"#;
        let parsed: AlephPluginToml = toml::from_str(toml_str).unwrap();
        assert!(parsed.memory.is_none());
    }
}
