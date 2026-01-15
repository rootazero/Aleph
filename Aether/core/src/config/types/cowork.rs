//! Cowork configuration types
//!
//! Contains Cowork task orchestration configuration:
//! - CoworkConfigToml: Main configuration for the Cowork engine
//! - FileOpsConfigToml: File operations executor configuration
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

    /// File operations configuration
    #[serde(default)]
    pub file_ops: FileOpsConfigToml,

    /// Code execution configuration
    #[serde(default)]
    pub code_exec: CodeExecConfigToml,
}

// =============================================================================
// FileOpsConfigToml
// =============================================================================

/// File operations executor configuration
///
/// Configures permissions and behavior for file system operations.
/// Uses path-based access control with allowed/denied lists.
///
/// # Example TOML
/// ```toml
/// [cowork.file_ops]
/// enabled = true
/// allowed_paths = ["~/Downloads", "~/Documents"]
/// denied_paths = ["~/.ssh", "~/.gnupg"]
/// max_file_size = "100MB"
/// require_confirmation_for_write = true
/// require_confirmation_for_delete = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpsConfigToml {
    /// Enable file operations executor
    #[serde(default = "default_file_ops_enabled")]
    pub enabled: bool,

    /// Paths that are allowed for file operations (glob patterns)
    /// Empty list = all paths allowed (except denied)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_paths: Vec<String>,

    /// Paths that are denied for file operations (glob patterns)
    /// Takes precedence over allowed_paths
    /// Default denied paths (~/.ssh, ~/.gnupg, etc.) are always applied
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_paths: Vec<String>,

    /// Maximum file size in bytes for read operations
    /// 0 = unlimited
    /// Accepts human-readable values: "100MB", "1GB", etc.
    #[serde(default = "default_max_file_size", deserialize_with = "deserialize_file_size")]
    pub max_file_size: u64,

    /// Require confirmation before write operations
    #[serde(default = "default_require_confirmation_for_write")]
    pub require_confirmation_for_write: bool,

    /// Require confirmation before delete operations
    #[serde(default = "default_require_confirmation_for_delete")]
    pub require_confirmation_for_delete: bool,
}

// =============================================================================
// CodeExecConfigToml
// =============================================================================

/// Code execution executor configuration
///
/// Configures code/script execution behavior and security.
/// Code execution is disabled by default for security.
///
/// # Example TOML
/// ```toml
/// [cowork.code_exec]
/// enabled = false
/// default_runtime = "shell"
/// timeout_seconds = 60
/// sandbox_enabled = true
/// allowed_runtimes = ["shell", "python"]
/// allow_network = false
/// working_directory = "~/Downloads"
/// pass_env = ["PATH", "HOME"]
/// blocked_commands = ["rm -rf /", "sudo"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecConfigToml {
    /// Enable code execution
    /// SECURITY: Disabled by default
    #[serde(default = "default_code_exec_enabled")]
    pub enabled: bool,

    /// Default runtime for code execution
    #[serde(default = "default_code_exec_runtime")]
    pub default_runtime: String,

    /// Execution timeout in seconds
    #[serde(default = "default_code_exec_timeout")]
    pub timeout_seconds: u64,

    /// Enable sandboxed execution (macOS sandbox-exec)
    #[serde(default = "default_code_exec_sandbox")]
    pub sandbox_enabled: bool,

    /// Allowed runtimes (empty = all)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_runtimes: Vec<String>,

    /// Allow network access in sandbox
    #[serde(default = "default_code_exec_network")]
    pub allow_network: bool,

    /// Working directory for executions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,

    /// Environment variables to pass to executed code
    #[serde(default = "default_code_exec_pass_env")]
    pub pass_env: Vec<String>,

    /// Blocked command patterns (regex)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_commands: Vec<String>,
}

impl Default for CodeExecConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_code_exec_enabled(),
            default_runtime: default_code_exec_runtime(),
            timeout_seconds: default_code_exec_timeout(),
            sandbox_enabled: default_code_exec_sandbox(),
            allowed_runtimes: Vec::new(),
            allow_network: default_code_exec_network(),
            working_directory: None,
            pass_env: default_code_exec_pass_env(),
            blocked_commands: Vec::new(),
        }
    }
}

