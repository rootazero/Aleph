//! Stateless permission policy engine for Owner+Guest model
//!
//! This module provides pure-function permission checking based on IdentityContext.
//! No internal state is maintained - all permission information is carried in the
//! IdentityContext parameter.

use aleph_protocol::{IdentityContext, Role, GuestScope};

/// Permission check result
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionResult {
    Allowed,
    Denied { reason: String },
}

impl PermissionResult {
    pub fn is_allowed(&self) -> bool {
        matches!(self, PermissionResult::Allowed)
    }
}

/// Stateless policy engine for tool permission evaluation
///
/// All permission checks are pure functions based on IdentityContext.
/// No internal state is maintained.
///
/// # Philosophy
///
/// This engine embodies the "stateless security" pattern:
/// - **Pure functions**: Same input always produces same output
/// - **No side effects**: No state mutation, no external queries
/// - **Audit-friendly**: Permission decision is deterministic and reproducible
///
/// # Usage
///
/// ```ignore
/// use aleph_protocol::IdentityContext;
/// use alephcore::gateway::security::{PolicyEngine, PermissionResult};
///
/// let identity = IdentityContext::owner("session:main".into(), "cli".into());
/// let result = PolicyEngine::check_tool_permission(&identity, "shell:exec");
/// assert!(result.is_allowed());
/// ```
pub struct PolicyEngine;

impl PolicyEngine {
    /// Check if identity can execute a tool (pure function)
    ///
    /// This is a stateless permission check - all information needed for the
    /// decision is contained in the IdentityContext parameter.
    ///
    /// # Arguments
    ///
    /// * `identity` - Complete identity context with frozen permissions
    /// * `tool_name` - Tool to check (e.g., "shell:exec", "translate")
    ///
    /// # Returns
    ///
    /// - `PermissionResult::Allowed` if permitted
    /// - `PermissionResult::Denied` with reason if denied
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Owner always allowed
    /// let owner = IdentityContext::owner("s1".into(), "cli".into());
    /// assert!(PolicyEngine::check_tool_permission(&owner, "any_tool").is_allowed());
    ///
    /// // Guest with scope
    /// let scope = GuestScope {
    ///     allowed_tools: vec!["translate".into()],
    ///     expires_at: None,
    ///     display_name: None,
    /// };
    /// let guest = IdentityContext::guest("s2".into(), "g1".into(), scope, "web".into());
    /// assert!(PolicyEngine::check_tool_permission(&guest, "translate").is_allowed());
    /// assert!(!PolicyEngine::check_tool_permission(&guest, "shell:exec").is_allowed());
    /// ```
    pub fn check_tool_permission(
        identity: &IdentityContext,
        tool_name: &str,
    ) -> PermissionResult {
        match identity.role {
            Role::Owner => {
                // Owner has unrestricted access
                PermissionResult::Allowed
            }

            Role::Guest => {
                // Guest must have a scope
                let Some(ref scope) = identity.scope else {
                    return PermissionResult::Denied {
                        reason: format!(
                            "Guest '{}' has no permission scope",
                            identity.identity_id
                        ),
                    };
                };

                // Check expiration
                if let Some(expires_at) = scope.expires_at {
                    if identity.created_at > expires_at {
                        return PermissionResult::Denied {
                            reason: "Guest token expired".to_string(),
                        };
                    }
                }

                // Check tool permission
                Self::check_guest_scope(scope, tool_name, &identity.identity_id)
            }

            Role::Anonymous => PermissionResult::Denied {
                reason: "Authentication required".to_string(),
            },
        }
    }

