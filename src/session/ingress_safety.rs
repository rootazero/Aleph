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
use crate::config::types::policies::ToolSafetyPolicy;
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
    safety_policy: Option<ToolSafetyPolicy>,
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
            safety_policy: None,
        }
    }

    /// Create a guard from raw components with an optional safety policy.
    pub fn with_policy(
        blocked: Vec<String>,
        tool_permissions: HashMap<String, PermissionAction>,
        default_permission: PermissionAction,
        safety_policy: Option<ToolSafetyPolicy>,
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
            safety_policy,
        }
    }

    /// Create a guard with sensible defaults for common dangerous commands.
    ///
    /// Default policy: all tools allowed, except `irreversible_high_risk` tools
    /// (bash, delete, destroy, etc.) which require confirmation (`Ask`).
    /// Truly destructive commands are also hard-blocked via pattern matching.
    pub fn default_guard() -> Self {
        let blocked = default_blocked_patterns();
        Self::with_policy(
            blocked,
            HashMap::new(),
            PermissionAction::Allow,
            Some(ToolSafetyPolicy::default()),
        )
    }

    /// Create a guard from global and agent-level permission configs.
    ///
    /// Merges the two layers (most restrictive wins) and combines with
    /// the default blocked patterns.
    pub fn from_permissions(
        global: &ToolPermissionsConfig,
        agent: &ToolPermissionsConfig,
        safety_policy: Option<ToolSafetyPolicy>,
    ) -> Self {
        let merged = ToolPermissionsConfig::merge(global, agent);
        let blocked = default_blocked_patterns();
        Self::with_policy(blocked, merged.overrides, merged.default, safety_policy)
    }

    /// Infer permission for a tool based on keyword matching in the safety policy.
    ///
    /// Priority order (first match wins):
    /// 1. high_risk keywords (delete, shell, bash...) → Ask
    /// 2. low_risk keywords (send, notify, post...) → Ask
    /// 3. readonly keywords (search, query, read...) → Allow
    /// 4. reversible keywords (create, write, edit...) → Allow
    /// 5. No match → default_permission
    ///
    /// If a tool name matches multiple categories (e.g. "delete_and_send"),
    /// the highest-priority match determines the result.
    fn infer_permission(&self, tool_name: &str) -> PermissionAction {
        if let Some(ref policy) = self.safety_policy {
            if policy.is_high_risk(tool_name) {
                return PermissionAction::Ask;
            }
            if policy.is_low_risk(tool_name) {
                return PermissionAction::Ask;
            }
            if policy.is_readonly(tool_name) {
                return PermissionAction::Allow;
            }
            if policy.is_reversible(tool_name) {
                return PermissionAction::Allow;
            }
        }
        self.default_permission
    }

    /// Check whether a tool name is classified as high-risk by the safety policy.
    ///
    /// This allows callers (e.g. tool_pipeline) to inspect risk level without
    /// performing a full safety check.
    pub fn is_high_risk(&self, tool_name: &str) -> bool {
        match self.tool_permissions.get(tool_name) {
            Some(p) => *p == PermissionAction::Ask,
            None => self.infer_permission(tool_name) == PermissionAction::Ask,
        }
    }

    /// Check whether a tool call is safe to execute.
    ///
    /// Returns `Ok(())` if the call is allowed unconditionally.
    /// Returns `Err(SafetyError::Blocked)` if it matches a blocked pattern (highest priority).
    /// Returns `Err(SafetyError::NeedsConfirmation)` if the tool permission is Ask.
    /// Returns `Err(SafetyError::PolicyDenied)` if the tool permission is Deny.
    pub fn check(&self, call: &ToolCall) -> Result<(), SafetyError> {
        // Build the haystack from extracted string values (not raw JSON).
        // Matching against serde_json::to_string() would let attackers bypass
        // patterns via JSON escape sequences (e.g. \n bypassing \s+).
        let mut haystack = call.name.clone();
        collect_string_values(&call.input, &mut haystack);

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
            .unwrap_or_else(|| self.infer_permission(&call.name));

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

    /// Check only permission rules (Allow/Ask/Deny) without blocked-pattern matching.
    ///
    /// Used when a hook has issued `PermissionDecision::Allow` — the hook vouches
    /// that the tool call is safe (skipping pattern checks), but settings-level
    /// deny/ask rules still apply unconditionally.
    pub fn check_permissions_only(&self, call: &ToolCall) -> Result<(), SafetyError> {
        let permission = self
            .tool_permissions
            .get(&call.name)
            .copied()
            .unwrap_or_else(|| self.infer_permission(&call.name));

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

/// Recursively extract all string values from a JSON value into the haystack.
///
/// This ensures blocked-pattern regexes match against actual string content
/// rather than JSON-encoded text, preventing bypass via escape sequences.
fn collect_string_values(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push(' ');
            out.push_str(s);
        }
        Value::Object(m) => m.values().for_each(|v| collect_string_values(v, out)),
        Value::Array(a) => a.iter().for_each(|v| collect_string_values(v, out)),
        _ => {}
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

        // High-risk tools should require confirmation (Ask).
        for name in &["bash_exec", "file_delete", "code_exec"] {
            let call = ToolCall {
                name: name.to_string(),
                input: json!({ "safe": true }),
            };
            assert!(
                matches!(
                    guard.check(&call),
                    Err(SafetyError::NeedsConfirmation { .. })
                ),
                "expected NeedsConfirmation for high-risk tool: {}",
                name
            );
        }

        // Non-high-risk tools should still be allowed.
        for name in &["read_file", "file_write", "memory_search"] {
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
        let guard = SafetyGuard::from_permissions(&global, &agent, None);

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

    #[test]
    fn test_is_high_risk() {
        let guard = SafetyGuard::default_guard();
        assert!(guard.is_high_risk("bash_exec"));
        assert!(guard.is_high_risk("file_delete"));
        assert!(!guard.is_high_risk("read_file"));
        assert!(!guard.is_high_risk("memory_search"));
    }

    #[test]
    fn check_permissions_only_skips_blocked_patterns() {
        let guard = SafetyGuard::new(
            vec![r"dangerous".to_string()],
            HashMap::new(),
            PermissionAction::Allow,
        );

        let call = ToolCall {
            name: "Bash".to_string(),
            input: json!({"command": "dangerous command"}),
        };

        // Full check should block it
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::Blocked { .. })
        ));

        // Permissions-only check should allow it (no pattern matching)
        assert!(guard.check_permissions_only(&call).is_ok());
    }

    #[test]
    fn check_permissions_only_still_enforces_deny() {
        let perms = [("Bash".to_string(), PermissionAction::Deny)]
            .into_iter()
            .collect();
        let guard = SafetyGuard::new(vec![], perms, PermissionAction::Allow);

        let call = ToolCall {
            name: "Bash".to_string(),
            input: json!({}),
        };

        assert!(matches!(
            guard.check_permissions_only(&call),
            Err(SafetyError::PolicyDenied { .. })
        ));
    }

    #[test]
    fn test_blocked_pattern_resists_json_escape_bypass() {
        let guard = SafetyGuard::new(
            vec![r"rm\s+-rf\s+/".to_string()],
            HashMap::new(),
            PermissionAction::Allow,
        );
        // Newline embedded in the command — previously bypassed because
        // serde_json::to_string() encodes \n as \\n (two chars, not whitespace).
        let call = ToolCall {
            name: "shell".to_string(),
            input: json!({ "command": "rm\n-rf /" }),
        };
        let result = guard.check(&call);
        assert!(
            matches!(result, Err(SafetyError::Blocked { .. })),
            "newline in command should still match blocked pattern"
        );
    }

    #[test]
    fn test_default_guard_uses_policy_inference() {
        let guard = SafetyGuard::default_guard();

        // "file_delete" is inferred as high-risk via keyword "delete" (not hardcoded)
        let call = ToolCall {
            name: "file_delete".to_string(),
            input: json!({}),
        };
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::NeedsConfirmation { .. })
        ));

        // "custom_destroy_tool" is also high-risk via keyword "destroy"
        let call = ToolCall {
            name: "custom_destroy_tool".to_string(),
            input: json!({}),
        };
        assert!(matches!(
            guard.check(&call),
            Err(SafetyError::NeedsConfirmation { .. })
        ));

        // "list_files" is readonly via keyword "list"
        let call = ToolCall {
            name: "list_files".to_string(),
            input: json!({}),
        };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_infer_permission_high_risk_keyword() {
        let policy = ToolSafetyPolicy::default();
        let guard = SafetyGuard::with_policy(
            vec![],
            HashMap::new(),
            PermissionAction::Allow,
            Some(policy),
        );
        let call = ToolCall {
            name: "file_delete".to_string(),
            input: json!({}),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::NeedsConfirmation { .. }));
    }

    #[test]
    fn test_infer_permission_readonly_keyword() {
        let policy = ToolSafetyPolicy::default();
        let guard = SafetyGuard::with_policy(
            vec![],
            HashMap::new(),
            PermissionAction::Allow,
            Some(policy),
        );
        let call = ToolCall {
            name: "memory_search".to_string(),
            input: json!({}),
        };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_infer_permission_low_risk_keyword() {
        let policy = ToolSafetyPolicy::default();
        let guard = SafetyGuard::with_policy(
            vec![],
            HashMap::new(),
            PermissionAction::Allow,
            Some(policy),
        );
        let call = ToolCall {
            name: "email_send".to_string(),
            input: json!({}),
        };
        let err = guard.check(&call).unwrap_err();
        assert!(matches!(err, SafetyError::NeedsConfirmation { .. }));
    }

    #[test]
    fn test_infer_permission_unknown_tool_uses_default() {
        let policy = ToolSafetyPolicy::default();
        let guard = SafetyGuard::with_policy(
            vec![],
            HashMap::new(),
            PermissionAction::Allow,
            Some(policy),
        );
        let call = ToolCall {
            name: "foobar".to_string(),
            input: json!({}),
        };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_explicit_override_beats_inference() {
        let policy = ToolSafetyPolicy::default();
        let perms = [("file_delete".to_string(), PermissionAction::Allow)]
            .into_iter()
            .collect();
        let guard = SafetyGuard::with_policy(vec![], perms, PermissionAction::Allow, Some(policy));
        let call = ToolCall {
            name: "file_delete".to_string(),
            input: json!({}),
        };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_no_policy_falls_back_to_default() {
        let guard = SafetyGuard::with_policy(vec![], HashMap::new(), PermissionAction::Allow, None);
        let call = ToolCall {
            name: "file_delete".to_string(),
            input: json!({}),
        };
        assert!(guard.check(&call).is_ok());
    }
}