impl CodeExecConfigToml {
    /// Validate the code execution configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate timeout
        if self.timeout_seconds == 0 {
            return Err("cowork.code_exec.timeout_seconds must be greater than 0".to_string());
        }
        if self.timeout_seconds > 3600 {
            tracing::warn!(
                timeout = self.timeout_seconds,
                "cowork.code_exec.timeout_seconds is very high (>1 hour)"
            );
        }

        // Validate runtime names
        let valid_runtimes = ["shell", "bash", "zsh", "python", "python3", "node", "nodejs", "ruby"];
        for runtime in &self.allowed_runtimes {
            if !valid_runtimes.contains(&runtime.as_str()) {
                tracing::warn!(
                    runtime = runtime,
                    "cowork.code_exec.allowed_runtimes contains unknown runtime"
                );
            }
        }

        // Validate blocked command patterns are valid regex
        for pattern in &self.blocked_commands {
            if regex::Regex::new(pattern).is_err() {
                return Err(format!(
                    "cowork.code_exec.blocked_commands contains invalid regex: '{}'",
                    pattern
                ));
            }
        }

        Ok(())
    }

    /// Create a CodeExecutor from this configuration
    pub fn create_executor(
        &self,
        permission_checker: crate::cowork::executor::PathPermissionChecker,
    ) -> crate::cowork::executor::CodeExecutor {
        use std::path::PathBuf;

        // Expand tilde in working directory
        let working_dir = self.working_directory.as_ref().map(|s| {
            if s.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    return PathBuf::from(s.replacen("~", home.to_string_lossy().as_ref(), 1));
                }
            } else if s == "~" {
                if let Some(home) = dirs::home_dir() {
                    return home;
                }
            }
            PathBuf::from(s)
        });

        crate::cowork::executor::CodeExecutor::new(
            self.enabled,
            self.default_runtime.clone(),
            self.timeout_seconds,
            self.sandbox_enabled,
            self.allowed_runtimes.clone(),
            self.allow_network,
            self.blocked_commands.clone(),
            permission_checker,
            working_dir,
            self.pass_env.clone(),
        )
    }
}

// Code execution default functions
fn default_code_exec_enabled() -> bool {
    false // Disabled by default for security
}

fn default_code_exec_runtime() -> String {
    "shell".to_string()
}

fn default_code_exec_timeout() -> u64 {
    60 // 1 minute default
}

fn default_code_exec_sandbox() -> bool {
    true // Sandbox enabled by default
}

fn default_code_exec_network() -> bool {
    false // Network disabled by default
}

fn default_code_exec_pass_env() -> Vec<String> {
    vec!["PATH".to_string(), "HOME".to_string(), "USER".to_string()]
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

// FileOps default functions
pub fn default_file_ops_enabled() -> bool {
    true
}

pub fn default_max_file_size() -> u64 {
    100 * 1024 * 1024 // 100MB
}

pub fn default_require_confirmation_for_write() -> bool {
    true
}

pub fn default_require_confirmation_for_delete() -> bool {
    true
}

/// Deserialize file size from human-readable string or number
///
/// Supports formats like "100MB", "1GB", "500KB", or plain numbers (bytes)
fn deserialize_file_size<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FileSizeValue {
        Number(u64),
        String(String),
    }

    match FileSizeValue::deserialize(deserializer)? {
        FileSizeValue::Number(n) => Ok(n),
        FileSizeValue::String(s) => parse_file_size(&s).map_err(D::Error::custom),
    }
}

