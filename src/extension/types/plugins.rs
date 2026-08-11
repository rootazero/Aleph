//! Plugin types
//!
//! Core data structures for plugin management, discovery, and lifecycle.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// =============================================================================
// Plugin Types
// =============================================================================

/// Plugin info for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub path: String,
    pub skills_count: usize,
    pub commands_count: usize,
    pub agents_count: usize,
    pub hooks_count: usize,
    pub mcp_servers_count: usize,
    /// Tools this plugin registers. Load-bearing beyond display: zero means
    /// tool-call accounting has nothing to observe for this plugin, so its
    /// usage must render as `—` rather than `0`.
    #[serde(default)]
    pub tools_count: usize,
    /// Runtime status label: "loaded" | "disabled" | "overridden" | "error".
    #[serde(default)]
    pub status: String,
    /// Error message when the plugin failed to load (status == "error").
    #[serde(default)]
    pub error: Option<String>,
}

/// Plugin origin - where the plugin was discovered from
///
/// Higher priority origins override lower priority ones when plugins
/// have the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginOrigin {
    /// From explicit config (highest priority)
    Config,
    /// From workspace .aleph/ directory
    Workspace,
    /// From global ~/.aleph/ directory
    Global,
    /// Bundled with core (lowest priority)
    Bundled,
}

impl PluginOrigin {
    /// Get the priority of this origin (higher = takes precedence)
    #[must_use]
    pub const fn priority(&self) -> u8 {
        match self {
            Self::Config => 4,
            Self::Workspace => 3,
            Self::Global => 2,
            Self::Bundled => 1,
        }
    }
}

/// Plugin kind - the type/format of the plugin
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// WebAssembly plugin (.wasm)
    Wasm,
    /// MCP server plugin (.mcp.json) — uses Aleph's MCP client system
    Mcp,
    /// Static content plugin (markdown files)
    Static,
}

impl PluginKind {
    /// Detect plugin kind from a file path
    ///
    /// Returns `Some(kind)` if the path indicates a known plugin type,
    /// `None` otherwise.
    #[must_use]
    pub fn detect_from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str());

        match (filename, ext) {
            (_, Some("wasm")) => Some(Self::Wasm),
            (".mcp.json", _) => Some(Self::Mcp),
            ("aleph.plugin.json", _) => Some(Self::Wasm),
            ("SKILL.md" | "COMMAND.md" | "AGENT.md", _) => Some(Self::Static),
            (_, Some("md")) => Some(Self::Static),
            _ => None,
        }
    }
}

/// Plugin status - the runtime state of a plugin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    /// Plugin is loaded and active
    Loaded,
    /// Plugin is disabled by user
    Disabled,
    /// Plugin is overridden by a higher-priority plugin with the same name
    Overridden,
    /// Plugin failed to load with an error
    Error(String),
}

impl PluginStatus {
    /// Check if this plugin is actively running
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Loaded)
    }

    /// Stable lowercase label for client display / serialization.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Disabled => "disabled",
            Self::Overridden => "overridden",
            Self::Error(_) => "error",
        }
    }
}

// =============================================================================
// Load Summary
// =============================================================================

/// Summary of extension loading returned by `ExtensionManager::load_all()`
#[derive(Debug, Default)]
pub struct LoadSummary {
    /// Number of skills loaded
    pub skills_loaded: usize,
    /// Number of commands loaded
    pub commands_loaded: usize,
    /// Number of agents loaded
    pub agents_loaded: usize,
    /// Number of plugins loaded
    pub plugins_loaded: usize,
    /// Number of hooks loaded
    pub hooks_loaded: usize,
    /// Number of plugins the owner trust policy refused to load. Surfaced in
    /// `extensions.stat` so operators can spot a plugin that's installed but
    /// not active because it wasn't on the allowlist.
    pub skipped_by_trust: usize,
    /// Errors encountered during loading
    pub errors: Vec<String>,
}

impl LoadSummary {
    /// Check if loading was successful (no errors)
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total components loaded
    #[must_use]
    pub const fn total_loaded(&self) -> usize {
        self.skills_loaded + self.commands_loaded + self.agents_loaded + self.plugins_loaded
    }
}

// =============================================================================
// Plugin Record
// =============================================================================

/// Installation scope for plugins.
///
/// Determines where a plugin is installed and its resolution priority
/// when multiple scopes define the same plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    /// Installed for the current user (~/.aleph/plugins/)
    User,
    /// Installed for the current project (.aleph/plugins/)
    Project,
    /// Installed locally for development (symlinked or path-based)
    Local,
}

