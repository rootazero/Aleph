//! Agent configuration types
//!
//! Contains Agent task orchestration configuration:
//! - CoworkConfigToml: Main configuration for the Agent engine
//! - FileOpsConfigToml: File operations executor configuration
//! - CodeExecConfigToml: Code execution executor configuration

mod code_exec;
mod file_ops;
mod subagents;

// Re-export all public types
pub use code_exec::CodeExecConfigToml;
pub use file_ops::FileOpsConfigToml;
pub use subagents::SubagentsConfigToml;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tool_metadata::{MAX_PARALLELISM, REQUIRE_CONFIRMATION};

// =============================================================================
// CoworkConfigToml
// =============================================================================

/// Agent task orchestration configuration
///
/// Configures the Agent engine for multi-task orchestration.
/// This includes task decomposition, parallel execution, and confirmation settings.
///
/// # Example TOML
/// ```toml
/// [agent]
/// planner_provider = "claude"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CoworkConfigToml {
    /// Require user confirmation before executing task graphs
    /// (Legacy field, ignored - confirmation is always required)
    #[serde(default = "default_require_confirmation", skip_serializing)]
    #[schemars(skip)]
    pub require_confirmation: bool,

    /// Maximum number of tasks to run in parallel
    /// (Legacy field, ignored - uses hardcoded value for stability)
    #[serde(default = "default_max_parallelism", skip_serializing)]
    #[schemars(skip)]
    pub max_parallelism: usize,

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

    /// Sub-agent orchestration configuration
    ///
    /// Controls which agents can be spawned and default spawn settings.
    #[serde(default)]
    pub subagents: SubagentsConfigToml,
}

// =============================================================================
// Default Functions
// =============================================================================

pub const fn default_require_confirmation() -> bool {
    REQUIRE_CONFIRMATION
}

pub const fn default_max_parallelism() -> usize {
    MAX_PARALLELISM
}

// =============================================================================
// Default Implementation
// =============================================================================

impl Default for CoworkConfigToml {
    fn default() -> Self {
        Self {
            require_confirmation: default_require_confirmation(),
            max_parallelism: default_max_parallelism(),
            planner_provider: None,
            file_ops: FileOpsConfigToml::default(),
            code_exec: CodeExecConfigToml::default(),
            subagents: SubagentsConfigToml::default(),
        }
    }
}

// =============================================================================
// CoworkConfigToml Implementation
// =============================================================================

impl CoworkConfigToml {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate max_parallelism
        if self.max_parallelism == 0 {
            return Err("agent.max_parallelism must be greater than 0".to_string());
        }
        if self.max_parallelism > 32 {
            tracing::warn!(
                max_parallelism = self.max_parallelism,
                "agent.max_parallelism is very high (>32), this may cause resource issues"
            );
        }

        // Validate file_ops configuration
        self.file_ops.validate()?;

        // Validate code_exec configuration
        self.code_exec.validate()?;

        // Validate subagents configuration
        self.subagents.validate()?;

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
        // Legacy fields still have defaults for TOML compatibility
        assert!(config.require_confirmation);
        assert_eq!(config.max_parallelism, 4);
        assert!(config.planner_provider.is_none());
    }

    #[test]
    fn test_validation() {
        let mut config = CoworkConfigToml::default();

        // Valid config should pass
        assert!(config.validate().is_ok());

        // Invalid max_parallelism
        config.max_parallelism = 0;
        assert!(config.validate().is_err());
        config.max_parallelism = 4;
    }

    #[test]
    fn test_agent_config_includes_file_ops() {
        let config = CoworkConfigToml::default();
        assert!(config.file_ops.enabled);
        assert!(config.file_ops.require_confirmation_for_write);
    }
}
