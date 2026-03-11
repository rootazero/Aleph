//! Single-layer safety guard for tool calls.
//!
//! Replaces the previous 3-layer tool filter system with a simple two-check approach:
//! 1. Pattern matching against blocked commands (hard block)
//! 2. Set membership for tools requiring user confirmation

use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;

// =============================================================================
// ToolCall
// =============================================================================

/// A tool invocation to be safety-checked before execution.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
}

// =============================================================================
// SafetyError
// =============================================================================

/// Safety check outcome when a tool call is not unconditionally allowed.
#[derive(Debug)]
pub enum SafetyError {
    /// The tool call matched a blocked pattern and must not execute.
    Blocked { tool: String, pattern: String },
    /// The tool call requires explicit user confirmation before execution.
    NeedsConfirmation { tool: String },
}

impl fmt::Display for SafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SafetyError::Blocked { tool, pattern } => {
                write!(f, "tool '{}' blocked by pattern '{}'", tool, pattern)
            }
            SafetyError::NeedsConfirmation { tool } => {
                write!(f, "tool '{}' requires user confirmation", tool)
            }
        }
    }
}

impl std::error::Error for SafetyError {}

// =============================================================================
// SafetyGuard
// =============================================================================

/// Single-layer safety guard using pattern matching and confirmation sets.
pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,
    confirmation_required: HashSet<String>,
}

impl SafetyGuard {
    /// Create a new guard from raw pattern strings and confirmation tool names.
    ///
    /// Each string in `blocked` is compiled as a regex pattern.
    /// Each string in `confirmation` is added to the confirmation-required set.
    pub fn new(blocked: Vec<String>, confirmation: Vec<String>) -> Self {
        let blocked_patterns = blocked
            .into_iter()
            .filter_map(|p| Regex::new(&p).ok())
            .collect();
        let confirmation_required = confirmation.into_iter().collect();
        Self {
            blocked_patterns,
            confirmation_required,
        }
    }

    /// Create a guard with sensible defaults for common dangerous commands.
    pub fn default_guard() -> Self {
        let blocked = vec![
            r"rm\s+-rf\s+/".to_string(),
            r"(?i)drop\s+database".to_string(),
            r"mkfs\.".to_string(),
            r"dd\s+if=.*of=/dev/".to_string(),
            r">\s*/dev/sd".to_string(),
        ];
        let confirmation = vec![
            "shell".to_string(),
            "file_write".to_string(),
            "file_delete".to_string(),
        ];
        Self::new(blocked, confirmation)
    }

    /// Check whether a tool call is safe to execute.
    ///
    /// Returns `Ok(())` if the call is allowed unconditionally.
    /// Returns `Err(SafetyError::Blocked)` if it matches a blocked pattern (highest priority).
    /// Returns `Err(SafetyError::NeedsConfirmation)` if the tool requires user confirmation.
    pub fn check(&self, call: &ToolCall) -> Result<(), SafetyError> {
        // Build the haystack: "{name} {input_json}"
        let input_json = call.input.to_string();
        let haystack = format!("{} {}", call.name, input_json);

        // Blocked patterns take priority over confirmation.
        for pattern in &self.blocked_patterns {
            if pattern.is_match(&haystack) {
                return Err(SafetyError::Blocked {
                    tool: call.name.clone(),
                    pattern: pattern.to_string(),
                });
            }
        }

        // Check confirmation set.
        if self.confirmation_required.contains(&call.name) {
            return Err(SafetyError::NeedsConfirmation {
                tool: call.name.clone(),
            });
        }

        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_blocked_pattern() {
        let guard = SafetyGuard::new(
            vec![r"rm\s+-rf\s+/".to_string()],
            vec![],
        );
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "rm -rf /" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::Blocked { .. }));
        assert!(err.to_string().contains("blocked"));
    }

    #[test]
    fn test_allowed_tool() {
        let guard = SafetyGuard::new(
            vec![r"rm\s+-rf\s+/".to_string()],
            vec!["shell".to_string()],
        );
        let call = ToolCall {
            name: "read_file".to_string(),
            input: json!({ "path": "/tmp/foo.txt" }),
        };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_confirmation_required() {
        let guard = SafetyGuard::new(
            vec![],
            vec!["shell".to_string(), "file_write".to_string()],
        );
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "echo hello" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::NeedsConfirmation { .. }));
        assert!(err.to_string().contains("confirmation"));
    }

    #[test]
    fn test_blocked_takes_priority_over_confirmation() {
        let guard = SafetyGuard::new(
            vec![r"rm\s+-rf\s+/".to_string()],
            vec!["shell".to_string()],
        );
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "rm -rf /" }),
        };
        // "shell" is in confirmation set, but the pattern also matches — Blocked wins.
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::Blocked { .. }));
    }

    #[test]
    fn test_default_guard_has_sensible_defaults() {
        let guard = SafetyGuard::default_guard();

        // Dangerous commands should be blocked.
        let dangerous_calls = vec![
            ToolCall {
                name: "shell".to_string(),
                input: json!({ "command": "rm -rf /" }),
            },
            ToolCall {
                name: "shell".to_string(),
                input: json!({ "command": "DROP DATABASE users" }),
            },
            ToolCall {
                name: "shell".to_string(),
                input: json!({ "command": "mkfs.ext4 /dev/sda1" }),
            },
            ToolCall {
                name: "shell".to_string(),
                input: json!({ "command": "dd if=/dev/zero of=/dev/sda" }),
            },
            ToolCall {
                name: "shell".to_string(),
                input: json!({ "command": "> /dev/sda" }),
            },
        ];
        for call in &dangerous_calls {
            let result = guard.check(call);
            assert!(
                matches!(result, Err(SafetyError::Blocked { .. })),
                "expected Blocked for input: {:?}",
                call.input
            );
        }

        // Confirmation-required tools should need confirmation (when not blocked).
        for name in &["shell", "file_write", "file_delete"] {
            let call = ToolCall {
                name: name.to_string(),
                input: json!({ "safe": true }),
            };
            let result = guard.check(&call);
            assert!(
                matches!(result, Err(SafetyError::NeedsConfirmation { .. })),
                "expected NeedsConfirmation for tool: {}",
                name
            );
        }

        // Safe tool should pass.
        let safe = ToolCall {
            name: "read_file".to_string(),
            input: json!({ "path": "/tmp/test" }),
        };
        assert!(guard.check(&safe).is_ok());
    }
}
