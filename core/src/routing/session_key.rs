//! Enhanced session key with full context encoding.
//!
//! Session keys encode agent identity, channel, peer, and scope information
//! into a single hierarchical key for session lookup and persistence.

use serde::{Deserialize, Serialize};

/// DM session isolation strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DmScope {
    /// All DMs share main session
    Main,
    /// Per-user isolation (cross-channel)
    PerPeer,
    /// Per-channel per-user isolation
    PerChannelPeer,
}

impl Default for DmScope {
    fn default() -> Self {
        Self::PerPeer
    }
}

/// Peer type for group sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerKind {
    Group,
    Channel,
    Thread,
}

/// Enhanced session key with full context encoding
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKey {
    /// Main session (cross-channel shared)
    Main {
        agent_id: String,
        #[serde(default = "default_main_key")]
        main_key: String,
    },

    /// Direct message session with scope strategy
    DirectMessage {
        agent_id: String,
        channel: String,
        peer_id: String,
        #[serde(default)]
        dm_scope: DmScope,
    },

    /// Group/channel session
    Group {
        agent_id: String,
        channel: String,
        peer_kind: PeerKind,
        peer_id: String,
        /// Optional thread ID for nested conversations
        thread_id: Option<String>,
    },

    /// Task session (cron, webhook, scheduled)
    Task {
        agent_id: String,
        task_type: String,
        task_id: String,
    },

    /// Subagent session (nested under parent)
    Subagent {
        parent_key: Box<SessionKey>,
        subagent_id: String,
    },

    /// Ephemeral session (no persistence)
    Ephemeral {
        agent_id: String,
        ephemeral_id: String,
    },
}

fn default_main_key() -> String {
    "main".to_string()
}

/// Default agent ID constant
pub const DEFAULT_AGENT_ID: &str = "main";
/// Default main key constant
pub const DEFAULT_MAIN_KEY: &str = "main";

impl SessionKey {
    /// Create a main session key
    pub fn main(agent_id: impl Into<String>) -> Self {
        Self::Main {
            agent_id: normalize_agent_id(&agent_id.into()),
            main_key: DEFAULT_MAIN_KEY.to_string(),
        }
    }

    /// Create a DM session key with scope strategy
    ///
    /// If dm_scope is Main, returns a Main session key (DMs collapse into main).
    pub fn dm(
        agent_id: impl Into<String>,
        channel: impl Into<String>,
        peer_id: impl Into<String>,
        dm_scope: DmScope,
    ) -> Self {
        let agent_id = normalize_agent_id(&agent_id.into());
        match dm_scope {
            DmScope::Main => Self::Main {
                agent_id,
                main_key: DEFAULT_MAIN_KEY.to_string(),
            },
            _ => Self::DirectMessage {
                agent_id,
                channel: channel.into().trim().to_lowercase(),
                peer_id: peer_id.into().trim().to_lowercase(),
                dm_scope,
            },
        }
    }

    /// Create a group session key
    pub fn group(
        agent_id: impl Into<String>,
        channel: impl Into<String>,
        peer_kind: PeerKind,
        peer_id: impl Into<String>,
    ) -> Self {
        Self::Group {
            agent_id: normalize_agent_id(&agent_id.into()),
            channel: channel.into().trim().to_lowercase(),
            peer_kind,
            peer_id: peer_id.into().trim().to_lowercase(),
            thread_id: None,
        }
    }

    /// Create a group session key with thread
    pub fn group_thread(
        agent_id: impl Into<String>,
        channel: impl Into<String>,
        peer_kind: PeerKind,
        peer_id: impl Into<String>,
        thread_id: impl Into<String>,
    ) -> Self {
        Self::Group {
            agent_id: normalize_agent_id(&agent_id.into()),
            channel: channel.into().trim().to_lowercase(),
            peer_kind,
            peer_id: peer_id.into().trim().to_lowercase(),
            thread_id: Some(thread_id.into().trim().to_lowercase()),
        }
    }

    /// Create a task session key
    pub fn task(
        agent_id: impl Into<String>,
        task_type: impl Into<String>,
        task_id: impl Into<String>,
    ) -> Self {
        Self::Task {
            agent_id: normalize_agent_id(&agent_id.into()),
            task_type: task_type.into(),
            task_id: task_id.into(),
        }
    }

    /// Create an ephemeral session key
    pub fn ephemeral(agent_id: impl Into<String>) -> Self {
        Self::Ephemeral {
            agent_id: normalize_agent_id(&agent_id.into()),
            ephemeral_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Get the agent ID from this session key
    pub fn agent_id(&self) -> &str {
        match self {
            Self::Main { agent_id, .. } => agent_id,
            Self::DirectMessage { agent_id, .. } => agent_id,
            Self::Group { agent_id, .. } => agent_id,
            Self::Task { agent_id, .. } => agent_id,
            Self::Subagent { parent_key, .. } => parent_key.agent_id(),
            Self::Ephemeral { agent_id, .. } => agent_id,
        }
    }

    /// Get the main session key for this agent
    pub fn main_session_key(&self) -> SessionKey {
        Self::Main {
            agent_id: self.agent_id().to_string(),
            main_key: DEFAULT_MAIN_KEY.to_string(),
        }
    }
}

/// Normalize agent ID: lowercase, alphanumeric + dash/underscore, max 64 chars
pub fn normalize_agent_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return DEFAULT_AGENT_ID.to_string();
    }

    let normalized: String = trimmed
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let result = normalized
        .trim_start_matches('-')
        .trim_end_matches('-')
        .to_string();

    if result.is_empty() {
        DEFAULT_AGENT_ID.to_string()
    } else if result.len() > 64 {
        result[..64].to_string()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_main_constructor() {
        let key = SessionKey::main("main");
        assert_eq!(key.agent_id(), "main");
    }

    #[test]
    fn test_dm_per_peer() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerPeer);
        assert_eq!(key.agent_id(), "main");
        assert!(matches!(key, SessionKey::DirectMessage { .. }));
    }

    #[test]
    fn test_dm_main_scope_returns_main() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::Main);
        assert!(matches!(key, SessionKey::Main { .. }));
    }

    #[test]
    fn test_group_constructor() {
        let key = SessionKey::group("main", "discord", PeerKind::Group, "guild456");
        assert_eq!(key.agent_id(), "main");
        assert!(matches!(key, SessionKey::Group { .. }));
    }

    #[test]
    fn test_task_constructor() {
        let key = SessionKey::task("main", "cron", "daily-summary");
        assert_eq!(key.agent_id(), "main");
    }

    #[test]
    fn test_ephemeral_constructor() {
        let key = SessionKey::ephemeral("main");
        assert_eq!(key.agent_id(), "main");
        assert!(matches!(key, SessionKey::Ephemeral { .. }));
    }

    #[test]
    fn test_subagent_agent_id_delegates_to_parent() {
        let parent = SessionKey::main("main");
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "coding".to_string(),
        };
        assert_eq!(key.agent_id(), "main");
    }

    #[test]
    fn test_main_session_key_from_any() {
        let dm = SessionKey::dm("work", "telegram", "user1", DmScope::PerPeer);
        let main = dm.main_session_key();
        assert!(matches!(main, SessionKey::Main { agent_id, main_key } if agent_id == "work" && main_key == "main"));
    }
}
