//! Config FFI Types
//!
//! Contains configuration FFI types:
//! - CodeExecConfigFFI: Code execution configuration
//! - FileOpsConfigFFI: File operations configuration

use crate::dispatcher::{
    DEFAULT_ALLOW_NETWORK, DEFAULT_CODE_EXEC_ENABLED, DEFAULT_CODE_EXEC_RUNTIME,
    DEFAULT_CODE_EXEC_TIMEOUT, DEFAULT_FILE_OPS_ENABLED, DEFAULT_MAX_FILE_SIZE, DEFAULT_PASS_ENV,
    DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE, DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE,
    DEFAULT_SANDBOX_ENABLED,
};

// ============================================================================
// Code Execution Config FFI
// ============================================================================

/// Code execution configuration for FFI
#[derive(Debug, Clone)]
pub struct CodeExecConfigFFI {
    /// Enable code execution (disabled by default for security)
    pub enabled: bool,
    /// Default runtime (shell, python, node)
    pub default_runtime: String,
    /// Execution timeout in seconds
    pub timeout_seconds: u64,
    /// Enable sandboxed execution
    pub sandbox_enabled: bool,
    /// Allow network access in sandbox
    pub allow_network: bool,
    /// Allowed runtimes (empty = all)
    pub allowed_runtimes: Vec<String>,
    /// Working directory for executions
    pub working_directory: Option<String>,
    /// Environment variables to pass
    pub pass_env: Vec<String>,
    /// Blocked command patterns
    pub blocked_commands: Vec<String>,
}

impl Default for CodeExecConfigFFI {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_CODE_EXEC_ENABLED,
            default_runtime: DEFAULT_CODE_EXEC_RUNTIME.to_string(),
            timeout_seconds: DEFAULT_CODE_EXEC_TIMEOUT,
            sandbox_enabled: DEFAULT_SANDBOX_ENABLED,
            allow_network: DEFAULT_ALLOW_NETWORK,
            allowed_runtimes: Vec::new(),
            working_directory: None,
            pass_env: DEFAULT_PASS_ENV.iter().map(|s| s.to_string()).collect(),
            blocked_commands: Vec::new(),
        }
    }
}

impl From<crate::config::types::agent::CodeExecConfigToml> for CodeExecConfigFFI {
    fn from(config: crate::config::types::agent::CodeExecConfigToml) -> Self {
        Self {
            enabled: config.enabled,
            default_runtime: config.default_runtime,
            timeout_seconds: config.timeout_seconds,
            sandbox_enabled: config.sandbox_enabled,
            allow_network: config.allow_network,
            allowed_runtimes: config.allowed_runtimes,
            working_directory: config.working_directory,
            pass_env: config.pass_env,
            blocked_commands: config.blocked_commands,
        }
    }
}

impl From<CodeExecConfigFFI> for crate::config::types::agent::CodeExecConfigToml {
    fn from(config: CodeExecConfigFFI) -> Self {
        Self {
            enabled: config.enabled,
            default_runtime: config.default_runtime,
            timeout_seconds: config.timeout_seconds,
            sandbox_enabled: config.sandbox_enabled,
            allow_network: config.allow_network,
            allowed_runtimes: config.allowed_runtimes,
            working_directory: config.working_directory,
            pass_env: config.pass_env,
            blocked_commands: config.blocked_commands,
        }
    }
}

// ============================================================================
// File Operations Config FFI
// ============================================================================

/// File operations configuration for FFI
#[derive(Debug, Clone)]
pub struct FileOpsConfigFFI {
    /// Enable file operations executor
    pub enabled: bool,
    /// Paths that are allowed for file operations (glob patterns)
    pub allowed_paths: Vec<String>,
    /// Paths that are denied for file operations (glob patterns)
    pub denied_paths: Vec<String>,
    /// Maximum file size in bytes for read operations
    pub max_file_size: u64,
    /// Require confirmation before write operations
    pub require_confirmation_for_write: bool,
    /// Require confirmation before delete operations
    pub require_confirmation_for_delete: bool,
}

impl Default for FileOpsConfigFFI {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_FILE_OPS_ENABLED,
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            require_confirmation_for_write: DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE,
            require_confirmation_for_delete: DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE,
        }
    }
}

impl From<crate::config::types::agent::FileOpsConfigToml> for FileOpsConfigFFI {
    fn from(config: crate::config::types::agent::FileOpsConfigToml) -> Self {
        Self {
            enabled: config.enabled,
            allowed_paths: config.allowed_paths,
            denied_paths: config.denied_paths,
            max_file_size: config.max_file_size,
            require_confirmation_for_write: config.require_confirmation_for_write,
            require_confirmation_for_delete: config.require_confirmation_for_delete,
        }
    }
}

impl From<FileOpsConfigFFI> for crate::config::types::agent::FileOpsConfigToml {
    fn from(config: FileOpsConfigFFI) -> Self {
        Self {
            enabled: config.enabled,
            allowed_paths: config.allowed_paths,
            denied_paths: config.denied_paths,
            max_file_size: config.max_file_size,
            require_confirmation_for_write: config.require_confirmation_for_write,
            require_confirmation_for_delete: config.require_confirmation_for_delete,
        }
    }
}
