//! Agent configuration types
//!
//! Contains Agent task orchestration configuration:
//! - `CoworkConfigToml`: Main configuration for the Agent engine
//! - `FileOpsConfigToml`: File operations executor configuration
//! - `CodeExecConfigToml`: Code execution executor configuration
//!
//! # Retired: `[agent.subagents]`, `require_confirmation`, `max_parallelism`
//!
//! This section once carried a spawn allowlist (`subagents.allow_agents`), a
//! session cleanup policy, a spawn timeout, a parallelism cap and a
//! confirmation switch. None of them had an implementation: they were
//! validated at boot, published into the Panel schema, and then read by
//! nobody — a decorative surface that looks like control. `max_parallelism =
//! 0` could even *block boot* over a value that was self-declared "legacy,
//! ignored".
//!
//! Sub-agent capability boundaries are owned by three real layers:
//! `AgentDefinition.subagents` (`config/types/agents_def.rs::SubagentPolicy`,
//! consumed by `config/agent_resolver`), `AgentDef.allowed_tools` +
//! `AllowlistToolService`, and the exec tier gate in `src/tools/scoped/`. A
//! fourth config only manufactures a second source of truth.
//!
//! Config deserialization does not `deny_unknown_fields`, so an existing
//! `config.toml` still carrying these keys keeps parsing — with the same
//! (zero) effect it always had. Before adding any of them back, wire the
//! enforcement point in the same commit.

mod code_exec;
mod file_ops;

// Re-export all public types
pub use code_exec::CodeExecConfigToml;
pub use file_ops::FileOpsConfigToml;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// CoworkConfigToml
// =============================================================================

/// Agent task orchestration configuration
///
/// Configures the Agent engine for multi-task orchestration.
///
/// # Example TOML
/// ```toml
/// [agent]
/// planner_provider = "claude"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CoworkConfigToml {
    /// AI provider to use for task planning (LLM decomposition)
    /// If not specified, uses the default provider from [general]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_provider: Option<String>,

    /// File operations configuration
    #[serde(default)]
    pub file_ops: FileOpsConfigToml,

    /// Code execution configuration
    #[serde(default)]
    pub code_exec: CodeExecConfigToml,
}

// =============================================================================
// CoworkConfigToml Implementation
// =============================================================================

impl CoworkConfigToml {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate file_ops configuration
        self.file_ops.validate()?;

        // Validate code_exec configuration
        self.code_exec.validate()?;

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CoworkConfigToml::default();
        assert!(config.planner_provider.is_none());
    }

    #[test]
    fn test_validation() {
        let config = CoworkConfigToml::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_agent_config_includes_file_ops() {
        let config = CoworkConfigToml::default();
        assert!(config.file_ops.enabled);
        assert!(config.file_ops.require_confirmation_for_write);
    }

    /// Retired keys must not become a hard parse failure for existing
    /// `config.toml` files. `Config` does not `deny_unknown_fields`, and this
    /// test pins that: turning it on later would flip "inert key" into
    /// "server will not start", which is strictly worse than the bug that
    /// motivated the removal.
    #[test]
    fn retired_keys_still_parse_and_are_ignored() {
        let toml_str = r#"
require_confirmation = false
max_parallelism = 0
planner_provider = "claude"

[subagents]
allow_agents = ["translator"]
default_cleanup = "persistent"
default_timeout_seconds = 0
"#;
        let config: CoworkConfigToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.planner_provider.as_deref(), Some("claude"));
        // `max_parallelism = 0` used to abort boot here.
        assert!(config.validate().is_ok());
    }
}
