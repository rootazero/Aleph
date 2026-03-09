//! A2A protocol configuration types
//!
//! Defines the `[a2a]` section in `aleph.toml` for controlling
//! A2A server exposure, security, and pre-registered remote agents.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Top-level A2A protocol configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct A2AConfig {
    /// Enable A2A protocol support
    #[serde(default)]
    pub enabled: bool,

    /// A2A Server configuration
    #[serde(default)]
    pub server: A2AServerConfig,

    /// Pre-registered remote A2A agents
    #[serde(default)]
    pub agents: Vec<A2AAgentEntry>,
}

/// A2A server endpoint configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct A2AServerConfig {
    /// Enable A2A server endpoints
    #[serde(default)]
    pub enabled: bool,

    /// Agent Card name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_name: Option<String>,

    /// Agent Card description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_description: Option<String>,

    /// Agent Card version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_version: Option<String>,

    /// Security configuration
    #[serde(default)]
    pub security: A2ASecurityConfig,

    /// Manually defined skills (auto-generated from ToolRegistry if empty)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<A2ASkillConfig>,
}

/// A2A security settings
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct A2ASecurityConfig {
    /// Allow unauthenticated access from localhost
    #[serde(default = "default_true")]
    pub local_bypass: bool,

    /// API tokens for trusted-level authentication
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// A pre-registered remote A2A agent entry
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct A2AAgentEntry {
    /// Display name for the agent
    pub name: String,

    /// A2A endpoint URL
    pub url: String,

    /// Trust level override (auto-inferred from URL if not set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_level: Option<String>,

    /// Authentication token for this agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// A manually defined A2A skill
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct A2ASkillConfig {
    /// Skill identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
