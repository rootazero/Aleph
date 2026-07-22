//! Unified plugin manifest types
//!
//! This module defines the unified `PluginManifest` type that can be parsed
//! from either `package.json` (Node.js plugins) or `aleph.plugin.json` (WASM plugins).

use crate::extension::error::ExtensionError;
use crate::extension::types::PluginKind;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;

// V2 field types from toml_types module
use super::toml_types::{
    CapabilitiesSection, CommandSection, HookSection, PermissionsSection, PromptSection,
    ServiceSection, ToolSection,
};
use crate::extension::runtime::wasm::WasmCapabilities;
use crate::extension::runtime::wasm::WasmResourceLimits;

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

/// Filesystem access level for plugin permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilesystemAccess {
    /// Read-only filesystem access
    Read,
    /// Write filesystem access (implies read)
    Write,
    /// Full filesystem access (read + write)
    Full,
}

/// Plugin permission types
///
/// Permissions control what system resources a plugin can access.
/// Plugins must declare required permissions in their manifest.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PluginPermission {
    /// Network access (HTTP, WebSocket, etc.)
    Network,

    /// Filesystem access with a specific level
    Filesystem(FilesystemAccess),

    /// Environment variable access
    Env,

    /// Shell command execution
    Shell,

    /// Background service registration
    Background,

    /// Custom/extension-specific permission
    Custom(String),
}

impl std::fmt::Display for PluginPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network => write!(f, "network"),
            Self::Filesystem(FilesystemAccess::Read) => write!(f, "filesystem:read"),
            Self::Filesystem(FilesystemAccess::Write) => write!(f, "filesystem:write"),
            Self::Filesystem(FilesystemAccess::Full) => write!(f, "filesystem"),
            Self::Env => write!(f, "env"),
            Self::Shell => write!(f, "shell"),
            Self::Background => write!(f, "background"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

// Custom serde: serialize as the Display string, deserialize from it.
// This maintains backward compat with "filesystem:read", "filesystem:write", "filesystem".

impl Serialize for PluginPermission {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PluginPermission {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str(&s))
    }
}

