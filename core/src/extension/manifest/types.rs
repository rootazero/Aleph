//! Unified plugin manifest types
//!
//! This module defines the unified `PluginManifest` type that can be parsed
//! from either `package.json` (Node.js plugins) or `aleph.plugin.json` (WASM plugins).

use crate::extension::types::PluginKind;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;

// V2 field types from aleph_plugin_toml module
use super::aleph_plugin_toml::{
    CapabilitiesSection, ChannelSection, CommandSection, HookSection, HttpRouteSection,
    PromptSection, ProviderSection, ServiceSection, ToolSection,
};

// =============================================================================
// Config UI Hints
// =============================================================================

/// UI hints for configuration fields
///
/// These hints help generate user-friendly configuration UIs
/// by providing additional context about each config field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUiHint {
    /// Human-readable label for the field
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Help text explaining the field's purpose
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,

    /// Whether this is an advanced option (hidden by default)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advanced: Option<bool>,

    /// Whether this field contains sensitive data (password, token, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive: Option<bool>,

    /// Placeholder text for input fields
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

// =============================================================================
// Plugin Permissions
// =============================================================================

/// Plugin permission types
///
/// Permissions control what system resources a plugin can access.
/// Plugins must declare required permissions in their manifest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginPermission {
    /// Network access (HTTP, WebSocket, etc.)
    Network,

    /// Read-only filesystem access
    #[serde(rename = "filesystem:read")]
    FilesystemRead,

    /// Write filesystem access (implies read)
    #[serde(rename = "filesystem:write")]
    FilesystemWrite,

    /// Full filesystem access (legacy, equivalent to read+write)
    Filesystem,

    /// Environment variable access
    Env,

    /// Custom/extension-specific permission
    #[serde(untagged)]
    Custom(String),
}

impl std::fmt::Display for PluginPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginPermission::Network => write!(f, "network"),
            PluginPermission::FilesystemRead => write!(f, "filesystem:read"),
            PluginPermission::FilesystemWrite => write!(f, "filesystem:write"),
            PluginPermission::Filesystem => write!(f, "filesystem"),
            PluginPermission::Env => write!(f, "env"),
            PluginPermission::Custom(s) => write!(f, "{}", s),
        }
    }
}

// =============================================================================
// Author Information
// =============================================================================

/// Plugin author information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthorInfo {
    /// Author name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Author email
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// Author URL (homepage, profile, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl AuthorInfo {
    /// Check if this author info has any content
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.email.is_none() && self.url.is_none()
    }
}

/// Parse author from npm package.json format
///
/// Supports both string format ("Name <email> (url)") and object format.
impl From<&str> for AuthorInfo {
    fn from(s: &str) -> Self {
        // Parse npm author string: "Name <email> (url)"
        let mut info = AuthorInfo::default();
        let mut remaining = s.trim();

        // Extract URL (last)
        if let Some(start) = remaining.rfind('(') {
            if let Some(end) = remaining.rfind(')') {
                if start < end {
                    info.url = Some(remaining[start + 1..end].trim().to_string());
                    remaining = remaining[..start].trim();
                }
            }
        }

        // Extract email
        if let Some(start) = remaining.rfind('<') {
            if let Some(end) = remaining.rfind('>') {
                if start < end {
                    info.email = Some(remaining[start + 1..end].trim().to_string());
                    remaining = remaining[..start].trim();
                }
            }
        }

        // Whatever remains is the name
        if !remaining.is_empty() {
            info.name = Some(remaining.to_string());
        }

        info
    }
}

// =============================================================================
// Plugin Manifest
// =============================================================================

