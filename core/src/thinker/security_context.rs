//! Security Context for Channel Capability Awareness
//!
//! This module defines policy-driven security types that describe what is ALLOWED
//! by security policy, orthogonal to InteractionManifest which describes what is
//! technically possible.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    SecurityContext                          │
//! │  ┌─────────────────┐  ┌──────────────┐  ┌───────────────┐  │
//! │  │  SandboxLevel   │  │ Tool Lists   │  │  Policies     │  │
//! │  │                 │  │              │  │               │  │
//! │  │ • None          │  │ allowed_tools│  │ filesystem    │  │
//! │  │ • Standard      │  │ denied_tools │  │ network       │  │
//! │  │ • Strict        │  │              │  │ elevated      │  │
//! │  │ • Untrusted     │  │              │  │               │  │
//! │  └─────────────────┘  └──────────────┘  └───────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Examples
//!
//! ```
//! use std::path::PathBuf;
//! use alephcore::thinker::{SecurityContext, SandboxLevel, ToolPermission};
//!
//! // Create a permissive context for trusted environments
//! let ctx = SecurityContext::permissive();
//! assert!(matches!(ctx.check_tool("bash"), ToolPermission::Allowed));
//!
//! // Create a strict context for untrusted inputs
//! let ctx = SecurityContext::strict_readonly(PathBuf::from("/workspace"));
//! assert!(matches!(ctx.check_tool("exec"), ToolPermission::Denied { .. }));
//! ```

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Sandbox isolation level for tool execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLevel {
    /// No sandboxing - full system access
    #[default]
    None,
    /// Standard sandboxing - workspace-scoped filesystem, network allowed
    Standard,
    /// Strict sandboxing - limited tools, no dangerous operations
    Strict,
    /// Untrusted mode - minimal permissions, heavy restrictions
    Untrusted,
}

impl SandboxLevel {
    /// Returns a human-readable description for use in prompts
    pub fn description(&self) -> &'static str {
        match self {
            Self::None => "Full system access with no sandboxing restrictions",
            Self::Standard => "Standard sandbox with workspace-scoped filesystem access",
            Self::Strict => "Strict sandbox with limited tool access and no dangerous operations",
            Self::Untrusted => "Untrusted mode with minimal permissions and heavy restrictions",
        }
    }
}

/// Permission result for a tool check
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ToolPermission {
    /// Tool is allowed to execute
    Allowed,
    /// Tool is denied with a reason
    Denied {
        /// Reason for denial
        reason: String,
    },
    /// Tool requires user approval before execution
    RequiresApproval {
        /// Prompt to display for approval
        prompt: String,
    },
}

impl ToolPermission {
    /// Check if the permission allows execution
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Check if the permission requires approval
    pub fn requires_approval(&self) -> bool {
        matches!(self, Self::RequiresApproval { .. })
    }
}

/// Policy for elevated/privileged operations (exec, bash, etc.)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevatedPolicy {
    /// Elevated operations are completely disabled
    #[default]
    Off,
    /// Ask for user approval on each elevated operation
    Ask,
    /// Allow only commands in the allowlist
    AllowList(Vec<String>),
    /// Full elevated access without restrictions
    Full,
}

/// Security context defining what operations are allowed by policy
///
/// This is orthogonal to `InteractionManifest` which describes technical
/// capabilities. SecurityContext describes what is ALLOWED, not what is
/// technically possible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityContext {
    /// The sandbox isolation level
    pub sandbox_level: SandboxLevel,
    /// Whitelist of allowed tools (None means all tools allowed)
    pub allowed_tools: Option<HashSet<String>>,
    /// Blacklist of denied tools (takes precedence over whitelist)
    pub denied_tools: HashSet<String>,
    /// Filesystem scope restriction (None means no restriction)
    pub filesystem_scope: Option<PathBuf>,
    /// Whether network operations are allowed
    pub network_allowed: bool,
    /// Policy for elevated operations (exec, bash)
    pub elevated_policy: ElevatedPolicy,
}

impl Default for SecurityContext {
    fn default() -> Self {
        Self::permissive()
    }
}

impl SecurityContext {
    /// Create a permissive context with full access
    ///
    /// Use this for trusted environments where the user has full control.
    pub fn permissive() -> Self {
        Self {
            sandbox_level: SandboxLevel::None,
            allowed_tools: None,
            denied_tools: HashSet::new(),
            filesystem_scope: None,
            network_allowed: true,
            elevated_policy: ElevatedPolicy::Full,
        }
    }

