//! Code execution executor configuration
//!
//! Contains `CodeExecConfigToml` for configuring code/script execution behavior.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tool_metadata::{
    DEFAULT_ALLOW_NETWORK, DEFAULT_CODE_EXEC_ENABLED, DEFAULT_CODE_EXEC_RUNTIME,
    DEFAULT_CODE_EXEC_TIMEOUT, DEFAULT_PASS_ENV, DEFAULT_SANDBOX_ENABLED,
};

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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
            return Err("agent.code_exec.timeout_seconds must be greater than 0".to_string());
        }
        if self.timeout_seconds > 3600 {
            tracing::warn!(
                timeout = self.timeout_seconds,
                "agent.code_exec.timeout_seconds is very high (>1 hour)"
            );
        }

        // Validate runtime names
        let valid_runtimes = [
            "shell", "bash", "zsh", "python", "python3", "node", "nodejs", "ruby",
        ];
        for runtime in &self.allowed_runtimes {
            if !valid_runtimes.contains(&runtime.as_str()) {
                tracing::warn!(
                    runtime = runtime,
                    "agent.code_exec.allowed_runtimes contains unknown runtime"
                );
            }
        }

        // Validate blocked command patterns are valid regex. Bounded build:
        // the patterns are operator config (untrusted input), and an
        // expansion bomb must fail validation, not exhaust memory on load.
        for pattern in &self.blocked_commands {
            if crate::security::safe_regex::bounded_builder(pattern)
                .build()
                .is_err()
            {
                return Err(format!(
                    "agent.code_exec.blocked_commands contains invalid regex: '{pattern}'"
                ));
            }
        }

        Ok(())
    }
}

// =============================================================================
// Default Functions
// =============================================================================

const fn default_code_exec_enabled() -> bool {
    DEFAULT_CODE_EXEC_ENABLED
}

fn default_code_exec_runtime() -> String {
    DEFAULT_CODE_EXEC_RUNTIME.to_string()
}

const fn default_code_exec_timeout() -> u64 {
    DEFAULT_CODE_EXEC_TIMEOUT
}

const fn default_code_exec_sandbox() -> bool {
    DEFAULT_SANDBOX_ENABLED
}

const fn default_code_exec_network() -> bool {
    DEFAULT_ALLOW_NETWORK
}

fn default_code_exec_pass_env() -> Vec<String> {
    DEFAULT_PASS_ENV.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validation runs at config load; an expansion bomb in operator config
    /// must fail validation through the same 1 MiB compiled-size cap the
    /// runtime compile sites enforce, not exhaust memory on the way in.
    #[test]
    fn blocked_commands_expansion_bomb_fails_validation() {
        let cfg = CodeExecConfigToml {
            blocked_commands: vec!["(a{1000}){1000}{1000}".to_string()],
            ..Default::default()
        };
        let err = cfg.validate().expect_err("expansion bomb must be rejected");
        assert!(err.contains("blocked_commands"), "{err}");
    }

    #[test]
    fn blocked_commands_sane_pattern_still_validates() {
        let cfg = CodeExecConfigToml {
            blocked_commands: vec![r"^rm\s+-rf\s+/".to_string()],
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }
}
