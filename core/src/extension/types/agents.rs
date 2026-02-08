//! Agent-related types for the extension system
//!
//! This module contains types for defining and configuring extension agents,
//! including agent modes, permission rules, and frontmatter parsing.

use crate::discovery::DiscoverySource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// =============================================================================
// Agent Types
// =============================================================================

/// Agent mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Primary agent (top-level, can be selected by user)
    Primary,
    /// Sub-agent (delegated to by primary agents)
    Subagent,
    /// Both primary and sub-agent
    #[default]
    All,
}

/// Permission rule for agent
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PermissionRule {
    /// Simple action for all patterns
    Simple(PermissionAction),
    /// Pattern-based rules
    Patterns(HashMap<String, PermissionAction>),
}

/// Permission action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

/// Extension agent definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionAgent {
    /// Agent name
    pub name: String,

    /// Plugin name (if from a plugin)
    pub plugin_name: Option<String>,

    /// Agent mode
    #[serde(default)]
    pub mode: AgentMode,

    /// Description
    #[serde(default)]
    pub description: Option<String>,

    /// Whether to hide from UI
    #[serde(default)]
    pub hidden: bool,

    /// UI color (hex format)
    #[serde(default)]
    pub color: Option<String>,

    /// Model specification (provider/model)
    #[serde(default)]
    pub model: Option<String>,

    /// Temperature
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Top P
    #[serde(default)]
    pub top_p: Option<f32>,

    /// Maximum iteration steps
    #[serde(default)]
    pub steps: Option<u32>,

    /// Tool permissions
    #[serde(default)]
    pub tools: Option<HashMap<String, bool>>,

    /// Permission rules
    #[serde(default)]
    pub permission: Option<HashMap<String, PermissionRule>>,

    /// Provider-specific options
    #[serde(default)]
    pub options: HashMap<String, serde_json::Value>,

    /// System prompt (markdown body)
    pub system_prompt: String,

    /// Source path
    pub source_path: PathBuf,

    /// Discovery source
    pub source: DiscoverySource,
}

impl ExtensionAgent {
    /// Get the fully qualified name
    pub fn qualified_name(&self) -> String {
        match &self.plugin_name {
            Some(plugin) => format!("{}:{}", plugin, self.name),
            None => self.name.clone(),
        }
    }

    /// Check if agent is a primary agent
    pub fn is_primary(&self) -> bool {
        matches!(self.mode, AgentMode::Primary | AgentMode::All)
    }

    /// Check if agent can be used as a sub-agent
    pub fn is_subagent(&self) -> bool {
        matches!(self.mode, AgentMode::Subagent | AgentMode::All)
    }
}

// =============================================================================
// Frontmatter Types
// =============================================================================

/// Agent frontmatter
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFrontmatter {
    #[serde(default)]
    pub mode: Option<AgentMode>,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub hidden: Option<bool>,

    #[serde(default)]
    pub color: Option<String>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub temperature: Option<f32>,

    #[serde(default)]
    pub top_p: Option<f32>,

    #[serde(default)]
    pub steps: Option<u32>,

    #[serde(default)]
    pub tools: Option<HashMap<String, bool>>,

    #[serde(default)]
    pub permission: Option<HashMap<String, PermissionRule>>,

    #[serde(default)]
    pub options: Option<HashMap<String, serde_json::Value>>,
}
