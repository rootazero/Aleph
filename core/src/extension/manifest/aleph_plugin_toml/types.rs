//! TOML manifest type definitions
//!
//! All struct and enum types used for deserializing the `aleph.plugin.toml` format.

use crate::extension::manifest::types::{AuthorInfo, ConfigUiHint};
use crate::extension::types::PluginKind;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

use super::wasm_capabilities::{WasmHttpToml, WasmSecretsToml, WasmToolInvokeToml, WasmWorkspaceToml};

// =============================================================================
// TOML Manifest Types
// =============================================================================

/// Root structure for aleph.plugin.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlephPluginToml {
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

    // ═══════════════════════════════════════════
    // P2 Extension Sections
    // ═══════════════════════════════════════════

    /// Channel definitions for messaging platform integrations (optional)
    #[serde(default)]
    pub channels: Vec<ChannelSection>,

    /// Provider definitions for AI model providers (optional)
    #[serde(default)]
    pub providers: Vec<ProviderSection>,

    /// HTTP route definitions for REST API endpoints (optional)
    #[serde(default)]
    pub http_routes: Vec<HttpRouteSection>,
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

    // WASM sandbox capabilities

    /// Workspace read access
    #[serde(default)]
    pub workspace: Option<WasmWorkspaceToml>,

    /// HTTP access control
    #[serde(default)]
    pub http: Option<WasmHttpToml>,

    /// Tool invocation via aliases
    #[serde(default)]
    pub tool_invoke: Option<WasmToolInvokeToml>,

    /// Secret existence checking
    #[serde(default)]
    pub secrets: Option<WasmSecretsToml>,
}

// =============================================================================
// P2 Extension Types
// =============================================================================

/// Channel definition section for messaging platform integrations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSection {
    /// Unique channel identifier (e.g., "slack", "telegram")
    pub id: String,

    /// Display label for the channel
    pub label: String,

    /// Handler function name for receiving/sending messages
    #[serde(default)]
    pub handler: Option<String>,

    /// Optional configuration schema (JSON Schema as TOML inline table)
    #[serde(default)]
    pub config_schema: Option<JsonValue>,
}

/// Provider definition section for AI model providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSection {
    /// Unique provider identifier (e.g., "custom-llm")
    pub id: String,

    /// Display name for the provider
    pub name: String,

    /// List of model IDs supported by this provider
    #[serde(default)]
    pub models: Vec<String>,

    /// Handler function name for chat completions
    #[serde(default)]
    pub handler: Option<String>,

    /// Optional configuration schema (JSON Schema as TOML inline table)
    #[serde(default)]
    pub config_schema: Option<JsonValue>,
}

/// HTTP route definition section for REST API endpoints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRouteSection {
    /// URL path pattern (e.g., "/api/v1/data", "/api/items/{id}")
    pub path: String,

    /// HTTP methods allowed (e.g., ["GET", "POST"])
    #[serde(default)]
    pub methods: Vec<String>,

    /// Handler function name within the plugin
    pub handler: String,
}
