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

impl Role {
    /// Returns true if the role is Owner
    pub fn is_owner(&self) -> bool {
        matches!(self, Self::Owner)
    }

    /// Returns true if the role is Guest
    pub fn is_guest(&self) -> bool {
        matches!(self, Self::Guest)
    }

    /// Returns true if the role is Anonymous
    pub fn is_anonymous(&self) -> bool {
        matches!(self, Self::Anonymous)
    }
}

/// Guest permission scope
///
/// Defines what a guest user can do and for how long.
/// - `allowed_tools`: Tool names (e.g., "translate") or categories (e.g., "text_*")
/// - `expires_at`: Unix timestamp in seconds (UTC timezone)
/// - `display_name`: Human-readable identifier for the guest
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestScope {
    /// Allowed tool names or categories
    pub allowed_tools: Vec<String>,
    /// Token expiration timestamp (Unix seconds, UTC)
    pub expires_at: Option<i64>,
    /// Human-readable name
    pub display_name: Option<String>,
}

impl GuestScope {
    /// Returns true if the scope has expired based on current time
    ///
    /// # Arguments
    /// * `current_time` - Unix timestamp in seconds (UTC) to check against
    ///
    /// # Returns
    /// - `true` if `expires_at` is set and current_time >= expires_at
    /// - `false` if `expires_at` is None or current_time < expires_at
    pub fn is_expired(&self, current_time: i64) -> bool {
        self.expires_at.map_or(false, |exp| current_time >= exp)
    }

    /// Checks if a tool name is allowed by this scope
    ///
    /// # Arguments
    /// * `tool_name` - The tool name to check
    ///
    /// # Returns
    /// `true` if the tool is explicitly listed in `allowed_tools`
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        self.allowed_tools.iter().any(|t| t == tool_name)
    }
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
    fn test_role_is_owner() {
        assert!(Role::Owner.is_owner());
        assert!(!Role::Guest.is_owner());
        assert!(!Role::Anonymous.is_owner());
    }

    #[test]
    fn test_role_is_guest() {
        assert!(!Role::Owner.is_guest());
        assert!(Role::Guest.is_guest());
        assert!(!Role::Anonymous.is_guest());
    }

    #[test]
    fn test_role_is_anonymous() {
        assert!(!Role::Owner.is_anonymous());
        assert!(!Role::Guest.is_anonymous());
        assert!(Role::Anonymous.is_anonymous());
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

    #[test]
    fn test_guest_scope_is_expired() {
        let scope = GuestScope {
            allowed_tools: vec![],
            expires_at: Some(1000),
            display_name: None,
        };

        // Before expiration
        assert!(!scope.is_expired(999));

        // At expiration
        assert!(scope.is_expired(1000));

        // After expiration
        assert!(scope.is_expired(1001));

        // No expiration set
        let no_expiry = GuestScope {
            allowed_tools: vec![],
            expires_at: None,
            display_name: None,
        };
        assert!(!no_expiry.is_expired(999999));
    }

    #[test]
    fn test_guest_scope_allows_tool() {
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string(), "summarize".to_string()],
            expires_at: None,
            display_name: None,
        };

        assert!(scope.allows_tool("translate"));
        assert!(scope.allows_tool("summarize"));
        assert!(!scope.allows_tool("delete_file"));
        assert!(!scope.allows_tool(""));
    }

    #[test]
    fn test_guest_scope_equality() {
        let scope1 = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(1000),
            display_name: Some("Alice".to_string()),
        };

        let scope2 = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(1000),
            display_name: Some("Alice".to_string()),
        };

        let scope3 = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(2000),
            display_name: Some("Alice".to_string()),
        };

        assert_eq!(scope1, scope2);
        assert_ne!(scope1, scope3);
    }
}
