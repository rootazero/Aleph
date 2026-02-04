//! Configuration types for command execution security.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Security level for command execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecSecurity {
    /// Reject all command execution
    #[default]
    Deny,
    /// Only allow whitelisted commands
    Allowlist,
    /// Allow all commands (full trust)
    Full,
}

/// Ask policy for commands not in allowlist
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecAsk {
    /// Never ask user (use fallback)
    Off,
    /// Ask when command not in allowlist (default)
    #[default]
    OnMiss,
    /// Ask for every execution
    Always,
}

/// Root configuration file for exec approvals
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalsFile {
    /// Config version (currently 1)
    #[serde(default = "default_version")]
    pub version: u8,

    /// Socket configuration for UI communication
    #[serde(default)]
    pub socket: Option<SocketConfig>,

    /// Default settings for all agents
    #[serde(default)]
    pub defaults: Option<ExecDefaults>,

    /// Per-agent configuration overrides
    #[serde(default)]
    pub agents: HashMap<String, AgentExecConfig>,
}

fn default_version() -> u8 {
    1
}

impl Default for ExecApprovalsFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            socket: None,
            defaults: None,
            agents: HashMap::new(),
        }
    }
}

/// Socket configuration for approval communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketConfig {
    /// Socket path (default: ~/.aleph/exec-approvals.sock)
    #[serde(default)]
    pub path: Option<String>,

    /// Authentication token
    #[serde(default)]
    pub token: Option<String>,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            path: Some("~/.aleph/exec-approvals.sock".into()),
            token: None,
        }
    }
}

/// Default execution settings
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecDefaults {
    /// Security level
    #[serde(default)]
    pub security: Option<ExecSecurity>,

    /// Ask policy
    #[serde(default)]
    pub ask: Option<ExecAsk>,

    /// Fallback when ask is off or times out
    #[serde(default)]
    pub ask_fallback: Option<ExecSecurity>,

    /// Auto-allow commands from skills
    #[serde(default)]
    pub auto_allow_skills: Option<bool>,
}

/// Per-agent execution configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentExecConfig {
    /// Agent-specific defaults (inherits from global defaults)
    #[serde(flatten)]
    pub defaults: ExecDefaults,

    /// Command allowlist for this agent
    #[serde(default)]
    pub allowlist: Option<Vec<AllowlistEntry>>,

    /// Skills that are pre-approved for CLI execution
    #[serde(default)]
    pub skill_allowlist: Option<Vec<String>>,
}

/// An entry in the command allowlist
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    /// Unique identifier
    #[serde(default)]
    pub id: Option<String>,

    /// Pattern to match (e.g., "/usr/bin/git", "~/bin/*", "git")
    pub pattern: String,

    /// Last time this entry was used (Unix timestamp)
    #[serde(default)]
    pub last_used_at: Option<i64>,

    /// Last command that matched this entry
    #[serde(default)]
    pub last_used_command: Option<String>,

    /// Last resolved path for this entry
    #[serde(default)]
    pub last_resolved_path: Option<String>,
}

impl ExecApprovalsFile {
    /// Get resolved config for an agent
    pub fn resolve_for_agent(&self, agent_id: &str) -> ResolvedExecConfig {
        let global = self.defaults.as_ref();
        let agent = self.agents.get(agent_id);

        let security = agent
            .and_then(|a| a.defaults.security)
            .or_else(|| global.and_then(|g| g.security))
            .unwrap_or_default();

        let ask = agent
            .and_then(|a| a.defaults.ask)
            .or_else(|| global.and_then(|g| g.ask))
            .unwrap_or_default();

        let ask_fallback = agent
            .and_then(|a| a.defaults.ask_fallback)
            .or_else(|| global.and_then(|g| g.ask_fallback))
            .unwrap_or(ExecSecurity::Deny);

        let auto_allow_skills = agent
            .and_then(|a| a.defaults.auto_allow_skills)
            .or_else(|| global.and_then(|g| g.auto_allow_skills))
            .unwrap_or(false);

        let allowlist = agent
            .and_then(|a| a.allowlist.clone())
            .unwrap_or_default();

        let skill_allowlist = agent
            .and_then(|a| a.skill_allowlist.clone())
            .unwrap_or_default();

        ResolvedExecConfig {
            security,
            ask,
            ask_fallback,
            auto_allow_skills,
            allowlist,
            skill_allowlist,
        }
    }
}

/// Resolved execution configuration with all defaults applied
#[derive(Debug, Clone)]
pub struct ResolvedExecConfig {
    pub security: ExecSecurity,
    pub ask: ExecAsk,
    pub ask_fallback: ExecSecurity,
    pub auto_allow_skills: bool,
    pub allowlist: Vec<AllowlistEntry>,
    /// Skills that are pre-approved for CLI execution
    pub skill_allowlist: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_security_default() {
        assert_eq!(ExecSecurity::default(), ExecSecurity::Deny);
    }

    #[test]
    fn test_exec_ask_default() {
        assert_eq!(ExecAsk::default(), ExecAsk::OnMiss);
    }

    #[test]
    fn test_config_deserialize() {
        let toml_str = r#"
            version = 1

            [defaults]
            security = "allowlist"
            ask = "on-miss"

            [agents.main]
            security = "full"

            [[agents.main.allowlist]]
            pattern = "/usr/bin/git"
        "#;

        let config: ExecApprovalsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.version, 1);
        assert!(config.agents.contains_key("main"));
    }

    #[test]
    fn test_resolve_for_agent() {
        let mut config = ExecApprovalsFile::default();
        config.defaults = Some(ExecDefaults {
            security: Some(ExecSecurity::Allowlist),
            ask: Some(ExecAsk::OnMiss),
            ..Default::default()
        });

        let mut agent_config = AgentExecConfig::default();
        agent_config.defaults.security = Some(ExecSecurity::Full);
        config.agents.insert("work".to_string(), agent_config);

        // Global defaults
        let main_resolved = config.resolve_for_agent("main");
        assert_eq!(main_resolved.security, ExecSecurity::Allowlist);

        // Agent override
        let work_resolved = config.resolve_for_agent("work");
        assert_eq!(work_resolved.security, ExecSecurity::Full);
    }

    #[test]
    fn test_exec_config_with_skill_allowlist() {
        let toml_str = r#"
            version = 1

            [agents.main]
            security = "allowlist"
            skill_allowlist = ["github", "ffmpeg"]
        "#;

        let config: ExecApprovalsFile = toml::from_str(toml_str).unwrap();
        let resolved = config.resolve_for_agent("main");

        assert_eq!(
            resolved.skill_allowlist,
            vec!["github".to_string(), "ffmpeg".to_string()]
        );
    }
}