    /// Create a standard sandbox context scoped to a workspace
    ///
    /// This provides reasonable security for most use cases:
    /// - Filesystem access scoped to workspace
    /// - Network allowed
    /// - Elevated operations require approval
    pub fn standard_sandbox(workspace: PathBuf) -> Self {
        Self {
            sandbox_level: SandboxLevel::Standard,
            allowed_tools: None,
            denied_tools: HashSet::new(),
            filesystem_scope: Some(workspace),
            network_allowed: true,
            elevated_policy: ElevatedPolicy::Ask,
        }
    }

    /// Create a strict read-only context
    ///
    /// Use this for untrusted inputs or when maximum safety is needed:
    /// - Filesystem access scoped to workspace
    /// - No network access
    /// - No elevated operations (exec, bash)
    /// - File operations tool is denied
    pub fn strict_readonly(workspace: PathBuf) -> Self {
        let mut denied_tools = HashSet::new();
        denied_tools.insert("file_ops".to_string());
        denied_tools.insert("exec".to_string());
        denied_tools.insert("bash".to_string());
        denied_tools.insert("bash_exec".to_string());
        denied_tools.insert("code_exec".to_string());

        Self {
            sandbox_level: SandboxLevel::Strict,
            allowed_tools: None,
            denied_tools,
            filesystem_scope: Some(workspace),
            network_allowed: false,
            elevated_policy: ElevatedPolicy::Off,
        }
    }

    /// Check if a tool is allowed by this security context
    ///
    /// The check follows this precedence:
    /// 1. If tool is in denied_tools -> Denied
    /// 2. If allowed_tools is Some and tool not in it -> Denied
    /// 3. If tool is exec/bash -> check elevated_policy
    /// 4. If network_allowed is false and tool is network tool -> Denied
    /// 5. Otherwise -> Allowed
    pub fn check_tool(&self, tool_name: &str) -> ToolPermission {
        // 1. Check blacklist first (highest priority)
        if self.denied_tools.contains(tool_name) {
            return ToolPermission::Denied {
                reason: format!("Tool '{}' is explicitly denied by security policy", tool_name),
            };
        }

        // 2. Check whitelist if set
        if let Some(ref allowed) = self.allowed_tools {
            if !allowed.contains(tool_name) {
                return ToolPermission::Denied {
                    reason: format!(
                        "Tool '{}' is not in the allowed tools list",
                        tool_name
                    ),
                };
            }
        }

        // 3. Check elevated policy for exec/bash tools
        if is_exec_tool(tool_name) {
            return self.check_exec_permission(tool_name);
        }

        // 4. Check network policy for network tools
        if !self.network_allowed && is_network_tool(tool_name) {
            return ToolPermission::Denied {
                reason: format!(
                    "Tool '{}' requires network access which is not allowed",
                    tool_name
                ),
            };
        }

        // 5. Default: allowed
        ToolPermission::Allowed
    }

    /// Check execution permission based on elevated policy
    fn check_exec_permission(&self, tool_name: &str) -> ToolPermission {
        match &self.elevated_policy {
            ElevatedPolicy::Off => ToolPermission::Denied {
                reason: format!(
                    "Tool '{}' requires elevated permissions which are disabled",
                    tool_name
                ),
            },
            ElevatedPolicy::Ask => ToolPermission::RequiresApproval {
                prompt: format!(
                    "Tool '{}' requires elevated permissions. Allow execution?",
                    tool_name
                ),
            },
            ElevatedPolicy::AllowList(allowed) => {
                // For exec tools, check if the tool itself is in the allowlist
                // (in practice, this would check the command being executed)
                if allowed.iter().any(|a| a == tool_name) {
                    ToolPermission::Allowed
                } else {
                    ToolPermission::RequiresApproval {
                        prompt: format!(
                            "Tool '{}' is not in the elevated allowlist. Allow execution?",
                            tool_name
                        ),
                    }
                }
            }
            ElevatedPolicy::Full => ToolPermission::Allowed,
        }
    }

    /// Generate security notes for prompt injection
    ///
    /// Returns a list of security-related notes that should be included
    /// in the system prompt to inform the LLM of current restrictions.
    pub fn security_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();

