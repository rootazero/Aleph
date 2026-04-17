//! Enhanced session key with full context encoding.
//!
//! Session keys encode agent identity, channel, peer, and scope information
//! into a single hierarchical key for session lookup and persistence.

use serde::{Deserialize, Serialize};
use std::fmt;

/// DM session isolation strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum DmScope {
    /// All DMs share main session
    Main,
    /// Per-user isolation (cross-channel)
    #[default]
    PerPeer,
    /// Per-channel per-user isolation
    PerChannelPeer,
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
        #[serde(default)]
        epoch: u32,
    },

    /// Direct message session with scope strategy
    DirectMessage {
        agent_id: String,
        channel: String,
        peer_id: String,
        #[serde(default)]
        dm_scope: DmScope,
        #[serde(default)]
        epoch: u32,
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
            epoch: 0,
        }
    }

    /// Create a per-peer DM session key (legacy compatibility alias).
    pub fn peer(agent_id: impl Into<String>, peer_id: impl Into<String>) -> Self {
        Self::dm(agent_id, "", peer_id, DmScope::PerPeer)
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
                epoch: 0,
            },
            _ => Self::DirectMessage {
                agent_id,
                channel: channel.into().trim().to_lowercase(),
                peer_id: peer_id.into().trim().to_lowercase(),
                dm_scope,
                epoch: 0,
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
            task_type: normalize_agent_id(&task_type.into()),
            task_id: normalize_agent_id(&task_id.into()),
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
            epoch: 0,
        }
    }

    /// Get the epoch of this session key (only Main and DirectMessage have epochs)
    pub fn epoch(&self) -> u32 {
        match self {
            Self::Main { epoch, .. } | Self::DirectMessage { epoch, .. } => *epoch,
            _ => 0,
        }
    }

    /// Return a clone with the given epoch (legacy compatibility alias).
    pub fn with_epoch(&self, epoch: u32) -> Self {
        match self {
            Self::Main {
                agent_id, main_key, ..
            } => Self::Main {
                agent_id: agent_id.clone(),
                main_key: main_key.clone(),
                epoch,
            },
            Self::DirectMessage {
                agent_id,
                channel,
                peer_id,
                dm_scope,
                ..
            } => Self::DirectMessage {
                agent_id: agent_id.clone(),
                channel: channel.clone(),
                peer_id: peer_id.clone(),
                dm_scope: *dm_scope,
                epoch,
            },
            other => other.clone(),
        }
    }

    /// Return a clone with epoch incremented by 1.
    /// For non-epoch types (Group, Task, Subagent, Ephemeral), returns clone unchanged.
    pub fn with_next_epoch(&self) -> Self {
        match self {
            Self::Main {
                agent_id,
                main_key,
                epoch,
            } => Self::Main {
                agent_id: agent_id.clone(),
                main_key: main_key.clone(),
                epoch: epoch + 1,
            },
            Self::DirectMessage {
                agent_id,
                channel,
                peer_id,
                dm_scope,
                epoch,
            } => Self::DirectMessage {
                agent_id: agent_id.clone(),
                channel: channel.clone(),
                peer_id: peer_id.clone(),
                dm_scope: *dm_scope,
                epoch: epoch + 1,
            },
            other => other.clone(),
        }
    }

    /// Return the base key string WITHOUT epoch suffix (for SQL LIKE queries).
    pub fn base_key_pattern(&self) -> String {
        match self {
            Self::Main {
                agent_id, main_key, ..
            } => {
                format!("agent:{}:{}", agent_id, main_key)
            }
            Self::DirectMessage {
                agent_id,
                channel,
                peer_id,
                dm_scope,
                ..
            } => match dm_scope {
                DmScope::Main => format!("agent:{}:main", agent_id),
                DmScope::PerPeer if channel.is_empty() => {
                    format!("agent:{}:peer:{}", agent_id, peer_id)
                }
                DmScope::PerPeer => format!("agent:{}:dm:{}", agent_id, peer_id),
                DmScope::PerChannelPeer => {
                    format!("agent:{}:{}:dm:{}", agent_id, channel, peer_id)
                }
            },
            // Non-epoch types: base_key_pattern == to_key_string
            _ => self.to_key_string(),
        }
    }

    /// Serialize to string key for storage/lookup
    pub fn to_key_string(&self) -> String {
        match self {
            Self::Main {
                agent_id,
                main_key,
                epoch,
            } => {
                let base = format!("agent:{}:{}", agent_id, main_key);
                if *epoch > 0 {
                    format!("{}:s{}", base, epoch)
                } else {
                    base
                }
            }
            Self::DirectMessage {
                agent_id,
                channel,
                peer_id,
                dm_scope,
                epoch,
            } => {
                let base = match dm_scope {
                    DmScope::Main => format!("agent:{}:main", agent_id),
                    DmScope::PerPeer if channel.is_empty() => {
                        format!("agent:{}:peer:{}", agent_id, peer_id)
                    }
                    DmScope::PerPeer => format!("agent:{}:dm:{}", agent_id, peer_id),
                    DmScope::PerChannelPeer => {
                        format!("agent:{}:{}:dm:{}", agent_id, channel, peer_id)
                    }
                };
                if *epoch > 0 {
                    format!("{}:s{}", base, epoch)
                } else {
                    base
                }
            }
            Self::Group {
                agent_id,
                channel,
                peer_kind,
                peer_id,
                thread_id,
            } => {
                let kind = match peer_kind {
                    PeerKind::Group => "group",
                    PeerKind::Channel => "channel",
                    PeerKind::Thread => "thread",
                };
                match thread_id {
                    Some(tid) => format!(
                        "agent:{}:{}:{}:{}:thread:{}",
                        agent_id, channel, kind, peer_id, tid
                    ),
                    None => format!("agent:{}:{}:{}:{}", agent_id, channel, kind, peer_id),
                }
            }
            Self::Task {
                agent_id,
                task_type,
                task_id,
            } => {
                format!("agent:{}:{}:{}", agent_id, task_type, task_id)
            }
            Self::Subagent {
                parent_key,
                subagent_id,
            } => {
                format!("{}:subagent:{}", parent_key.to_key_string(), subagent_id)
            }
            Self::Ephemeral {
                agent_id,
                ephemeral_id,
            } => {
                format!("agent:{}:ephemeral:{}", agent_id, ephemeral_id)
            }
        }
    }

    /// Parse a session key from a string
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || parts[0] != "agent" {
            return None;
        }

        let agent_id = normalize_agent_id(parts[1]);
        if agent_id.is_empty() {
            return None;
        }

        let rest = &parts[2..];

        // Check if the last segment is an epoch suffix (pattern: sN where N is a number)
        let (rest, epoch) = if let Some(last) = rest.last() {
            if let Some(n_str) = last.strip_prefix('s') {
                if let Ok(n) = n_str.parse::<u32>() {
                    if n > 0 {
                        (&rest[..rest.len() - 1], n)
                    } else {
                        (rest, 0)
                    }
                } else {
                    (rest, 0)
                }
            } else {
                (rest, 0)
            }
        } else {
            (rest, 0)
        };

        match rest {
            // agent:id:peer:peer_id (legacy per-peer DM format)
            ["peer", peer_id] => Some(Self::DirectMessage {
                agent_id,
                channel: String::new(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerPeer,
                epoch,
            }),

            // agent:id:dm:peer (per-peer DM)
            ["dm", peer_id] => Some(Self::DirectMessage {
                agent_id,
                channel: String::new(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerPeer,
                epoch,
            }),

            // agent:id:channel:dm:peer (per-channel-peer DM)
            [channel, "dm", peer_id] => Some(Self::DirectMessage {
                agent_id,
                channel: channel.to_string(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerChannelPeer,
                epoch,
            }),

            // agent:id:channel:group:peer:thread:tid
            [channel, "group", peer_id, "thread", thread_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Group,
                peer_id: peer_id.to_string(),
                thread_id: Some(thread_id.to_string()),
            }),

            // agent:id:channel:channel:peer:thread:tid
            [channel, "channel", peer_id, "thread", thread_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Channel,
                peer_id: peer_id.to_string(),
                thread_id: Some(thread_id.to_string()),
            }),

            // agent:id:channel:thread:peer:thread:tid
            [channel, "thread", peer_id, "thread", thread_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Thread,
                peer_id: peer_id.to_string(),
                thread_id: Some(thread_id.to_string()),
            }),

            // agent:id:channel:group:peer
            [channel, "group", peer_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Group,
                peer_id: peer_id.to_string(),
                thread_id: None,
            }),

            // agent:id:channel:channel:peer
            [channel, "channel", peer_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Channel,
                peer_id: peer_id.to_string(),
                thread_id: None,
            }),

            // agent:id:channel:thread:peer
            [channel, "thread", peer_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Thread,
                peer_id: peer_id.to_string(),
                thread_id: None,
            }),

            // agent:id:cron|webhook|scheduled:task_id
            [task_type @ ("cron" | "webhook" | "scheduled"), task_id] => Some(Self::Task {
                agent_id,
                task_type: task_type.to_string(),
                task_id: task_id.to_string(),
            }),

            // agent:id:ephemeral:uuid
            ["ephemeral", ephemeral_id] => Some(Self::Ephemeral {
                agent_id,
                ephemeral_id: ephemeral_id.to_string(),
            }),

            // agent:id:main (or any single token as main_key)
            [main_key] => Some(Self::Main {
                agent_id,
                main_key: main_key.to_string(),
                epoch,
            }),

            _ => None,
        }
    }

    /// Alias for `parse` with legacy fallback to match gateway/router.rs behavior.
    pub fn from_key_string(s: &str) -> Option<Self> {
        Self::parse(s).or_else(|| Self::from_legacy(s))
    }

    /// Parse legacy format from gateway/router.rs for backward compatibility
    pub fn from_legacy(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || parts[0] != "agent" {
            return None;
        }

        let agent_id = parts[1].to_string();

        match parts.get(2..) {
            Some(&["peer", ref rest @ ..]) if !rest.is_empty() => Some(Self::DirectMessage {
                agent_id,
                channel: String::new(),
                peer_id: rest.join(":"),
                dm_scope: DmScope::PerPeer,
                epoch: 0,
            }),
            Some(&["ephemeral", ephemeral_id]) => Some(Self::Ephemeral {
                agent_id,
                ephemeral_id: ephemeral_id.to_string(),
            }),
            _ => Self::parse(s),
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
        // Safe: all chars are ASCII after the filter above, so byte[64] is always a char boundary.
        // Using chars().take() for defensive correctness against future filter changes.
        result.chars().take(64).collect()
    } else {
        result
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_key_string())
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
        assert!(
            matches!(main, SessionKey::Main { agent_id, main_key, .. } if agent_id == "work" && main_key == "main")
        );
    }

    // --- Serialization tests ---

    #[test]
    fn test_to_key_string_main() {
        let key = SessionKey::main("main");
        assert_eq!(key.to_key_string(), "agent:main:main");
    }

    #[test]
    fn test_to_key_string_dm_per_peer() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerPeer);
        assert_eq!(key.to_key_string(), "agent:main:dm:user123");
    }

    #[test]
    fn test_to_key_string_peer() {
        let key = SessionKey::peer("main", "user123");
        assert_eq!(key.to_key_string(), "agent:main:peer:user123");
    }

    #[test]
    fn test_to_key_string_dm_per_channel_peer() {
        let key = SessionKey::dm("main", "telegram", "user123", DmScope::PerChannelPeer);
        assert_eq!(key.to_key_string(), "agent:main:telegram:dm:user123");
    }

    #[test]
    fn test_to_key_string_group() {
        let key = SessionKey::group("main", "discord", PeerKind::Group, "guild456");
        assert_eq!(key.to_key_string(), "agent:main:discord:group:guild456");
    }

    #[test]
    fn test_to_key_string_group_channel() {
        let key = SessionKey::group("main", "slack", PeerKind::Channel, "C123");
        assert_eq!(key.to_key_string(), "agent:main:slack:channel:c123");
    }

    #[test]
    fn test_to_key_string_group_thread() {
        let key = SessionKey::group_thread("main", "telegram", PeerKind::Group, "chat789", "t1");
        assert_eq!(
            key.to_key_string(),
            "agent:main:telegram:group:chat789:thread:t1"
        );
    }

    #[test]
    fn test_to_key_string_task() {
        let key = SessionKey::task("main", "cron", "daily-summary");
        assert_eq!(key.to_key_string(), "agent:main:cron:daily-summary");
    }

    #[test]
    fn test_to_key_string_subagent() {
        let parent = SessionKey::main("main");
        let key = SessionKey::Subagent {
            parent_key: Box::new(parent),
            subagent_id: "coding".to_string(),
        };
        assert_eq!(key.to_key_string(), "agent:main:main:subagent:coding");
    }

    // --- Parse tests ---

    #[test]
    fn test_parse_main() {
        let key = SessionKey::parse("agent:main:main").unwrap();
        assert!(
            matches!(key, SessionKey::Main { agent_id, main_key, .. } if agent_id == "main" && main_key == "main")
        );
    }

    #[test]
    fn test_parse_dm_per_peer() {
        let key = SessionKey::parse("agent:main:dm:user123").unwrap();
        assert!(
            matches!(key, SessionKey::DirectMessage { peer_id, dm_scope: DmScope::PerPeer, .. } if peer_id == "user123")
        );
    }

    #[test]
    fn test_parse_peer_legacy() {
        let key = SessionKey::parse("agent:main:peer:user123").unwrap();
        assert!(
            matches!(key, SessionKey::DirectMessage { peer_id, dm_scope: DmScope::PerPeer, channel, .. } if peer_id == "user123" && channel.is_empty())
        );
    }

    #[test]
    fn test_parse_dm_per_channel_peer() {
        let key = SessionKey::parse("agent:main:telegram:dm:user123").unwrap();
        assert!(
            matches!(key, SessionKey::DirectMessage { channel, peer_id, dm_scope: DmScope::PerChannelPeer, .. } if channel == "telegram" && peer_id == "user123")
        );
    }

    #[test]
    fn test_parse_group() {
        let key = SessionKey::parse("agent:main:discord:group:guild456").unwrap();
        assert!(
            matches!(key, SessionKey::Group { channel, peer_kind: PeerKind::Group, peer_id, .. } if channel == "discord" && peer_id == "guild456")
        );
    }

    #[test]
    fn test_parse_group_thread() {
        let key = SessionKey::parse("agent:main:telegram:group:chat789:thread:t1").unwrap();
        assert!(matches!(key, SessionKey::Group { thread_id: Some(tid), .. } if tid == "t1"));
    }

    #[test]
    fn test_parse_task() {
        let key = SessionKey::parse("agent:main:cron:daily").unwrap();
        assert!(
            matches!(key, SessionKey::Task { task_type, task_id, .. } if task_type == "cron" && task_id == "daily")
        );
    }

    #[test]
    fn test_parse_ephemeral() {
        let key = SessionKey::parse("agent:main:ephemeral:abc-123").unwrap();
        assert!(
            matches!(key, SessionKey::Ephemeral { ephemeral_id, .. } if ephemeral_id == "abc-123")
        );
    }

    #[test]
    fn test_parse_invalid() {
        assert!(SessionKey::parse("invalid").is_none());
        assert!(SessionKey::parse("agent:").is_none());
        assert!(SessionKey::parse("").is_none());
    }

    #[test]
    fn test_roundtrip() {
        let keys = vec![
            SessionKey::main("work"),
            SessionKey::peer("main", "user1"),
            SessionKey::dm("main", "discord", "user2", DmScope::PerChannelPeer),
            SessionKey::group("main", "slack", PeerKind::Channel, "C123"),
            SessionKey::task("main", "webhook", "hook-1"),
        ];
        for key in keys {
            let s = key.to_key_string();
            let parsed = SessionKey::parse(&s).unwrap_or_else(|| panic!("Failed to parse: {}", s));
            assert_eq!(parsed.to_key_string(), s, "Roundtrip failed for: {}", s);
        }
    }

    // --- Epoch tests ---

    #[test]
    fn test_main_with_epoch() {
        let key = SessionKey::Main {
            agent_id: "main".to_string(),
            main_key: "main".to_string(),
            epoch: 2,
        };
        assert_eq!(key.to_key_string(), "agent:main:main:s2");
        assert_eq!(key.epoch(), 2);
    }

    #[test]
    fn test_main_epoch_zero_no_suffix() {
        let key = SessionKey::main("main");
        assert_eq!(key.to_key_string(), "agent:main:main");
        assert_eq!(key.epoch(), 0);
    }

    #[test]
    fn test_parse_with_epoch() {
        let key = SessionKey::parse("agent:main:main:s3").unwrap();
        assert_eq!(key.epoch(), 3);
    }

    #[test]
    fn test_parse_without_epoch_defaults_zero() {
        let key = SessionKey::parse("agent:main:main").unwrap();
        assert_eq!(key.epoch(), 0);
    }

    #[test]
    fn test_dm_with_epoch_roundtrip() {
        let key = SessionKey::DirectMessage {
            agent_id: "main".to_string(),
            channel: String::new(),
            peer_id: "user123".to_string(),
            dm_scope: DmScope::PerPeer,
            epoch: 1,
        };
        assert_eq!(key.to_key_string(), "agent:main:peer:user123:s1");
        let parsed = SessionKey::parse(&key.to_key_string()).unwrap();
        assert_eq!(parsed.epoch(), 1);
    }

    #[test]
    fn test_next_epoch() {
        let key = SessionKey::main("main");
        let next = key.with_next_epoch();
        assert_eq!(next.epoch(), 1);
        assert_eq!(next.to_key_string(), "agent:main:main:s1");
        let next2 = next.with_next_epoch();
        assert_eq!(next2.epoch(), 2);
    }

    #[test]
    fn test_base_key_pattern() {
        let key = SessionKey::Main {
            agent_id: "main".to_string(),
            main_key: "main".to_string(),
            epoch: 3,
        };
        assert_eq!(key.base_key_pattern(), "agent:main:main");
    }

    #[test]
    fn test_epoch_roundtrip_all_types() {
        let keys_with_epoch = vec![
            SessionKey::Main {
                agent_id: "work".to_string(),
                main_key: "main".to_string(),
                epoch: 5,
            },
            SessionKey::DirectMessage {
                agent_id: "main".to_string(),
                channel: "telegram".to_string(),
                peer_id: "u1".to_string(),
                dm_scope: DmScope::PerChannelPeer,
                epoch: 2,
            },
        ];
        for key in keys_with_epoch {
            let s = key.to_key_string();
            let parsed = SessionKey::parse(&s).unwrap_or_else(|| panic!("Failed to parse: {}", s));
            assert_eq!(
                parsed.epoch(),
                key.epoch(),
                "Epoch roundtrip failed for: {}",
                s
            );
        }
    }
}
