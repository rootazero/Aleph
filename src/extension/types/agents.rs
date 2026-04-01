//! Agent-related types for the extension system
//!
//! This module contains types for defining and configuring extension agents,
//! including agent modes, permission rules, and frontmatter parsing.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Extension agent definition (unified with AgentRegistration).
///
/// This is now a type alias for `AgentRegistration`, which contains all fields
/// needed for both plugin-registered and filesystem-discovered agents.
pub type ExtensionAgent = crate::extension::AgentRegistration;

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