/// Unified plugin manifest
///
/// This struct represents the parsed and normalized manifest data
/// from either `package.json` or `aleph.plugin.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier (lowercase, alphanumeric with hyphens)
    pub id: String,

    /// Human-readable plugin name
    pub name: String,

    /// Plugin version (semver format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Plugin description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Plugin type/kind
    pub kind: PluginKind,

    /// Entry point relative to plugin root
    pub entry: PathBuf,

    /// Plugin root directory (set after parsing, not serialized)
    #[serde(skip)]
    pub root_dir: PathBuf,

    /// JSON Schema for plugin configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<JsonValue>,

    /// UI hints for configuration fields
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub config_ui_hints: HashMap<String, ConfigUiHint>,

    /// Required permissions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<PluginPermission>,

    /// Author information
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<AuthorInfo>,

    /// Plugin homepage URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,

    /// Repository URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,

    /// License identifier (SPDX)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,

    /// Search keywords
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,

    /// Supported file extensions (for static plugins)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,

    // ═══════════════════════════════════════════
    // V2 Extension fields (from aleph_plugin.toml)
    // ═══════════════════════════════════════════

    /// V2: Static tool declarations from TOML
    #[serde(skip)]
    pub tools_v2: Option<Vec<ToolSection>>,

    /// V2: Typed hook declarations from TOML
    #[serde(skip)]
    pub hooks_v2: Option<Vec<HookSection>>,

    /// V2: Direct command declarations from TOML
    #[serde(skip)]
    pub commands_v2: Option<Vec<CommandSection>>,

    /// V2: Background service declarations from TOML
    #[serde(skip)]
    pub services_v2: Option<Vec<ServiceSection>>,

    /// V2: Global prompt configuration
    #[serde(skip)]
    pub prompt_v2: Option<PromptSection>,

    /// V2: Dynamic capability declarations
    #[serde(skip)]
    pub capabilities_v2: Option<CapabilitiesSection>,

    // ═══════════════════════════════════════════
    // P2 Extension fields
    // ═══════════════════════════════════════════

    /// V2: Channel definitions for messaging platform integrations
    #[serde(skip)]
    pub channels_v2: Option<Vec<ChannelSection>>,

    /// V2: Provider definitions for AI model providers
    #[serde(skip)]
    pub providers_v2: Option<Vec<ProviderSection>>,

    /// V2: HTTP route definitions for REST API endpoints
    #[serde(skip)]
    pub http_routes_v2: Option<Vec<HttpRouteSection>>,
}

impl PluginManifest {
    /// Create a new plugin manifest with required fields
    pub fn new(id: String, name: String, kind: PluginKind, entry: PathBuf) -> Self {
        Self {
            id,
            name,
            version: None,
            description: None,
            kind,
            entry,
            root_dir: PathBuf::new(),
            config_schema: None,
            config_ui_hints: HashMap::new(),
            permissions: Vec::new(),
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: Vec::new(),
            extensions: Vec::new(),
            // V2 fields
            tools_v2: None,
            hooks_v2: None,
            commands_v2: None,
            services_v2: None,
            prompt_v2: None,
            capabilities_v2: None,
            // P2 fields
            channels_v2: None,
            providers_v2: None,
            http_routes_v2: None,
        }
    }

    /// Set the root directory and return self (builder pattern)
    pub fn with_root_dir(mut self, root: PathBuf) -> Self {
        self.root_dir = root;
        self
    }

    /// Get the absolute path to the entry point
    pub fn entry_path(&self) -> PathBuf {
        if self.entry.is_absolute() {
            self.entry.clone()
        } else {
            self.root_dir.join(&self.entry)
        }
    }

    /// Check if this manifest has configuration schema
    pub fn has_config(&self) -> bool {
        self.config_schema.is_some()
    }

