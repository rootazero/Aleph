//! Authentication types for Owner+Guest model

use serde::{Deserialize, Serialize};

/// User role in Personal AI Hub
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full system control, can manage guests
    Owner,
    /// Limited access with scoped permissions
    Guest,
    /// Unauthenticated, access denied
    Anonymous,
}

impl Default for Role {
    fn default() -> Self {
        Self::Anonymous
    }
}

/// Guest permission scope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestScope {
    /// Allowed tool names or categories
    pub allowed_tools: Vec<String>,
    /// Token expiration timestamp (Unix seconds)
    pub expires_at: Option<i64>,
    /// Human-readable name
    pub display_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_default_is_anonymous() {
        assert_eq!(Role::default(), Role::Anonymous);
    }

    #[test]
    fn test_role_serde() {
        let role = Role::Owner;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"owner\"");
        let parsed: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Role::Owner);
    }

    #[test]
    fn test_guest_scope_with_expiry() {
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(1735689600),
            display_name: Some("Mom".to_string()),
        };
        let json = serde_json::to_string(&scope).unwrap();
        let parsed: GuestScope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allowed_tools, vec!["translate"]);
    }
}
