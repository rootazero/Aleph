//! Permission policy engine for Owner+Guest model

use aleph_protocol::auth::{Role, GuestScope};
use dashmap::DashMap;

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

/// Policy engine for checking tool execution permissions
pub struct PolicyEngine {
    /// guest_id -> GuestScope
    guest_scopes: DashMap<String, GuestScope>,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self {
            guest_scopes: DashMap::new(),
        }
    }

    /// Register or update guest scope
    pub fn set_guest_scope(&self, guest_id: String, scope: GuestScope) {
        self.guest_scopes.insert(guest_id, scope);
    }

    /// Remove guest scope
    pub fn remove_guest_scope(&self, guest_id: &str) {
        self.guest_scopes.remove(guest_id);
    }

    /// Check if role can execute tool
    pub fn check_tool_permission(
        &self,
        role: &Role,
        guest_id: Option<&str>,
        tool_name: &str,
    ) -> PermissionResult {
        match role {
            Role::Owner => PermissionResult::Allowed,
            Role::Guest => {
                let Some(guest_id) = guest_id else {
                    return PermissionResult::Denied {
                        reason: "Guest ID required for Guest role".to_string(),
                    };
                };

                let Some(scope) = self.guest_scopes.get(guest_id) else {
                    return PermissionResult::Denied {
                        reason: format!("No scope found for guest '{}'", guest_id),
                    };
                };

                // Check expiry
                if let Some(expires_at) = scope.expires_at {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64;
                    if now > expires_at {
                        return PermissionResult::Denied {
                            reason: "Guest token expired".to_string(),
                        };
                    }
                }

                // Check tool permission
                let tool_category = tool_name.split(':').next().unwrap_or(tool_name);
                let allowed = scope.allowed_tools.iter().any(|allowed| {
                    allowed == tool_name || allowed == tool_category || allowed == "*"
                });

                if allowed {
                    PermissionResult::Allowed
                } else {
                    PermissionResult::Denied {
                        reason: format!("Tool '{}' not in guest scope", tool_name),
                    }
                }
            }
            Role::Anonymous => PermissionResult::Denied {
                reason: "Authentication required".to_string(),
            },
        }
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owner_always_allowed() {
        let engine = PolicyEngine::new();
        let result = engine.check_tool_permission(&Role::Owner, None, "shell:exec");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_anonymous_always_denied() {
        let engine = PolicyEngine::new();
        let result = engine.check_tool_permission(&Role::Anonymous, None, "translate");
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_guest_with_scope_allowed() {
        let engine = PolicyEngine::new();
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "translate");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_guest_without_permission_denied() {
        let engine = PolicyEngine::new();
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "shell:exec");
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_guest_expired_token_denied() {
        let engine = PolicyEngine::new();
        let past_timestamp = 1000000000; // Year 2001
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["*".to_string()],
                expires_at: Some(past_timestamp),
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "translate");
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_tool_category_matching() {
        let engine = PolicyEngine::new();
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["shell".to_string()],
                expires_at: None,
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "shell:exec");
        assert!(result.is_allowed());
    }
}
