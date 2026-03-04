//! Agent Definition configuration types
//!
//! Defines the `[agents]` section of the configuration, which declares
//! the set of available agents and their global defaults.
//!
//! - `AgentsConfig`: Top-level `[agents]` section
//! - `AgentDefaults`: Global defaults inherited by all agents
//! - `AgentDefinition`: A single agent declaration
//! - `SubagentPolicy`: Controls which sub-agents an agent may spawn

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// AgentsConfig
// =============================================================================

/// Top-level `[agents]` configuration section
///
/// Contains global defaults and the list of agent definitions.
/// If `list` is empty after deserialization, call `ensure_default()` to
/// guarantee at least a "main" agent exists.
///
/// # Example TOML
/// ```toml
/// [agents.defaults]
/// model = "claude-sonnet-4"
///
/// [[agents.list]]
/// id = "main"
/// default = true
/// name = "Main Agent"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentsConfig {
    /// Global defaults inherited by all agents unless overridden
    #[serde(default)]
    pub defaults: AgentDefaults,

    /// List of agent definitions
    #[serde(default)]
    pub list: Vec<AgentDefinition>,
}

impl AgentsConfig {
    /// Ensure at least one default "main" agent exists.
    ///
    /// If `list` is empty, inserts a minimal agent with `id = "main"` and
    /// `default = true`. If agents are already present, this is a no-op.
    pub fn ensure_default(&mut self) {
        if self.list.is_empty() {
            self.list.push(AgentDefinition {
                id: "main".to_string(),
                default: true,
                name: Some("Main Agent".to_string()),
                ..Default::default()
            });
        }
    }
}

// =============================================================================
// AgentDefaults
// =============================================================================

/// Global agent defaults
///
/// Values here are inherited by every `AgentDefinition` unless that agent
/// overrides them explicitly.
///
/// # Example TOML
/// ```toml
/// [agents.defaults]
/// model = "claude-sonnet-4"
/// workspace_root = "~/workspaces"
/// skills = ["search", "code_review"]
/// dm_scope = "workspace"
/// bootstrap_max_chars = 8000
/// bootstrap_total_max_chars = 32000
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentDefaults {
    /// Default AI model for all agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Default workspace root directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,

    /// Default skills available to all agents
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,

    /// Default DM (domain model) scope
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dm_scope: Option<String>,

    /// Maximum characters for a single bootstrap file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_max_chars: Option<usize>,

    /// Maximum total characters across all bootstrap files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_total_max_chars: Option<usize>,
}

// =============================================================================
// AgentDefinition
// =============================================================================

/// A single agent definition
///
/// Each agent has a unique `id` and optional overrides for model, skills,
/// workspace, profile, and sub-agent policies.
///
/// # Example TOML
/// ```toml
/// [[agents.list]]
/// id = "coder"
/// name = "Coding Agent"
/// workspace = "~/projects"
/// profile = "coding"
/// model = "claude-opus-4"
/// skills = ["git_*", "fs_*"]
///
/// [agents.list.subagents]
/// allow = ["reviewer", "tester"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AgentDefinition {
    /// Unique agent identifier
    #[serde(default)]
    pub id: String,

    /// Whether this is the default agent (at most one should be true)
    #[serde(default)]
    pub default: bool,

    /// Human-readable display name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Workspace directory for this agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,

    /// Profile to inherit from (references a `[profiles.<name>]` entry)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

    /// AI model override for this agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Skills available to this agent (overrides defaults)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,

    /// Sub-agent spawning policy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<SubagentPolicy>,
}

// =============================================================================
// SubagentPolicy
// =============================================================================