/// Parse human-readable file size string
fn parse_file_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_uppercase();

    // Try to parse as plain number first
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }

    // Parse with suffix
    let (num_part, suffix) = if s.ends_with("GB") {
        (&s[..s.len() - 2], 1024 * 1024 * 1024)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1024 * 1024)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1024)
    } else if s.ends_with('B') {
        (&s[..s.len() - 1], 1)
    } else {
        return Err(format!("Invalid file size format: '{}'. Use formats like '100MB', '1GB', etc.", s));
    };

    let num: u64 = num_part
        .trim()
        .parse()
        .map_err(|_| format!("Invalid number in file size: '{}'", num_part))?;

    Ok(num * suffix)
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
            file_ops: FileOpsConfigToml::default(),
            code_exec: CodeExecConfigToml::default(),
        }
    }
}

impl Default for FileOpsConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_file_ops_enabled(),
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
            max_file_size: default_max_file_size(),
            require_confirmation_for_write: default_require_confirmation_for_write(),
            require_confirmation_for_delete: default_require_confirmation_for_delete(),
        }
    }
}

impl FileOpsConfigToml {
    /// Validate the file ops configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate max_file_size (warn if very large)
        if self.max_file_size > 10 * 1024 * 1024 * 1024 {
            tracing::warn!(
                max_file_size = self.max_file_size,
                "cowork.file_ops.max_file_size is very large (>10GB)"
            );
        }

        // Validate path patterns are valid glob patterns
        for path in &self.allowed_paths {
            if glob::Pattern::new(path).is_err() {
                return Err(format!(
                    "cowork.file_ops.allowed_paths contains invalid glob pattern: '{}'",
                    path
                ));
            }
        }

        for path in &self.denied_paths {
            if glob::Pattern::new(path).is_err() {
                return Err(format!(
                    "cowork.file_ops.denied_paths contains invalid glob pattern: '{}'",
                    path
                ));
            }
        }

        Ok(())
    }

    /// Create a FileOpsExecutor from this configuration
    pub fn create_executor(&self) -> crate::cowork::executor::FileOpsExecutor {
        crate::cowork::executor::FileOpsExecutor::new(
            self.allowed_paths.clone(),
            self.denied_paths.clone(),
            self.max_file_size,
            self.require_confirmation_for_write,
            self.require_confirmation_for_delete,
        )
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

        // Validate file_ops configuration
        self.file_ops.validate()?;

        // Validate code_exec configuration
        self.code_exec.validate()?;

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

    // =========================================================================
    // FileOpsConfigToml Tests
    // =========================================================================

    #[test]
    fn test_file_ops_default_config() {
        let config = FileOpsConfigToml::default();
        assert!(config.enabled);
        assert!(config.allowed_paths.is_empty());
        assert!(config.denied_paths.is_empty());
        assert_eq!(config.max_file_size, 100 * 1024 * 1024); // 100MB
        assert!(config.require_confirmation_for_write);
        assert!(config.require_confirmation_for_delete);
    }

    #[test]
    fn test_file_ops_validation() {
        let mut config = FileOpsConfigToml::default();

        // Valid config should pass
        assert!(config.validate().is_ok());

        // Valid glob patterns
        config.allowed_paths = vec!["~/Documents/**".to_string()];
        assert!(config.validate().is_ok());

        // Invalid glob pattern
        config.allowed_paths = vec!["[invalid".to_string()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_parse_file_size() {
        assert_eq!(parse_file_size("100").unwrap(), 100);
        assert_eq!(parse_file_size("1KB").unwrap(), 1024);
        assert_eq!(parse_file_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(parse_file_size("1GB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_file_size("100MB").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_file_size("  50 MB  ").unwrap(), 50 * 1024 * 1024);

        // Case insensitive
        assert_eq!(parse_file_size("1mb").unwrap(), 1024 * 1024);
        assert_eq!(parse_file_size("1Mb").unwrap(), 1024 * 1024);

        // Invalid formats
        assert!(parse_file_size("invalid").is_err());
        assert!(parse_file_size("100TB").is_err()); // TB not supported
    }

    #[test]
    fn test_cowork_config_includes_file_ops() {
        let config = CoworkConfigToml::default();
        assert!(config.file_ops.enabled);
        assert!(config.file_ops.require_confirmation_for_write);
    }
}