    /// Check if a tool is allowed by guest scope (pure function)
    ///
    /// # Arguments
    ///
    /// * `scope` - Guest permission scope
    /// * `tool_name` - Tool to check
    /// * `guest_id` - Guest identifier (for error messages)
    ///
    /// # Returns
    ///
    /// Permission result with detailed reason if denied
    ///
    /// # Matching Rules
    ///
    /// - Exact match: "translate" matches "translate"
    /// - Category match: "shell" matches "shell:exec"
    /// - Wildcard: "*" matches any tool
    fn check_guest_scope(
        scope: &GuestScope,
        tool_name: &str,
        guest_id: &str,
    ) -> PermissionResult {
        // Extract tool category (e.g., "shell:exec" -> "shell")
        let tool_category = tool_name.split(':').next().unwrap_or(tool_name);

        // Check if tool or category is in allowed list
        let allowed = scope.allowed_tools.iter().any(|allowed| {
            allowed == tool_name       // Exact match
            || allowed == tool_category // Category match
            || allowed == "*"           // Wildcard
        });

        if allowed {
            PermissionResult::Allowed
        } else {
            PermissionResult::Denied {
                reason: format!(
                    "Tool '{}' not in guest '{}' scope (allowed: {:?})",
                    tool_name, guest_id, scope.allowed_tools
                ),
            }
        }
    }

}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to create a test scope
    fn test_scope(tools: Vec<&str>) -> GuestScope {
        GuestScope {
            allowed_tools: tools.iter().map(|s| s.to_string()).collect(),
            expires_at: None,
            display_name: None,
        }
    }

    #[test]
    fn test_owner_always_allowed() {
        let identity = IdentityContext::owner("session:main".to_string(), "cli".to_string());
        let result = PolicyEngine::check_tool_permission(&identity, "shell:exec");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_owner_allowed_any_tool() {
        let identity = IdentityContext::owner("s1".to_string(), "cli".to_string());
        assert!(PolicyEngine::check_tool_permission(&identity, "dangerous_tool").is_allowed());
        assert!(PolicyEngine::check_tool_permission(&identity, "shell:rm").is_allowed());
        assert!(PolicyEngine::check_tool_permission(&identity, "*").is_allowed());
    }

    #[test]
    fn test_anonymous_always_denied() {
        let identity = IdentityContext::anonymous("session:temp".to_string(), "web".to_string());
        let result = PolicyEngine::check_tool_permission(&identity, "translate");
        assert!(!result.is_allowed());
        assert!(matches!(result, PermissionResult::Denied { .. }));
    }

    #[test]
    fn test_guest_with_scope_allowed() {
        let scope = test_scope(vec!["translate"]);
        let identity = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest1".to_string(),
            scope,
            "telegram".to_string(),
        );

        let result = PolicyEngine::check_tool_permission(&identity, "translate");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_guest_without_permission_denied() {
        let scope = test_scope(vec!["translate"]);
        let identity = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest1".to_string(),
            scope,
            "telegram".to_string(),
        );

        let result = PolicyEngine::check_tool_permission(&identity, "shell:exec");
        assert!(!result.is_allowed());

        if let PermissionResult::Denied { reason } = result {
            assert!(reason.contains("not in guest"));
            assert!(reason.contains("shell:exec"));
        } else {
            panic!("Expected Denied result");
        }
    }

    #[test]
    fn test_guest_without_scope_denied() {
        // Manually create identity with no scope (shouldn't happen in practice)
        let mut identity = IdentityContext::owner("s1".to_string(), "cli".to_string());
        identity.role = Role::Guest;
        identity.identity_id = "guest1".to_string();
        identity.scope = None;

        let result = PolicyEngine::check_tool_permission(&identity, "translate");
        assert!(!result.is_allowed());

        if let PermissionResult::Denied { reason } = result {
            assert!(reason.contains("no permission scope"));
        }
    }

    #[test]
    fn test_guest_expired_token_denied() {
        let scope = GuestScope {
            allowed_tools: vec!["*".to_string()],
            expires_at: Some(1000), // Past timestamp
            display_name: None,
        };

        let mut identity = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest1".to_string(),
            scope,
            "telegram".to_string(),
        );

        // Set created_at to be after expiry
        identity.created_at = 2000;

        let result = PolicyEngine::check_tool_permission(&identity, "translate");
        assert!(!result.is_allowed());

        if let PermissionResult::Denied { reason } = result {
            assert!(reason.contains("expired"));
        }
    }

    #[test]
    fn test_guest_not_expired_allowed() {
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(3000), // Future timestamp
            display_name: None,
        };

        let mut identity = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest1".to_string(),
            scope,
            "telegram".to_string(),
        );

        // Set created_at to be before expiry
        identity.created_at = 2000;

        let result = PolicyEngine::check_tool_permission(&identity, "translate");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_tool_category_matching() {
        let scope = test_scope(vec!["shell"]);
        let identity = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest1".to_string(),
            scope,
            "cli".to_string(),
        );

        // Category "shell" should match "shell:exec"
        let result = PolicyEngine::check_tool_permission(&identity, "shell:exec");
        assert!(result.is_allowed());

        // But not other categories
        let result2 = PolicyEngine::check_tool_permission(&identity, "file:read");
        assert!(!result2.is_allowed());
    }

    #[test]
    fn test_wildcard_matching() {
        let scope = test_scope(vec!["*"]);
        let identity = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest1".to_string(),
            scope,
            "cli".to_string(),
        );

        assert!(PolicyEngine::check_tool_permission(&identity, "any_tool").is_allowed());
        assert!(PolicyEngine::check_tool_permission(&identity, "shell:exec").is_allowed());
        assert!(PolicyEngine::check_tool_permission(&identity, "translate").is_allowed());
    }

    #[test]
    fn test_multiple_allowed_tools() {
        let scope = test_scope(vec!["translate", "summarize", "search"]);
        let identity = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest1".to_string(),
            scope,
            "web".to_string(),
        );

        assert!(PolicyEngine::check_tool_permission(&identity, "translate").is_allowed());
        assert!(PolicyEngine::check_tool_permission(&identity, "summarize").is_allowed());
        assert!(PolicyEngine::check_tool_permission(&identity, "search").is_allowed());
        assert!(!PolicyEngine::check_tool_permission(&identity, "delete").is_allowed());
    }

}
