//! Plugin types
//!
//! Core data structures for plugin management, discovery, and lifecycle.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{ExtensionAgent, ExtensionCommand, ExtensionSkill, HookConfig, McpServerConfig};

// =============================================================================
// Plugin Types
// =============================================================================

/// Loaded plugin
#[derive(Debug, Clone)]
pub struct ExtensionPlugin {
    /// Plugin name (from manifest)
    pub name: String,

    /// Plugin version
    pub version: Option<String>,

    /// Plugin description
    pub description: Option<String>,

    /// Plugin root path
    pub path: PathBuf,

    /// Whether plugin is enabled
    pub enabled: bool,

    /// Skills provided by this plugin
    pub skills: Vec<ExtensionSkill>,

    /// Commands provided by this plugin
    pub commands: Vec<ExtensionCommand>,

    /// Agents provided by this plugin
    pub agents: Vec<ExtensionAgent>,

    /// Hook configurations
    pub hooks: Vec<HookConfig>,

    /// MCP server configurations
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

impl ExtensionPlugin {
    /// Get plugin info
    pub fn info(&self) -> PluginInfo {
        PluginInfo {
            name: self.name.clone(),
            version: self.version.clone(),
            description: self.description.clone(),
            enabled: self.enabled,
            path: self.path.to_string_lossy().to_string(),
            skills_count: self.skills.len(),
            commands_count: self.commands.len(),
            agents_count: self.agents.len(),
            hooks_count: self.hooks.len(),
            mcp_servers_count: self.mcp_servers.len(),
        }
    }
}

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
    pub fn priority(&self) -> u8 {
        match self {
            PluginOrigin::Config => 4,
            PluginOrigin::Workspace => 3,
            PluginOrigin::Global => 2,
            PluginOrigin::Bundled => 1,
        }
    }
}

/// Plugin kind - the type/format of the plugin
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// WebAssembly plugin (.wasm)
    Wasm,
    /// Node.js plugin (package.json)
    NodeJs,
    /// Static content plugin (markdown files)
    Static,
}

impl PluginKind {
    /// Detect plugin kind from a file path
    ///
    /// Returns `Some(kind)` if the path indicates a known plugin type,
    /// `None` otherwise.
    pub fn detect_from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str());

        match (filename, ext) {
            (_, Some("wasm")) => Some(PluginKind::Wasm),
            ("package.json", _) => Some(PluginKind::NodeJs),
            ("aleph.plugin.json", _) => Some(PluginKind::Wasm),
            ("SKILL.md" | "COMMAND.md" | "AGENT.md", _) => Some(PluginKind::Static),
            (_, Some("md")) => Some(PluginKind::Static),
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
    pub fn is_active(&self) -> bool {
        matches!(self, PluginStatus::Loaded)
    }
}

// =============================================================================
// Load Summary
// =============================================================================

/// Summary of extension loading returned by ComponentLoader::load_all()
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
    /// Errors encountered during loading
    pub errors: Vec<String>,
}

impl LoadSummary {
    /// Check if loading was successful (no errors)
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total components loaded
    pub fn total_loaded(&self) -> usize {
        self.skills_loaded + self.commands_loaded + self.agents_loaded + self.plugins_loaded
    }
}

// =============================================================================
// Plugin Record
// =============================================================================

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
    /// Channel IDs registered by this plugin
    pub channel_ids: Vec<String>,
    /// Provider IDs registered by this plugin
    pub provider_ids: Vec<String>,
    /// Gateway RPC methods registered by this plugin
    pub gateway_methods: Vec<String>,
    /// Service IDs registered by this plugin
    pub service_ids: Vec<String>,
}

impl PluginRecord {
    /// Create a new plugin record with default values
    pub fn new(id: String, name: String, kind: PluginKind, origin: PluginOrigin) -> Self {
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
            channel_ids: Vec::new(),
            provider_ids: Vec::new(),
            gateway_methods: Vec::new(),
            service_ids: Vec::new(),
        }
    }

    /// Set an error status with message
    pub fn with_error(mut self, error: String) -> Self {
        self.status = PluginStatus::Error(error.clone());
        self.error = Some(error);
        self
    }

    /// Set the root directory
    pub fn with_root_dir(mut self, path: PathBuf) -> Self {
        self.root_dir = path;
        self
    }
}
