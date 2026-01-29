//! Helper functions for session management tools.
//!
//! These helpers are used by sessions_list and sessions_send tools to
//! classify, format, parse, and extract information from session keys.

use serde::{Deserialize, Serialize};

use crate::routing::session_key::SessionKey;

/// Session kind classification for display and filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Main session (cross-channel shared)
    Main,
    /// Direct message session
    DirectMessage,
    /// Group/channel session
    Group,
    /// Task session (cron, webhook, scheduled)
    Task,
    /// Subagent session
    Subagent,
    /// Ephemeral session (no persistence)
    Ephemeral,
}

impl SessionKind {
    /// Returns the string representation of the session kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::DirectMessage => "dm",
            Self::Group => "group",
            Self::Task => "task",
            Self::Subagent => "subagent",
            Self::Ephemeral => "ephemeral",
        }
    }
}

impl std::fmt::Display for SessionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Classify a session key into its kind.
///
/// # Arguments
/// * `key` - The session key to classify
///
/// # Returns
/// The SessionKind enum variant corresponding to the session key type.
///
/// # Example
/// ```
/// use aethecore::routing::session_key::SessionKey;
/// use aethecore::builtin_tools::sessions::helpers::{classify_session_kind, SessionKind};
///
/// let key = SessionKey::main("main");
/// assert_eq!(classify_session_kind(&key), SessionKind::Main);
/// ```
pub fn classify_session_kind(key: &SessionKey) -> SessionKind {
    match key {
        SessionKey::Main { .. } => SessionKind::Main,
        SessionKey::DirectMessage { .. } => SessionKind::DirectMessage,
        SessionKey::Group { .. } => SessionKind::Group,
        SessionKey::Task { .. } => SessionKind::Task,
        SessionKey::Subagent { .. } => SessionKind::Subagent,
        SessionKey::Ephemeral { .. } => SessionKind::Ephemeral,
    }
}

/// Format a session key for display.
///
/// Uses the session key's built-in `to_key_string()` method.
///
/// # Arguments
/// * `key` - The session key to format
///
/// # Returns
/// A string representation suitable for display and storage.
///
/// # Example
/// ```
/// use aethecore::routing::session_key::SessionKey;
/// use aethecore::builtin_tools::sessions::helpers::resolve_display_key;
///
/// let key = SessionKey::main("main");
/// assert_eq!(resolve_display_key(&key), "agent:main:main");
/// ```
pub fn resolve_display_key(key: &SessionKey) -> String {
    key.to_key_string()
}

/// Parse a session key from its display format.
///
/// Wraps the session key's built-in `parse()` method with better error handling.
///
/// # Arguments
/// * `display` - The display string to parse
///
/// # Returns
/// * `Ok(SessionKey)` if parsing succeeded
/// * `Err(String)` with an error message if parsing failed
///
/// # Example
/// ```
/// use aethecore::routing::session_key::SessionKey;
/// use aethecore::builtin_tools::sessions::helpers::parse_session_key;
///
/// let key = parse_session_key("agent:main:main").unwrap();
/// assert!(matches!(key, SessionKey::Main { .. }));
/// ```
pub fn parse_session_key(display: &str) -> Result<SessionKey, String> {
    let trimmed = display.trim();
    if trimmed.is_empty() {
        return Err("Empty session key string".to_string());
    }

    SessionKey::parse(trimmed)
        .ok_or_else(|| format!("Invalid session key format: '{}'", trimmed))
}