/// Sub-agent spawning policy
///
/// Controls which sub-agents an agent is allowed to spawn.
/// Use `["*"]` to allow all agents, or list specific agent IDs.
///
/// # Example TOML
/// ```toml
/// [agents.list.subagents]
/// allow = ["reviewer", "tester"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct SubagentPolicy {
    /// List of allowed sub-agent IDs, or `["*"]` for unrestricted
    #[serde(default)]
    pub allow: Vec<String>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agents_config_deserialize_full() {
        let toml_str = r#"
            [defaults]
            model = "claude-sonnet-4"
            workspace_root = "/home/user/workspaces"
            skills = ["search", "code_review"]
            dm_scope = "workspace"
            bootstrap_max_chars = 8000
            bootstrap_total_max_chars = 32000

            [[list]]
            id = "main"
            default = true
            name = "Main Agent"
            model = "claude-opus-4"
            skills = ["git_*", "fs_*"]

            [[list]]
            id = "reviewer"
            name = "Code Reviewer"
            profile = "coding"
            workspace = "/home/user/reviews"

            [list.subagents]
            allow = ["tester"]
        "#;

        let config: AgentsConfig = toml::from_str(toml_str).unwrap();

        // Verify defaults
        assert_eq!(config.defaults.model, Some("claude-sonnet-4".to_string()));
        assert_eq!(
            config.defaults.workspace_root,
            Some(PathBuf::from("/home/user/workspaces"))
        );
        assert_eq!(
            config.defaults.skills,
            Some(vec!["search".to_string(), "code_review".to_string()])
        );
        assert_eq!(
            config.defaults.dm_scope,
            Some("workspace".to_string())
        );
        assert_eq!(config.defaults.bootstrap_max_chars, Some(8000));
        assert_eq!(config.defaults.bootstrap_total_max_chars, Some(32000));

        // Verify agent list
        assert_eq!(config.list.len(), 2);

        // First agent
        let main = &config.list[0];
        assert_eq!(main.id, "main");
        assert!(main.default);
        assert_eq!(main.name, Some("Main Agent".to_string()));
        assert_eq!(main.model, Some("claude-opus-4".to_string()));
        assert_eq!(
            main.skills,
            Some(vec!["git_*".to_string(), "fs_*".to_string()])
        );
        assert!(main.subagents.is_none());

        // Second agent
        let reviewer = &config.list[1];
        assert_eq!(reviewer.id, "reviewer");
        assert!(!reviewer.default);
        assert_eq!(reviewer.name, Some("Code Reviewer".to_string()));
        assert_eq!(reviewer.profile, Some("coding".to_string()));
        assert_eq!(
            reviewer.workspace,
            Some(PathBuf::from("/home/user/reviews"))
        );
        let subagents = reviewer.subagents.as_ref().unwrap();
        assert_eq!(subagents.allow, vec!["tester"]);
    }

    #[test]
    fn test_agents_config_empty_deserialize() {
        let toml_str = "";
        let config: AgentsConfig = toml::from_str(toml_str).unwrap();

        // Defaults should all be None/empty
        assert!(config.defaults.model.is_none());
        assert!(config.defaults.workspace_root.is_none());
        assert!(config.defaults.skills.is_none());
        assert!(config.defaults.dm_scope.is_none());
        assert!(config.defaults.bootstrap_max_chars.is_none());
        assert!(config.defaults.bootstrap_total_max_chars.is_none());

        // List should be empty
        assert!(config.list.is_empty());
    }

    #[test]
    fn test_ensure_default_when_empty() {
        let mut config = AgentsConfig::default();
        assert!(config.list.is_empty());

        config.ensure_default();

        assert_eq!(config.list.len(), 1);
        let agent = &config.list[0];
        assert_eq!(agent.id, "main");
        assert!(agent.default);
        assert_eq!(agent.name, Some("Main Agent".to_string()));
    }

    #[test]
    fn test_ensure_default_noop_when_populated() {
        let mut config = AgentsConfig {
            list: vec![AgentDefinition {
                id: "custom".to_string(),
                name: Some("Custom Agent".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        config.ensure_default();

        // Should not add another agent
        assert_eq!(config.list.len(), 1);
        assert_eq!(config.list[0].id, "custom");
    }

    #[test]
    fn test_subagent_policy_wildcard() {
        let toml_str = r#"
            allow = ["*"]
        "#;

        let policy: SubagentPolicy = toml::from_str(toml_str).unwrap();
        assert_eq!(policy.allow, vec!["*"]);
    }
}
