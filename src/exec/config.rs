//! Configuration types for command execution security.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Root configuration file for exec approvals
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalsFile {
    /// Config version (currently 1)
    #[serde(default = "default_version")]
    pub version: u8,

    /// Per-agent configuration overrides
    #[serde(default)]
    pub agents: BTreeMap<String, AgentExecConfig>,
}

const fn default_version() -> u8 {
    1
}

impl Default for ExecApprovalsFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            agents: BTreeMap::new(),
        }
    }
}

/// Per-agent execution configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentExecConfig {
    /// Command allowlist for this agent
    #[serde(default)]
    pub allowlist: Option<Vec<AllowlistEntry>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_deserialize() {
        let toml_str = r#"
            version = 1

            [[agents.main.allowlist]]
            pattern = "/usr/bin/git"
        "#;

        let config: ExecApprovalsFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.version, 1);
        let allowlist = config.agents["main"].allowlist.as_ref().unwrap();
        assert_eq!(allowlist[0].pattern, "/usr/bin/git");
    }
}