/// Extract the channel name from a session key.
///
/// For session types that have a channel (DirectMessage, Group), returns the channel name.
/// For other session types, returns "unknown".
///
/// # Arguments
/// * `key` - The session key to extract the channel from
///
/// # Returns
/// The channel name (e.g., "telegram", "discord") or "unknown" if not applicable.
///
/// # Example
/// ```
/// use aethecore::routing::session_key::{SessionKey, DmScope};
/// use aethecore::builtin_tools::sessions::helpers::derive_channel;
///
/// let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerChannelPeer);
/// assert_eq!(derive_channel(&key), "telegram");
///
/// let main_key = SessionKey::main("main");
/// assert_eq!(derive_channel(&main_key), "unknown");
/// ```
pub fn derive_channel(key: &SessionKey) -> String {
    match key {
        SessionKey::DirectMessage { channel, .. } => {
            if channel.is_empty() {
                "unknown".to_string()
            } else {
                channel.clone()
            }
        }
        SessionKey::Group { channel, .. } => channel.clone(),
        SessionKey::Main { .. } => "unknown".to_string(),
        SessionKey::Task { .. } => "unknown".to_string(),
        SessionKey::Subagent { parent_key, .. } => derive_channel(parent_key),
        SessionKey::Ephemeral { .. } => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::{DmScope, PeerKind};

    // ============================================================================
    // classify_session_kind tests
    // ============================================================================

    #[test]
    fn test_classify_main() {
        let key = SessionKey::main("main");
        assert_eq!(classify_session_kind(&key), SessionKind::Main);
    }

    #[test]
    fn test_classify_direct_message() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerPeer);
        assert_eq!(classify_session_kind(&key), SessionKind::DirectMessage);
    }

    #[test]
    fn test_classify_group() {
        let key = SessionKey::group("main", "discord", PeerKind::Group, "guild456");
        assert_eq!(classify_session_kind(&key), SessionKind::Group);
    }

    #[test]
    fn test_classify_task() {
        let key = SessionKey::task("main", "cron", "daily-summary");
        assert_eq!(classify_session_kind(&key), SessionKind::Task);
    }

    #[test]
    fn test_classify_subagent() {
        let parent = SessionKey::main("main");
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "translator".to_string(),
        };
        assert_eq!(classify_session_kind(&key), SessionKind::Subagent);
    }

    #[test]
    fn test_classify_ephemeral() {
        let key = SessionKey::ephemeral("main");
        assert_eq!(classify_session_kind(&key), SessionKind::Ephemeral);
    }

    // ============================================================================
    // resolve_display_key tests
    // ============================================================================

    #[test]
    fn test_resolve_display_key_main() {
        let key = SessionKey::main("main");
        assert_eq!(resolve_display_key(&key), "agent:main:main");
    }

    #[test]
    fn test_resolve_display_key_dm() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerChannelPeer);
        assert_eq!(resolve_display_key(&key), "agent:main:telegram:dm:user123");
    }

    #[test]
    fn test_resolve_display_key_group() {
        let key = SessionKey::group("main", "discord", PeerKind::Channel, "channel123");
        assert_eq!(resolve_display_key(&key), "agent:main:discord:channel:channel123");
    }

    #[test]
    fn test_resolve_display_key_task() {
        let key = SessionKey::task("main", "webhook", "hook-1");
        assert_eq!(resolve_display_key(&key), "agent:main:webhook:hook-1");
    }

    // ============================================================================
    // parse_session_key tests
    // ============================================================================

    #[test]
    fn test_parse_session_key_main() {
        let result = parse_session_key("agent:main:main");
        assert!(result.is_ok());
        let key = result.unwrap();
        assert_eq!(classify_session_kind(&key), SessionKind::Main);
    }

    #[test]
    fn test_parse_session_key_dm_per_peer() {
        let result = parse_session_key("agent:main:dm:user123");
        assert!(result.is_ok());
        let key = result.unwrap();
        assert_eq!(classify_session_kind(&key), SessionKind::DirectMessage);
    }

    #[test]
    fn test_parse_session_key_dm_per_channel_peer() {
        let result = parse_session_key("agent:main:telegram:dm:user123");
        assert!(result.is_ok());
        let key = result.unwrap();
        assert_eq!(classify_session_kind(&key), SessionKind::DirectMessage);
    }

    #[test]
    fn test_parse_session_key_group() {
        let result = parse_session_key("agent:main:discord:group:guild456");
        assert!(result.is_ok());
        let key = result.unwrap();
        assert_eq!(classify_session_kind(&key), SessionKind::Group);
    }

    #[test]
    fn test_parse_session_key_task() {
        let result = parse_session_key("agent:main:cron:daily");
        assert!(result.is_ok());
        let key = result.unwrap();
        assert_eq!(classify_session_kind(&key), SessionKind::Task);
    }

    #[test]
    fn test_parse_session_key_ephemeral() {
        let result = parse_session_key("agent:main:ephemeral:uuid-123");
        assert!(result.is_ok());
        let key = result.unwrap();
        assert_eq!(classify_session_kind(&key), SessionKind::Ephemeral);
    }

    #[test]
    fn test_parse_session_key_empty() {
        let result = parse_session_key("");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Empty session key string");
    }

    #[test]
    fn test_parse_session_key_whitespace() {
        let result = parse_session_key("   ");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Empty session key string");
    }

    #[test]
    fn test_parse_session_key_invalid() {
        let result = parse_session_key("invalid");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid session key format"));
    }

    #[test]
    fn test_parse_session_key_with_whitespace() {
        let result = parse_session_key("  agent:main:main  ");
        assert!(result.is_ok());
    }

    // ============================================================================
    // derive_channel tests
    // ============================================================================

    #[test]
    fn test_derive_channel_dm_with_channel() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerChannelPeer);
        assert_eq!(derive_channel(&key), "telegram");
    }

    #[test]
    fn test_derive_channel_dm_per_peer_empty_channel() {
        // PerPeer DMs have empty channel in storage
        let key = SessionKey::DirectMessage {
            agent_id: "main".to_string(),
            channel: String::new(),
            peer_id: "user123".to_string(),
            dm_scope: DmScope::PerPeer,
        };
        assert_eq!(derive_channel(&key), "unknown");
    }

    #[test]
    fn test_derive_channel_group() {
        let key = SessionKey::group("main", "discord", PeerKind::Group, "guild456");
        assert_eq!(derive_channel(&key), "discord");
    }

    #[test]
    fn test_derive_channel_main() {
        let key = SessionKey::main("main");
        assert_eq!(derive_channel(&key), "unknown");
    }

    #[test]
    fn test_derive_channel_task() {
        let key = SessionKey::task("main", "cron", "daily-summary");
        assert_eq!(derive_channel(&key), "unknown");
    }

    #[test]
    fn test_derive_channel_ephemeral() {
        let key = SessionKey::ephemeral("main");
        assert_eq!(derive_channel(&key), "unknown");
    }

    #[test]
    fn test_derive_channel_subagent_inherits_from_parent() {
        let parent = SessionKey::group("main", "slack", PeerKind::Channel, "C123");
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "translator".to_string(),
        };
        assert_eq!(derive_channel(&key), "slack");
    }

    #[test]
    fn test_derive_channel_nested_subagent() {
        let grandparent = SessionKey::dm("main", "telegram", "user", DmScope::PerChannelPeer);
        let parent = SessionKey::Subagent {
            parent_key: Box::new(grandparent),
            subagent_id: "level1".to_string(),
        };
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "level2".to_string(),
        };
        assert_eq!(derive_channel(&key), "telegram");
    }

    // ============================================================================
    // SessionKind tests
    // ============================================================================

    #[test]
    fn test_session_kind_as_str() {
        assert_eq!(SessionKind::Main.as_str(), "main");
        assert_eq!(SessionKind::DirectMessage.as_str(), "dm");
        assert_eq!(SessionKind::Group.as_str(), "group");
        assert_eq!(SessionKind::Task.as_str(), "task");
        assert_eq!(SessionKind::Subagent.as_str(), "subagent");
        assert_eq!(SessionKind::Ephemeral.as_str(), "ephemeral");
    }

    #[test]
    fn test_session_kind_display() {
        assert_eq!(format!("{}", SessionKind::Main), "main");
        assert_eq!(format!("{}", SessionKind::DirectMessage), "dm");
    }

    // ============================================================================
    // Roundtrip tests
    // ============================================================================

    #[test]
    fn test_roundtrip_parse_display() {
        let test_cases = vec![
            "agent:main:main",
            "agent:work:custom",
            "agent:main:dm:user123",
            "agent:main:telegram:dm:user456",
            "agent:main:discord:group:guild789",
            "agent:main:slack:channel:c123",
            "agent:main:cron:daily",
            "agent:main:webhook:hook-1",
            "agent:main:ephemeral:uuid-abc",
        ];

        for display in test_cases {
            let parsed = parse_session_key(display).expect(&format!("Failed to parse: {}", display));
            let reparsed = resolve_display_key(&parsed);
            assert_eq!(reparsed, display, "Roundtrip failed for: {}", display);
        }
    }
}