impl PluginScope {
    /// Get the resolution priority of this scope (higher = takes precedence).
    #[must_use]
    pub const fn priority(&self) -> u8 {
        match self {
            Self::Local => 3,
            Self::Project => 2,
            Self::User => 1,
        }
    }
}

impl std::fmt::Display for PluginScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Plugin record - comprehensive plugin information for registry tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    /// Unique plugin identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Version string (semver)
    pub version: Option<String>,
    /// Plugin description
    pub description: Option<String>,
    /// Plugin type/format
    pub kind: PluginKind,
    /// Discovery origin
    pub origin: PluginOrigin,
    /// Current status
    pub status: PluginStatus,
    /// Error message if status is Error
    pub error: Option<String>,
    /// Root directory of the plugin
    pub root_dir: PathBuf,
    // Registration tracking
    /// Tool names registered by this plugin
    pub tool_names: Vec<String>,
    /// Number of hooks registered
    pub hook_count: usize,
    /// Service IDs registered by this plugin
    pub service_ids: Vec<String>,
    /// Number of skills declared by this plugin
    #[serde(default)]
    pub skill_count: usize,
    /// Number of in-chat commands declared by this plugin
    #[serde(default)]
    pub command_count: usize,
    /// Number of agents declared by this plugin
    #[serde(default)]
    pub agent_count: usize,
    /// Number of MCP servers declared by this plugin
    #[serde(default)]
    pub mcp_server_count: usize,
}

impl PluginRecord {
    /// Create a new plugin record with default values
    #[must_use]
    pub const fn new(id: String, name: String, kind: PluginKind, origin: PluginOrigin) -> Self {
        Self {
            id,
            name,
            version: None,
            description: None,
            kind,
            origin,
            status: PluginStatus::Loaded,
            error: None,
            root_dir: PathBuf::new(),
            tool_names: Vec::new(),
            hook_count: 0,
            service_ids: Vec::new(),
            skill_count: 0,
            command_count: 0,
            agent_count: 0,
            mcp_server_count: 0,
        }
    }

    /// Create a `PluginRecord` from an `AdapterOutput`.
    ///
    /// Populates metadata from the adapter output and derives tool/hook counts
    /// from the declared capabilities.
    #[must_use]
    pub fn from_adapter_output(
        output: &crate::extension::manifest::adapter::AdapterOutput,
        root_dir: PathBuf,
    ) -> Self {
        use crate::extension::capability::CapabilityDeclaration;

        let mut tool_names = Vec::new();
        let mut hook_count = 0;
        let mut service_ids = Vec::new();
        let mut skill_count = 0;
        let mut command_count = 0;
        let mut agent_count = 0;
        let mut mcp_server_count = 0;

        for cap in &output.capabilities {
            match cap {
                CapabilityDeclaration::Tool(t) => tool_names.push(t.name.clone()),
                CapabilityDeclaration::Hook(_) => hook_count += 1,
                CapabilityDeclaration::Service(s) => service_ids.push(s.id.clone()),
                // Skills and `commands/`-derived commands share the Skill
                // variant, split by `skill_type`.
                CapabilityDeclaration::Skill(s) => {
                    if s.skill_type == crate::extension::types::SkillType::Command {
                        command_count += 1;
                    } else {
                        skill_count += 1;
                    }
                }
                CapabilityDeclaration::Agent(_) => agent_count += 1,
                CapabilityDeclaration::McpServer(_) => mcp_server_count += 1,
            }
        }

        Self {
            id: output.plugin_id.clone(),
            name: output
                .name
                .clone()
                .unwrap_or_else(|| output.plugin_id.clone()),
            version: output.version.clone(),
            description: output.description.clone(),
            kind: PluginKind::Static, // default; caller can override
            origin: output.source.origin,
            status: PluginStatus::Loaded,
            error: None,
            root_dir,
            tool_names,
            hook_count,
            service_ids,
            skill_count,
            command_count,
            agent_count,
            mcp_server_count,
        }
    }

    /// Set an error status with message
    #[must_use]
    pub fn with_error(mut self, error: String) -> Self {
        self.status = PluginStatus::Error(error.clone());
        self.error = Some(error);
        self
    }

    /// Set the root directory
    #[must_use]
    pub fn with_root_dir(mut self, path: PathBuf) -> Self {
        self.root_dir = path;
        self
    }
}
