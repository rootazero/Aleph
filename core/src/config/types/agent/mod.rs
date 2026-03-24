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

use crate::dispatcher::{
    DEFAULT_SANDBOX_ENABLED, MAX_PARALLELISM, MAX_TASK_RETRIES, REQUIRE_CONFIRMATION,
};

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
/// planner_model = "claude"
/// auto_execute_threshold = 0.9
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

    /// Confidence threshold for auto-execution without confirmation
    /// Tasks with confidence >= threshold may bypass confirmation
    /// Range: 0.0 - 1.0 (0.0 = always confirm, 1.0 = never auto-execute)
    #[serde(default = "default_auto_execute_threshold")]
    pub auto_execute_threshold: f32,

    /// Maximum number of tasks allowed in a single graph
    /// Prevents runaway task decomposition
    #[serde(default = "default_max_tasks_per_graph")]
    pub max_tasks_per_graph: usize,

    /// Timeout for individual task execution (seconds)
    /// 0 = no timeout
    #[serde(default = "default_task_timeout_seconds")]
    pub task_timeout_seconds: u64,

    /// Maximum number of retry attempts for failed tasks
    /// Range: 1-10 (default: 3)
    #[serde(default = "default_max_task_retries")]
    pub max_task_retries: u32,

    /// Enable sandboxed execution for code tasks
    /// When true, code execution tasks run in isolated environment
    #[serde(default = "default_sandbox_enabled")]
    pub sandbox_enabled: bool,

    /// Categories of tasks that are allowed
    /// Empty list = all categories allowed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_categories: Vec<String>,

    /// Categories of tasks that are blocked
    /// Takes precedence over allowed_categories
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_categories: Vec<String>,

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

pub fn default_require_confirmation() -> bool {
    REQUIRE_CONFIRMATION
}

pub fn default_max_parallelism() -> usize {
    MAX_PARALLELISM
}

pub fn default_auto_execute_threshold() -> f32 {
    0.95 // Very high confidence required for auto-execution
}

pub fn default_max_tasks_per_graph() -> usize {
    20
}

pub fn default_task_timeout_seconds() -> u64 {
    300 // 5 minutes default
}

pub fn default_max_task_retries() -> u32 {
    MAX_TASK_RETRIES
}

pub fn default_sandbox_enabled() -> bool {
    DEFAULT_SANDBOX_ENABLED
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
            auto_execute_threshold: default_auto_execute_threshold(),
            max_tasks_per_graph: default_max_tasks_per_graph(),
            task_timeout_seconds: default_task_timeout_seconds(),
            max_task_retries: default_max_task_retries(),
            sandbox_enabled: default_sandbox_enabled(),
            allowed_categories: Vec::new(),
            blocked_categories: Vec::new(),
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

        // Validate auto_execute_threshold
        if !(0.0..=1.0).contains(&self.auto_execute_threshold) {
            return Err(format!(
                "agent.auto_execute_threshold must be between 0.0 and 1.0, got {}",
                self.auto_execute_threshold
            ));
        }

        // Validate max_tasks_per_graph
        if self.max_tasks_per_graph == 0 {
            return Err("agent.max_tasks_per_graph must be greater than 0".to_string());
        }
        if self.max_tasks_per_graph > 100 {
            tracing::warn!(
                max_tasks = self.max_tasks_per_graph,
                "agent.max_tasks_per_graph is very high (>100), this may indicate a problem"
            );
        }

        // Validate category names
        let valid_categories = [
            "file_operation",
            "code_execution",
            "document_generation",
            "app_automation",
            "ai_inference",
        ];

        for cat in &self.allowed_categories {
            if !valid_categories.contains(&cat.as_str()) {
                return Err(format!(
                    "agent.allowed_categories contains unknown category '{}'. Valid: {:?}",
                    cat, valid_categories
                ));
            }
        }

        for cat in &self.blocked_categories {
            if !valid_categories.contains(&cat.as_str()) {
                return Err(format!(
                    "agent.blocked_categories contains unknown category '{}'. Valid: {:?}",
                    cat, valid_categories
                ));
            }
        }

        // Validate file_ops configuration
        self.file_ops.validate()?;

        // Validate code_exec configuration
        self.code_exec.validate()?;

        // Validate subagents configuration
        self.subagents.validate()?;

        Ok(())
    }

    /// Check if a task category is allowed
    pub fn is_category_allowed(&self, category: &str) -> bool {
        // Blocked categories take precedence
        if self.blocked_categories.contains(&category.to_string()) {
            return false;
        }

        // If allowed_categories is empty, all categories are allowed
        if self.allowed_categories.is_empty() {
            return true;
        }

        // Check if category is in allowed list
        self.allowed_categories.contains(&category.to_string())
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

        // Invalid auto_execute_threshold
        config.auto_execute_threshold = 1.5;
        assert!(config.validate().is_err());
        config.auto_execute_threshold = 0.95;

        // Invalid category
        config.allowed_categories = vec!["invalid_category".to_string()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_category_filtering() {
        let mut config = CoworkConfigToml::default();

        // All allowed by default
        assert!(config.is_category_allowed("file_operation"));
        assert!(config.is_category_allowed("code_execution"));

        // Block a category
        config.blocked_categories = vec!["code_execution".to_string()];
        assert!(config.is_category_allowed("file_operation"));
        assert!(!config.is_category_allowed("code_execution"));

        // Allow list
        config.blocked_categories.clear();
        config.allowed_categories = vec!["file_operation".to_string()];
        assert!(config.is_category_allowed("file_operation"));
        assert!(!config.is_category_allowed("code_execution"));

        // Blocked takes precedence
        config.blocked_categories = vec!["file_operation".to_string()];
        assert!(!config.is_category_allowed("file_operation"));
    }

    #[test]
    fn test_agent_config_includes_file_ops() {
        let config = CoworkConfigToml::default();
        assert!(config.file_ops.enabled);
        assert!(config.file_ops.require_confirmation_for_write);
    }
}