        // Sandbox level note
        notes.push(format!(
            "Security Level: {} - {}",
            match self.sandbox_level {
                SandboxLevel::None => "None",
                SandboxLevel::Standard => "Standard",
                SandboxLevel::Strict => "Strict",
                SandboxLevel::Untrusted => "Untrusted",
            },
            self.sandbox_level.description()
        ));

        // Filesystem scope note
        if let Some(ref scope) = self.filesystem_scope {
            notes.push(format!(
                "Filesystem Access: Restricted to {}",
                scope.display()
            ));
        }

        // Network note
        if !self.network_allowed {
            notes.push("Network Access: Disabled".to_string());
        }

        // Elevated policy note
        match &self.elevated_policy {
            ElevatedPolicy::Off => {
                notes.push("Elevated Operations: Disabled (exec, bash not available)".to_string());
            }
            ElevatedPolicy::Ask => {
                notes.push(
                    "Elevated Operations: Require user approval before execution".to_string(),
                );
            }
            ElevatedPolicy::AllowList(list) => {
                notes.push(format!(
                    "Elevated Operations: Limited to allowlist ({} entries)",
                    list.len()
                ));
            }
            ElevatedPolicy::Full => {
                // No note needed for full access
            }
        }

        // Denied tools note
        if !self.denied_tools.is_empty() {
            notes.push(format!(
                "Denied Tools: {}",
                self.denied_tools
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        notes
    }
}

/// Check if a tool is a network-related tool
///
/// Network tools include:
/// - web_search: Performs web searches
/// - web_fetch: Fetches web pages
/// - http_request: Makes HTTP requests
pub fn is_network_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "web_search" | "web_fetch" | "http_request" | "search"
    )
}

