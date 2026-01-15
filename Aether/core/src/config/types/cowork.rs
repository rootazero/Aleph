//! Cowork configuration types
//!
//! Contains Cowork task orchestration configuration:
//! - CoworkConfigToml: Main configuration for the Cowork engine
//!
//! Cowork is a multi-task orchestration system that decomposes complex requests
//! into DAG-structured task graphs and executes them with parallel scheduling.

use serde::{Deserialize, Serialize};

// =============================================================================
// CoworkConfigToml
// =============================================================================

/// Cowork task orchestration configuration
///
/// Configures the Cowork engine for multi-task orchestration.
/// This includes task decomposition, parallel execution, and confirmation settings.
///
/// # Example TOML
/// ```toml
/// [cowork]
/// enabled = true
/// require_confirmation = true
/// max_parallelism = 4
/// dry_run = false
/// planner_model = "claude"
/// auto_execute_threshold = 0.9
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoworkConfigToml {
    /// Enable Cowork task orchestration
    #[serde(default = "default_cowork_enabled")]
    pub enabled: bool,

    /// Require user confirmation before executing task graphs
    /// When true, shows confirmation UI with task list before execution
    #[serde(default = "default_require_confirmation")]
    pub require_confirmation: bool,

    /// Maximum number of tasks to run in parallel
    /// Higher values improve throughput but increase resource usage
    #[serde(default = "default_max_parallelism")]
    pub max_parallelism: usize,

    /// Enable dry-run mode (plan tasks but don't execute)
    /// Useful for testing and debugging task graphs
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,

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
}

// =============================================================================
// Default Functions
// =============================================================================

pub fn default_cowork_enabled() -> bool {
    true
}

pub fn default_require_confirmation() -> bool {
    true
}

pub fn default_max_parallelism() -> usize {
    4
}

pub fn default_dry_run() -> bool {
    false
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

pub fn default_sandbox_enabled() -> bool {
    true
}

// =============================================================================
// Default Implementation
// =============================================================================

impl Default for CoworkConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_cowork_enabled(),
            require_confirmation: default_require_confirmation(),
            max_parallelism: default_max_parallelism(),
            dry_run: default_dry_run(),
            planner_provider: None,
            auto_execute_threshold: default_auto_execute_threshold(),
            max_tasks_per_graph: default_max_tasks_per_graph(),
            task_timeout_seconds: default_task_timeout_seconds(),
            sandbox_enabled: default_sandbox_enabled(),
            allowed_categories: Vec::new(),
            blocked_categories: Vec::new(),
        }
    }
}

// =============================================================================
// Conversion to Engine Config
// =============================================================================

impl CoworkConfigToml {
    /// Convert to engine configuration
    ///
    /// This creates a CoworkConfig suitable for the CoworkEngine.
    pub fn to_engine_config(&self) -> crate::cowork::CoworkConfig {
        crate::cowork::CoworkConfig {
            enabled: self.enabled,
            require_confirmation: self.require_confirmation,
            max_parallelism: self.max_parallelism,
            dry_run: self.dry_run,
        }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate max_parallelism
        if self.max_parallelism == 0 {
            return Err("cowork.max_parallelism must be greater than 0".to_string());
        }
        if self.max_parallelism > 32 {
            // Warning but not error
            tracing::warn!(
                max_parallelism = self.max_parallelism,
                "cowork.max_parallelism is very high (>32), this may cause resource issues"
            );
        }

        // Validate auto_execute_threshold
        if !(0.0..=1.0).contains(&self.auto_execute_threshold) {
            return Err(format!(
                "cowork.auto_execute_threshold must be between 0.0 and 1.0, got {}",
                self.auto_execute_threshold
            ));
        }

        // Validate max_tasks_per_graph
        if self.max_tasks_per_graph == 0 {
            return Err("cowork.max_tasks_per_graph must be greater than 0".to_string());
        }
        if self.max_tasks_per_graph > 100 {
            tracing::warn!(
                max_tasks = self.max_tasks_per_graph,
                "cowork.max_tasks_per_graph is very high (>100), this may indicate a problem"
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
                    "cowork.allowed_categories contains unknown category '{}'. Valid: {:?}",
                    cat, valid_categories
                ));
            }
        }

        for cat in &self.blocked_categories {
            if !valid_categories.contains(&cat.as_str()) {
                return Err(format!(
                    "cowork.blocked_categories contains unknown category '{}'. Valid: {:?}",
                    cat, valid_categories
                ));
            }
        }

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
        assert!(config.enabled);
        assert!(config.require_confirmation);
        assert_eq!(config.max_parallelism, 4);
        assert!(!config.dry_run);
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
    fn test_to_engine_config() {
        let config = CoworkConfigToml {
            enabled: true,
            require_confirmation: false,
            max_parallelism: 8,
            dry_run: true,
            ..Default::default()
        };

        let engine_config = config.to_engine_config();
        assert!(engine_config.enabled);
        assert!(!engine_config.require_confirmation);
        assert_eq!(engine_config.max_parallelism, 8);
        assert!(engine_config.dry_run);
    }
}