    /// Check if this manifest requires any permissions
    pub fn requires_permissions(&self) -> bool {
        !self.permissions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_author_info_from_string_full() {
        let author = AuthorInfo::from("John Doe <john@example.com> (https://example.com)");
        assert_eq!(author.name, Some("John Doe".to_string()));
        assert_eq!(author.email, Some("john@example.com".to_string()));
        assert_eq!(author.url, Some("https://example.com".to_string()));
    }

    #[test]
    fn test_author_info_from_string_name_only() {
        let author = AuthorInfo::from("John Doe");
        assert_eq!(author.name, Some("John Doe".to_string()));
        assert_eq!(author.email, None);
        assert_eq!(author.url, None);
    }

    #[test]
    fn test_author_info_from_string_name_and_email() {
        let author = AuthorInfo::from("John Doe <john@example.com>");
        assert_eq!(author.name, Some("John Doe".to_string()));
        assert_eq!(author.email, Some("john@example.com".to_string()));
        assert_eq!(author.url, None);
    }

    #[test]
    fn test_plugin_permission_display() {
        assert_eq!(PluginPermission::Network.to_string(), "network");
        assert_eq!(
            PluginPermission::FilesystemRead.to_string(),
            "filesystem:read"
        );
        assert_eq!(
            PluginPermission::FilesystemWrite.to_string(),
            "filesystem:write"
        );
        assert_eq!(PluginPermission::Filesystem.to_string(), "filesystem");
        assert_eq!(PluginPermission::Env.to_string(), "env");
        assert_eq!(
            PluginPermission::Custom("custom:perm".to_string()).to_string(),
            "custom:perm"
        );
    }

    #[test]
    fn test_plugin_manifest_new() {
        let manifest = PluginManifest::new(
            "my-plugin".to_string(),
            "My Plugin".to_string(),
            PluginKind::NodeJs,
            PathBuf::from("dist/index.js"),
        );

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.name, "My Plugin");
        assert_eq!(manifest.kind, PluginKind::NodeJs);
        assert_eq!(manifest.entry, PathBuf::from("dist/index.js"));
        assert!(manifest.root_dir.as_os_str().is_empty());
    }

    #[test]
    fn test_plugin_manifest_with_root_dir() {
        let manifest = PluginManifest::new(
            "my-plugin".to_string(),
            "My Plugin".to_string(),
            PluginKind::Wasm,
            PathBuf::from("plugin.wasm"),
        )
        .with_root_dir(PathBuf::from("/path/to/plugin"));

        assert_eq!(manifest.root_dir, PathBuf::from("/path/to/plugin"));
    }

    #[test]
    fn test_plugin_manifest_entry_path() {
        let manifest = PluginManifest::new(
            "my-plugin".to_string(),
            "My Plugin".to_string(),
            PluginKind::NodeJs,
            PathBuf::from("dist/index.js"),
        )
        .with_root_dir(PathBuf::from("/plugins/my-plugin"));

        assert_eq!(
            manifest.entry_path(),
            PathBuf::from("/plugins/my-plugin/dist/index.js")
        );
    }

    #[test]
    fn test_plugin_manifest_entry_path_absolute() {
        let manifest = PluginManifest::new(
            "my-plugin".to_string(),
            "My Plugin".to_string(),
            PluginKind::NodeJs,
            PathBuf::from("/absolute/path/index.js"),
        )
        .with_root_dir(PathBuf::from("/plugins/my-plugin"));

        // Absolute entry path should be returned as-is
        assert_eq!(
            manifest.entry_path(),
            PathBuf::from("/absolute/path/index.js")
        );
    }

    #[test]
    fn test_config_ui_hint_default() {
        let hint = ConfigUiHint::default();
        assert!(hint.label.is_none());
        assert!(hint.help.is_none());
        assert!(hint.advanced.is_none());
        assert!(hint.sensitive.is_none());
        assert!(hint.placeholder.is_none());
    }

    #[test]
    fn test_plugin_permission_serde() {
        // Serialize
        let perms = vec![
            PluginPermission::Network,
            PluginPermission::FilesystemRead,
            PluginPermission::Custom("my:perm".to_string()),
        ];
        let json = serde_json::to_string(&perms).unwrap();

        // Deserialize
        let parsed: Vec<PluginPermission> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], PluginPermission::Network);
        assert_eq!(parsed[1], PluginPermission::FilesystemRead);
        assert_eq!(parsed[2], PluginPermission::Custom("my:perm".to_string()));
    }
}
