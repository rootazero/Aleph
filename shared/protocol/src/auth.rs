//! Authentication types for Owner+Guest model

use serde::{Deserialize, Serialize};
use uuid;

/// User role in Personal AI Hub
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Role {
    /// Full system control, can manage guests
    Owner,
    /// Limited access with scoped permissions
    Guest,
    /// Unauthenticated, access denied
    #[default]
    Anonymous,
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
        self.expires_at.is_some_and(|exp| current_time >= exp)
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

/// Identity context for a single execution request
///
/// This is an immutable snapshot of the caller's identity and permissions
/// at the moment the request was made. It serves as audit evidence.
///
/// # Philosophy
///
/// IdentityContext embodies the "Certificate of Authority" pattern:
/// - **Immutable**: Permissions frozen at request time, immune to mid-execution changes
/// - **Self-contained**: All audit information embedded (no external queries needed)
/// - **Traceable**: Unique request_id for log correlation
///
/// # Usage
///
/// ```ignore
/// // Owner context
/// let owner_ctx = IdentityContext::owner("session:main".into(), "cli".into());
///
/// // Guest context with scope
/// let guest_ctx = IdentityContext::guest(
///     "session:guest1".into(),
///     "guest-uuid".into(),
///     scope,
///     "telegram".into(),
/// );
///
/// // Check permissions
/// let result = PolicyEngine::check_tool_permission(&guest_ctx, "translate");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityContext {
    /// Unique identifier for this request (for log correlation)
    pub request_id: String,

    /// Session key that originated this request
    pub session_key: String,

    /// User's role (Owner, Guest, Anonymous)
    pub role: Role,

    /// Specific identity ID
    /// - "owner" for Role::Owner
    /// - guest_id (UUID) for Role::Guest
    /// - "anonymous" for Role::Anonymous
    pub identity_id: String,

    /// Guest scope (only present for Role::Guest)
    /// This is the permission snapshot frozen at request time
    pub scope: Option<GuestScope>,

    /// Timestamp when this context was created (Unix seconds)
    pub created_at: i64,

    /// Source channel (e.g., "telegram", "cli", "websocket")
    pub source_channel: String,
}

impl IdentityContext {
    /// Create an Owner identity context
    ///
    /// # Arguments
    ///
    /// * `session_key` - Session key for this request
    /// * `source_channel` - Channel identifier (e.g., "cli", "telegram")
    ///
    /// # Returns
    ///
    /// IdentityContext with Role::Owner and no scope restrictions
    pub fn owner(session_key: String, source_channel: String) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            role: Role::Owner,
            identity_id: "owner".to_string(),
            scope: None,
            created_at: Self::now_unix_timestamp(),
            source_channel,
        }
    }

    /// Create a Guest identity context
    ///
    /// # Arguments
    ///
    /// * `session_key` - Session key for this request
    /// * `guest_id` - Unique guest identifier (usually UUID)
    /// * `scope` - Guest permission scope (frozen at session creation)
    /// * `source_channel` - Channel identifier
    ///
    /// # Returns
    ///
    /// IdentityContext with Role::Guest and embedded scope
    pub fn guest(
        session_key: String,
        guest_id: String,
        scope: GuestScope,
        source_channel: String,
    ) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            role: Role::Guest,
            identity_id: guest_id,
            scope: Some(scope),
            created_at: Self::now_unix_timestamp(),
            source_channel,
        }
    }

    /// Create an Anonymous identity context (always denied)
    ///
    /// # Arguments
    ///
    /// * `session_key` - Session key for this request
    /// * `source_channel` - Channel identifier
    ///
    /// # Returns
    ///
    /// IdentityContext with Role::Anonymous (all operations denied)
    pub fn anonymous(session_key: String, source_channel: String) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            role: Role::Anonymous,
            identity_id: "anonymous".to_string(),
            scope: None,
            created_at: Self::now_unix_timestamp(),
            source_channel,
        }
    }

    /// Get current Unix timestamp in seconds
    fn now_unix_timestamp() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("System clock is before Unix epoch")
            .as_secs() as i64
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

    // IdentityContext tests
    #[test]
    fn test_identity_context_owner_factory() {
        let ctx = IdentityContext::owner("session:main".to_string(), "cli".to_string());

        assert_eq!(ctx.session_key, "session:main");
        assert_eq!(ctx.role, Role::Owner);
        assert_eq!(ctx.identity_id, "owner");
        assert_eq!(ctx.source_channel, "cli");
        assert!(ctx.scope.is_none());
        assert!(!ctx.request_id.is_empty());
    }

    #[test]
    fn test_identity_context_guest_factory() {
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(2000),
            display_name: Some("Guest User".to_string()),
        };

        let ctx = IdentityContext::guest(
            "session:guest1".to_string(),
            "guest-uuid-123".to_string(),
            scope.clone(),
            "telegram".to_string(),
        );

        assert_eq!(ctx.session_key, "session:guest1");
        assert_eq!(ctx.role, Role::Guest);
        assert_eq!(ctx.identity_id, "guest-uuid-123");
        assert_eq!(ctx.source_channel, "telegram");
        assert!(ctx.scope.is_some());
        assert_eq!(ctx.scope.unwrap(), scope);
    }

    #[test]
    fn test_identity_context_anonymous_factory() {
        let ctx = IdentityContext::anonymous("session:temp".to_string(), "web".to_string());

        assert_eq!(ctx.session_key, "session:temp");
        assert_eq!(ctx.role, Role::Anonymous);
        assert_eq!(ctx.identity_id, "anonymous");
        assert_eq!(ctx.source_channel, "web");
        assert!(ctx.scope.is_none());
    }

    #[test]
    fn test_identity_context_unique_request_ids() {
        let ctx1 = IdentityContext::owner("s1".to_string(), "cli".to_string());
        let ctx2 = IdentityContext::owner("s1".to_string(), "cli".to_string());

        // Each context should have a unique request_id
        assert_ne!(ctx1.request_id, ctx2.request_id);
    }

    #[test]
    fn test_identity_context_serde() {
        let scope = GuestScope {
            allowed_tools: vec!["tool1".to_string()],
            expires_at: None,
            display_name: None,
        };

        let ctx = IdentityContext::guest(
            "session:1".to_string(),
            "guest1".to_string(),
            scope,
            "test".to_string(),
        );

        let json = serde_json::to_string(&ctx).unwrap();
        let parsed: IdentityContext = serde_json::from_str(&json).unwrap();

        assert_eq!(ctx.session_key, parsed.session_key);
        assert_eq!(ctx.role, parsed.role);
        assert_eq!(ctx.identity_id, parsed.identity_id);
    }
}
