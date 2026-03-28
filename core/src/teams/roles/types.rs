//! Role types and configuration for team agents.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// AgentRole
// ---------------------------------------------------------------------------

/// The role an agent plays within a team.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Leader,
    Explorer,
    Critic,
    Worker,
    Custom(String),
}

impl AgentRole {
    /// Canonical string representation for storage.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Leader => "leader",
            Self::Explorer => "explorer",
            Self::Critic => "critic",
            Self::Worker => "worker",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Reconstruct from a stored string value.
    pub fn from_stored(s: &str) -> Self {
        match s {
            "leader" => Self::Leader,
            "explorer" => Self::Explorer,
            "critic" => Self::Critic,
            "worker" => Self::Worker,
            other => Self::Custom(other.to_string()),
        }
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// Severity level for review challenges.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    Major,
    Minor,
}

// ---------------------------------------------------------------------------
// TeamRoleConfig
// ---------------------------------------------------------------------------

/// Configuration for a role within a team, governing review thresholds
/// and prompt templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamRoleConfig {
    pub role: AgentRole,
    pub prompt_template: String,
    pub review_dimensions: Vec<String>,
    #[serde(default = "default_min_score_threshold")]
    pub min_score_threshold: u8,
    #[serde(default = "default_min_challenges")]
    pub min_challenges: u32,
}

fn default_min_score_threshold() -> u8 {
    7
}

fn default_min_challenges() -> u32 {
    3
}

impl Default for TeamRoleConfig {
    fn default() -> Self {
        Self {
            role: AgentRole::Worker,
            prompt_template: String::new(),
            review_dimensions: vec![
                "correctness".to_string(),
                "completeness".to_string(),
                "clarity".to_string(),
            ],
            min_score_threshold: default_min_score_threshold(),
            min_challenges: default_min_challenges(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_role_roundtrip() {
        let roles = vec![
            AgentRole::Leader,
            AgentRole::Explorer,
            AgentRole::Critic,
            AgentRole::Worker,
            AgentRole::Custom("analyst".into()),
        ];

        for role in roles {
            let s = role.as_str();
            let restored = AgentRole::from_stored(s);
            assert_eq!(role, restored);
        }
    }

    #[test]
    fn test_default_config_thresholds() {
        let config = TeamRoleConfig::default();
        assert_eq!(config.min_score_threshold, 7);
        assert_eq!(config.min_challenges, 3);
    }
}