impl PluginPermission {
    /// Parse a permission string into a `PluginPermission`.
    ///
    /// Unrecognized strings are kept as `Custom` so deserialization never
    /// fails on an unknown name.
    fn from_str(s: &str) -> Self {
        match s {
            "network" => Self::Network,
            "filesystem:read" => Self::Filesystem(FilesystemAccess::Read),
            "filesystem:write" => Self::Filesystem(FilesystemAccess::Write),
            "filesystem" => Self::Filesystem(FilesystemAccess::Full),
            "env" => Self::Env,
            "shell" => Self::Shell,
            "background" => Self::Background,
            other => Self::Custom(other.to_string()),
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
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.name.is_none() && self.email.is_none() && self.url.is_none()
    }
}

/// Parse author from npm package.json format
///
/// Supports both string format ("Name <email> (url)") and object format.
impl From<&str> for AuthorInfo {
    fn from(s: &str) -> Self {
        // Parse npm author string: "Name <email> (url)".
        //
        // Use `find` (leftmost) rather than `rfind`: the LAST `(` could
        // belong to a name segment in parentheses, e.g.
        // "Note (about X) <real@email.com>" — `rfind('(')` would return
        // the position of "about" and the email stage would then mis-extract.
        // Likewise "x (y) <z>" would lose the email because rfind('>') on
        // the remaining "x (y)" finds nothing.
        let mut info = Self::default();
        let mut remaining = s.trim();

        // Extract URL (leftmost `(...)`).
        if let Some(start) = remaining.find('(') {
            if let Some(end) = remaining[start..].find(')') {
                let end = start + end;
                if start < end {
                    info.url = Some(remaining[start + 1..end].trim().to_string());
                    remaining = remaining[..start].trim();
                }
            }
        }

        // Extract email (leftmost `<...>`).
        if let Some(start) = remaining.find('<') {
            if let Some(end) = remaining[start..].find('>') {
                let end = start + end;
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

    /// WASM sandbox capability declarations (parsed from [plugin.capabilities])
    #[serde(skip)]
    pub wasm_capabilities: Option<WasmCapabilities>,

    /// WASM resource limits (parsed from [plugin.limits] or defaults)
    #[serde(skip)]
    pub wasm_resource_limits: Option<WasmResourceLimits>,

    /// Aleph-only extensions from [aleph] section in CC-format manifest
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aleph_extensions: Option<AlephExtensions>,

    /// Optional [memory] section — memory extension hook declarations.
    /// Parsed from aleph.plugin.toml or .claude-plugin/plugin.toml [memory].
    #[serde(skip)]
    pub memory_manifest: Option<crate::memory::extensions::manifest::MemoryManifestSection>,
}

impl PluginManifest {
    /// Create a new plugin manifest with required fields
    #[must_use]
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
            wasm_capabilities: None,
            wasm_resource_limits: None,
            // CC-compat extensions
            aleph_extensions: None,
            // Memory extension manifest section
            memory_manifest: None,
        }
    }

    /// Set the root directory and return self (builder pattern)
    #[must_use]
    pub fn with_root_dir(mut self, root: PathBuf) -> Self {
        self.root_dir = root;
        self
    }

    /// Get the absolute path to the entry point
    ///
    /// Returns an error if the entry path contains traversal components
    /// or escapes the root directory.
    pub fn entry_path(&self) -> Result<PathBuf, ExtensionError> {
        let entry_str = self.entry.to_string_lossy();
        if self.entry.is_absolute() || entry_str.contains("..") {
            return Err(ExtensionError::Runtime(format!(
                "Path traversal not allowed in plugin entry: {entry_str}"
            )));
        }

        let resolved = self.root_dir.join(&self.entry);

        if resolved.exists() && !self.root_dir.as_os_str().is_empty() {
            match (resolved.canonicalize(), self.root_dir.canonicalize()) {
                (Ok(canonical_entry), Ok(canonical_root)) => {
                    if !canonical_entry.starts_with(&canonical_root) {
                        return Err(ExtensionError::Runtime(format!(
                            "Plugin entry path escapes root directory: {canonical_entry:?} is not within {canonical_root:?}"
                        )));
                    }
                }
                _ => {
                    return Err(ExtensionError::Runtime(format!(
                        "Unable to verify plugin entry path: {resolved:?}"
                    )));
                }
            }
        }

        Ok(resolved)
    }

    /// Check if this manifest has configuration schema
    #[must_use]
    pub const fn has_config(&self) -> bool {
        self.config_schema.is_some()
    }

    /// Check if this manifest requires any permissions
    #[must_use]
    pub const fn requires_permissions(&self) -> bool {
        !self.permissions.is_empty()
    }
}

// =============================================================================
// Aleph Extensions (CC-compat superset)
// =============================================================================

/// Runtime type for Aleph plugins
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlephRuntime {
    /// MCP Server protocol (default for Node.js, Python, etc.)
    #[default]
    Mcp,
    /// WASM via Extism (sandbox)
    Wasm,
    /// Static (Markdown only, no runtime)
    Static,
}

/// Aleph-only extension fields in plugin.toml [aleph] section.
/// Claude Code ignores these fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AlephExtensions {
    /// Runtime type
    pub runtime: AlephRuntime,
    /// WASM entry point (only for runtime = "wasm")
    pub entry: Option<String>,
    /// Background services
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceSection>,
    /// Permission grants
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermissionsSection>,
    /// WASM-specific capabilities
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<CapabilitiesSection>,
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
            PluginPermission::Filesystem(FilesystemAccess::Read).to_string(),
            "filesystem:read"
        );
        assert_eq!(
            PluginPermission::Filesystem(FilesystemAccess::Write).to_string(),
            "filesystem:write"
        );
        assert_eq!(
            PluginPermission::Filesystem(FilesystemAccess::Full).to_string(),
            "filesystem"
        );
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
            PluginKind::Mcp,
            PathBuf::from("dist/index.js"),
        );

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.name, "My Plugin");
        assert_eq!(manifest.kind, PluginKind::Mcp);
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
            PluginKind::Mcp,
            PathBuf::from("dist/index.js"),
        )
        .with_root_dir(PathBuf::from("/plugins/my-plugin"));

        assert_eq!(
            manifest.entry_path().unwrap(),
            PathBuf::from("/plugins/my-plugin/dist/index.js")
        );
    }

    #[test]
    fn test_plugin_manifest_entry_path_absolute() {
        let manifest = PluginManifest::new(
            "my-plugin".to_string(),
            "My Plugin".to_string(),
            PluginKind::Mcp,
            PathBuf::from("/absolute/path/index.js"),
        )
        .with_root_dir(PathBuf::from("/plugins/my-plugin"));

        assert!(manifest.entry_path().is_err());
    }

    #[test]
    fn test_plugin_manifest_entry_path_traversal_blocked() {
        let manifest = PluginManifest::new(
            "evil-plugin".to_string(),
            "Evil Plugin".to_string(),
            PluginKind::Mcp,
            PathBuf::from("../../etc/passwd"),
        )
        .with_root_dir(PathBuf::from("/plugins/evil-plugin"));

        assert!(manifest.entry_path().is_err());
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
    fn withdrawn_permission_string_parses_gracefully_not_panicked() {
        // A stale manifest may still carry a withdrawn permission name
        // (http-routes / gateway-rpc — capabilities now withdrawn).
        // from_str must not panic; the Custom catch-all keeps them parseable.
        assert_eq!(
            PluginPermission::from_str("http-routes"),
            PluginPermission::Custom("http-routes".to_string())
        );
        assert_eq!(
            PluginPermission::from_str("gateway-rpc"),
            PluginPermission::Custom("gateway-rpc".to_string())
        );
        // A known permission still parses to its real variant.
        assert_eq!(
            PluginPermission::from_str("network"),
            PluginPermission::Network
        );
    }

    #[test]
    fn test_plugin_permission_serde() {
        // Serialize
        let perms = vec![
            PluginPermission::Network,
            PluginPermission::Filesystem(FilesystemAccess::Read),
            PluginPermission::Custom("my:perm".to_string()),
        ];
        let json = serde_json::to_string(&perms).unwrap();

        // Deserialize
        let parsed: Vec<PluginPermission> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], PluginPermission::Network);
        assert_eq!(
            parsed[1],
            PluginPermission::Filesystem(FilesystemAccess::Read)
        );
        assert_eq!(parsed[2], PluginPermission::Custom("my:perm".to_string()));
    }
}
