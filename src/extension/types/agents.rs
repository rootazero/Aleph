//! Agent-related types for the extension system
//!
//! This module contains types for defining and configuring extension agents,
//! including agent modes and permission actions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

/// Permission action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

/// Extension agent definition (unified with `AgentRegistration`).
///
/// This is now a type alias for `AgentRegistration`, which contains all fields
/// needed for both plugin-registered and filesystem-discovered agents.
pub type ExtensionAgent = crate::extension::AgentRegistration;