/// Check if a tool is an exec/shell-related tool
fn is_exec_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "exec" | "bash" | "bash_exec" | "shell" | "code_exec"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissive_allows_all() {
        let ctx = SecurityContext::permissive();

        // All tools should be allowed
        assert!(matches!(ctx.check_tool("bash"), ToolPermission::Allowed));
        assert!(matches!(ctx.check_tool("exec"), ToolPermission::Allowed));
        assert!(matches!(
            ctx.check_tool("web_search"),
            ToolPermission::Allowed
        ));
        assert!(matches!(ctx.check_tool("file_ops"), ToolPermission::Allowed));
        assert!(matches!(
            ctx.check_tool("any_random_tool"),
            ToolPermission::Allowed
        ));
    }

    #[test]
    fn test_strict_denies_exec() {
        let ctx = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        // Exec tools should be denied
        assert!(matches!(
            ctx.check_tool("exec"),
            ToolPermission::Denied { .. }
        ));
        assert!(matches!(
            ctx.check_tool("bash"),
            ToolPermission::Denied { .. }
        ));
        assert!(matches!(
            ctx.check_tool("bash_exec"),
            ToolPermission::Denied { .. }
        ));
        assert!(matches!(
            ctx.check_tool("code_exec"),
            ToolPermission::Denied { .. }
        ));

        // file_ops should also be denied in strict mode
        assert!(matches!(
            ctx.check_tool("file_ops"),
            ToolPermission::Denied { .. }
        ));
    }

    #[test]
    fn test_network_blocked() {
        let ctx = SecurityContext::strict_readonly(PathBuf::from("/workspace"));

        // Network tools should be denied when network is not allowed
        assert!(!ctx.network_allowed);
        assert!(matches!(
            ctx.check_tool("web_search"),
            ToolPermission::Denied { .. }
        ));
        assert!(matches!(
            ctx.check_tool("web_fetch"),
            ToolPermission::Denied { .. }
        ));
        assert!(matches!(
            ctx.check_tool("http_request"),
            ToolPermission::Denied { .. }
        ));
        assert!(matches!(
            ctx.check_tool("search"),
            ToolPermission::Denied { .. }
        ));

        // Non-network tools should still be allowed (if not in denied list)
        assert!(matches!(
            ctx.check_tool("read_skill"),
            ToolPermission::Allowed
        ));
    }

    #[test]
    fn test_standard_sandbox_requires_approval() {
        let ctx = SecurityContext::standard_sandbox(PathBuf::from("/workspace"));

        // Exec tools should require approval in standard mode
        assert!(matches!(
            ctx.check_tool("bash"),
            ToolPermission::RequiresApproval { .. }
        ));
        assert!(matches!(
            ctx.check_tool("exec"),
            ToolPermission::RequiresApproval { .. }
        ));

        // Other tools should be allowed
        assert!(matches!(
            ctx.check_tool("web_search"),
            ToolPermission::Allowed
        ));
        assert!(matches!(ctx.check_tool("file_ops"), ToolPermission::Allowed));
    }

    #[test]
    fn test_blacklist_priority() {
        // Create a context with both whitelist and blacklist
        let mut allowed = HashSet::new();
        allowed.insert("bash".to_string());
        allowed.insert("web_search".to_string());

        let mut denied = HashSet::new();
        denied.insert("bash".to_string()); // bash is in both lists

        let ctx = SecurityContext {
            sandbox_level: SandboxLevel::Standard,
            allowed_tools: Some(allowed),
            denied_tools: denied,
            filesystem_scope: None,
            network_allowed: true,
            elevated_policy: ElevatedPolicy::Full,
        };

        // bash should be denied because blacklist takes priority
        assert!(matches!(
            ctx.check_tool("bash"),
            ToolPermission::Denied { .. }
        ));

        // web_search should be allowed (in whitelist, not in blacklist)
        assert!(matches!(
            ctx.check_tool("web_search"),
            ToolPermission::Allowed
        ));

        // file_ops should be denied (not in whitelist)
        assert!(matches!(
            ctx.check_tool("file_ops"),
            ToolPermission::Denied { .. }
        ));
    }

    #[test]
    fn test_security_notes() {
        let ctx = SecurityContext::strict_readonly(PathBuf::from("/workspace"));
        let notes = ctx.security_notes();

        // Should have multiple notes
        assert!(!notes.is_empty());

        // Should mention strict level
        assert!(notes.iter().any(|n| n.contains("Strict")));

        // Should mention filesystem restriction
        assert!(notes.iter().any(|n| n.contains("/workspace")));

        // Should mention network disabled
        assert!(notes.iter().any(|n| n.contains("Network Access: Disabled")));

        // Should mention elevated operations disabled
        assert!(notes.iter().any(|n| n.contains("Elevated Operations: Disabled")));

        // Should mention denied tools
        assert!(notes.iter().any(|n| n.contains("Denied Tools")));
    }

    #[test]
    fn test_sandbox_level_descriptions() {
        assert!(SandboxLevel::None.description().to_lowercase().contains("no"));
        assert!(SandboxLevel::Standard.description().contains("Standard"));
        assert!(SandboxLevel::Strict.description().contains("Strict"));
        assert!(SandboxLevel::Untrusted.description().contains("Untrusted"));
    }

    #[test]
    fn test_tool_permission_helpers() {
        let allowed = ToolPermission::Allowed;
        assert!(allowed.is_allowed());
        assert!(!allowed.requires_approval());

        let denied = ToolPermission::Denied {
            reason: "test".to_string(),
        };
        assert!(!denied.is_allowed());
        assert!(!denied.requires_approval());

        let requires = ToolPermission::RequiresApproval {
            prompt: "test".to_string(),
        };
        assert!(!requires.is_allowed());
        assert!(requires.requires_approval());
    }

    #[test]
    fn test_elevated_allowlist() {
        let ctx = SecurityContext {
            sandbox_level: SandboxLevel::Standard,
            allowed_tools: None,
            denied_tools: HashSet::new(),
            filesystem_scope: None,
            network_allowed: true,
            elevated_policy: ElevatedPolicy::AllowList(vec!["bash".to_string()]),
        };

        // bash is in allowlist - should be allowed
        assert!(matches!(ctx.check_tool("bash"), ToolPermission::Allowed));

        // exec is not in allowlist - should require approval
        assert!(matches!(
            ctx.check_tool("exec"),
            ToolPermission::RequiresApproval { .. }
        ));
    }

    #[test]
    fn test_is_network_tool() {
        assert!(is_network_tool("web_search"));
        assert!(is_network_tool("web_fetch"));
        assert!(is_network_tool("http_request"));
        assert!(is_network_tool("search"));
        assert!(!is_network_tool("file_ops"));
        assert!(!is_network_tool("bash"));
    }

    #[test]
    fn test_default_context() {
        let ctx = SecurityContext::default();
        // Default should be permissive
        assert_eq!(ctx.sandbox_level, SandboxLevel::None);
        assert!(ctx.network_allowed);
        assert!(matches!(ctx.elevated_policy, ElevatedPolicy::Full));
    }
}
