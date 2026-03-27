//! Single-layer safety guard for tool calls.
//!
//! Replaces the previous 3-layer tool filter system with a simple two-check approach:
//! 1. Pattern matching against blocked commands (hard block)
//! 2. Permission lookup (Allow / Ask / Deny) from merged tool permissions
//!
//! Permission levels are resolved via `ToolPermissionsConfig::merge` which
//! combines global policies and agent-level overrides (most restrictive wins).

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

use crate::config::types::policies::ToolPermissionsConfig;
use crate::extension::PermissionAction;

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
    /// The tool call is denied by permission policy.
    PolicyDenied { tool: String },
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
            SafetyError::PolicyDenied { tool } => {
                write!(f, "tool '{}' denied by permission policy", tool)
            }
        }
    }
}

impl std::error::Error for SafetyError {}

// =============================================================================
// SafetyGuard
// =============================================================================

/// Single-layer safety guard using pattern matching and permission levels.
pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,
    tool_permissions: HashMap<String, PermissionAction>,
    default_permission: PermissionAction,
}

impl SafetyGuard {
    /// Create a new guard from raw components.
    ///
    /// Each string in `blocked` is compiled as a regex pattern.
    /// `tool_permissions` maps tool names to permission levels.
    /// `default_permission` is used for tools not in the map.
    pub fn new(
        blocked: Vec<String>,
        tool_permissions: HashMap<String, PermissionAction>,
        default_permission: PermissionAction,
    ) -> Self {
        let blocked_patterns: Vec<Regex> = blocked
            .into_iter()
            .filter_map(|p| match Regex::new(&p) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::warn!(pattern = %p, error = %e, "Failed to compile safety regex — pattern skipped");
                    None
                }
            })
            .collect();
        Self {
            blocked_patterns,
            tool_permissions,
            default_permission,
        }
    }

    /// Create a guard with sensible defaults for common dangerous commands.
    ///
    /// Default policy: all tools allowed (no permission restrictions).
    /// Only truly destructive commands are hard-blocked.
    pub fn default_guard() -> Self {
        let blocked = default_blocked_patterns();
        Self::new(blocked, HashMap::new(), PermissionAction::Allow)
    }

    /// Create a guard from global and agent-level permission configs.
    ///
    /// Merges the two layers (most restrictive wins) and combines with
    /// the default blocked patterns.
    pub fn from_permissions(
        global: &ToolPermissionsConfig,
        agent: &ToolPermissionsConfig,
    ) -> Self {
        let merged = ToolPermissionsConfig::merge(global, agent);
        let blocked = default_blocked_patterns();
        Self::new(blocked, merged.overrides, merged.default)
    }

    /// Check whether a tool call is safe to execute.
    ///
    /// Returns `Ok(())` if the call is allowed unconditionally.
    /// Returns `Err(SafetyError::Blocked)` if it matches a blocked pattern (highest priority).
    /// Returns `Err(SafetyError::NeedsConfirmation)` if the tool permission is Ask.
    /// Returns `Err(SafetyError::PolicyDenied)` if the tool permission is Deny.
    pub fn check(&self, call: &ToolCall) -> Result<(), SafetyError> {
        // Build the haystack: "{name} {input_json}"
        let input_json = call.input.to_string();
        let haystack = format!("{} {}", call.name, input_json);

        // Blocked patterns take highest priority.
        for pattern in &self.blocked_patterns {
            if pattern.is_match(&haystack) {
                return Err(SafetyError::Blocked {
                    tool: call.name.clone(),
                    pattern: pattern.to_string(),
                });
            }
        }

        // Permission lookup
        let permission = self
            .tool_permissions
            .get(&call.name)
            .copied()
            .unwrap_or(self.default_permission);

        match permission {
            PermissionAction::Allow => Ok(()),
            PermissionAction::Ask => Err(SafetyError::NeedsConfirmation {
                tool: call.name.clone(),
            }),
            PermissionAction::Deny => Err(SafetyError::PolicyDenied {
                tool: call.name.clone(),
            }),
        }
    }
}

/// Default blocked patterns for dangerous system commands.
fn default_blocked_patterns() -> Vec<String> {
    vec![
        r"rm\s+-rf\s+/".to_string(),
        r"(?i)drop\s+database".to_string(),
        r"mkfs\.".to_string(),
        r"dd\s+if=.*of=/dev/".to_string(),
        r">\s*/dev/sd".to_string(),
    ]
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
            HashMap::new(),
            PermissionAction::Allow,
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
            HashMap::new(),
            PermissionAction::Allow,
        );
        let call = ToolCall {
            name: "read_file".to_string(),
            input: json!({ "path": "/tmp/foo.txt" }),
        };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_needs_confirmation_via_ask() {
        let perms = [("shell".to_string(), PermissionAction::Ask)]
            .into_iter()
            .collect();
        let guard = SafetyGuard::new(vec![], perms, PermissionAction::Allow);
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "echo hello" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::NeedsConfirmation { .. }));
        assert!(err.to_string().contains("confirmation"));
    }

    #[test]
    fn test_policy_denied() {
        let perms = [("shell".to_string(), PermissionAction::Deny)]
            .into_iter()
            .collect();
        let guard = SafetyGuard::new(vec![], perms, PermissionAction::Allow);
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "echo hello" }),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::PolicyDenied { .. }));
        assert!(err.to_string().contains("denied"));
    }

    #[test]
    fn test_blocked_takes_priority_over_permission() {
        let perms = [("shell".to_string(), PermissionAction::Allow)]
            .into_iter()
            .collect();
        let guard = SafetyGuard::new(
            vec![r"rm\s+-rf\s+/".to_string()],
            perms,
            PermissionAction::Allow,
        );
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "rm -rf /" }),
        };
        // Blocked wins even when tool permission is Allow.
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::Blocked { .. }));
    }

    #[test]
    fn test_default_permission_deny() {
        let guard = SafetyGuard::new(vec![], HashMap::new(), PermissionAction::Deny);
        let call = ToolCall {
            name: "any_tool".to_string(),
            input: json!({}),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::PolicyDenied { .. }));
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

        // Default policy: all tools allowed.
        for name in &["shell", "file_write", "file_delete"] {
            let call = ToolCall {
                name: name.to_string(),
                input: json!({ "safe": true }),
            };
            assert!(
                guard.check(&call).is_ok(),
                "expected Ok (allowed) for tool: {}",
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

    #[test]
    fn test_from_permissions() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("shell".to_string(), PermissionAction::Ask)]
                .into_iter()
                .collect(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("file_delete".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let guard = SafetyGuard::from_permissions(&global, &agent);

        // shell: Ask from global
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({}),
        };
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::NeedsConfirmation { .. })
        ));

        // file_delete: Deny from agent
        let call = ToolCall {
            name: "file_delete".to_string(),
            input: json!({}),
        };
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::PolicyDenied { .. })
        ));

        // read_file: Allow (default)
        let call = ToolCall {
            name: "read_file".to_string(),
            input: json!({}),
        };
        assert!(guard.check(&call).is_ok());
    }
}
