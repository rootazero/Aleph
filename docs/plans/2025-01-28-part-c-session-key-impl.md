# Part C: Session Key Enhancement - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the simple 4-variant SessionKey with a channel-aware, DM-scope-supporting, identity-links-enabled routing system.

**Architecture:** Create a new `routing` module at `core/src/routing/` that replaces the SessionKey and AgentRouter from `core/src/gateway/router.rs`. The new module adds channel context, DM scope strategies, identity links, and hierarchical route binding resolution (peer → guild → team → account → channel → default).

**Tech Stack:** Rust, serde, regex (for ID normalization), HashMap (identity links), TOML config

---

### Task 1: Create routing module with new SessionKey

**Files:**
- Create: `core/src/routing/mod.rs`
- Create: `core/src/routing/session_key.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create `core/src/routing/mod.rs`**

```rust
//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route resolution.

pub mod session_key;

pub use session_key::{DmScope, PeerKind, SessionKey};
```

**Step 2: Create `core/src/routing/session_key.rs` with types only (no methods yet)**

```rust
//! Enhanced session key with full context encoding.
//!
//! Session keys encode agent identity, channel, peer, and scope information
//! into a single hierarchical key for session lookup and persistence.

use serde::{Deserialize, Serialize};
use std::fmt;

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
```

**Step 3: Add routing module to `core/src/lib.rs`**

Add this line after `pub mod tools;` (line 102) and before `pub mod uniffi_core;` (line 103):

```rust
pub mod routing; // NEW: Channel-aware routing and session key system
```

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo check 2>&1 | head -30`
Expected: Compiles with no errors (no consumers yet)

**Step 5: Commit**

```bash
git add core/src/routing/mod.rs core/src/routing/session_key.rs core/src/lib.rs
git commit -m "routing: add session key types with channel-aware variants"
```

---

### Task 2: Add SessionKey constructors and agent_id accessor

**Files:**
- Modify: `core/src/routing/session_key.rs`

**Step 1: Write tests at the bottom of session_key.rs**

```rust
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
```

**Step 2: Run tests to verify they fail**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib routing::session_key::tests 2>&1 | tail -20`
Expected: FAIL - methods not defined

**Step 3: Implement constructors and agent_id**

Add this `impl` block before the `#[cfg(test)]` section:

```rust
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
```

**Step 4: Run tests to verify they pass**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib routing::session_key::tests 2>&1 | tail -20`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/routing/session_key.rs
git commit -m "routing: add SessionKey constructors and agent_id accessor"
```

---

### Task 3: Add SessionKey serialization (to_key_string / parse)

**Files:**
- Modify: `core/src/routing/session_key.rs`

**Step 1: Add serialization tests**

Append to the `mod tests` block:

```rust
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
        assert_eq!(key.to_key_string(), "agent:main:telegram:group:chat789:thread:t1");
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
        assert!(matches!(key, SessionKey::Main { agent_id, main_key } if agent_id == "main" && main_key == "main"));
    }

    #[test]
    fn test_parse_dm_per_peer() {
        let key = SessionKey::parse("agent:main:dm:user123").unwrap();
        assert!(matches!(key, SessionKey::DirectMessage { peer_id, dm_scope: DmScope::PerPeer, .. } if peer_id == "user123"));
    }

    #[test]
    fn test_parse_dm_per_channel_peer() {
        let key = SessionKey::parse("agent:main:telegram:dm:user123").unwrap();
        assert!(matches!(key, SessionKey::DirectMessage { channel, peer_id, dm_scope: DmScope::PerChannelPeer, .. } if channel == "telegram" && peer_id == "user123"));
    }

    #[test]
    fn test_parse_group() {
        let key = SessionKey::parse("agent:main:discord:group:guild456").unwrap();
        assert!(matches!(key, SessionKey::Group { channel, peer_kind: PeerKind::Group, peer_id, .. } if channel == "discord" && peer_id == "guild456"));
    }

    #[test]
    fn test_parse_group_thread() {
        let key = SessionKey::parse("agent:main:telegram:group:chat789:thread:t1").unwrap();
        assert!(matches!(key, SessionKey::Group { thread_id: Some(tid), .. } if tid == "t1"));
    }

    #[test]
    fn test_parse_task() {
        let key = SessionKey::parse("agent:main:cron:daily").unwrap();
        assert!(matches!(key, SessionKey::Task { task_type, task_id, .. } if task_type == "cron" && task_id == "daily"));
    }

    #[test]
    fn test_parse_ephemeral() {
        let key = SessionKey::parse("agent:main:ephemeral:abc-123").unwrap();
        assert!(matches!(key, SessionKey::Ephemeral { ephemeral_id, .. } if ephemeral_id == "abc-123"));
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
            SessionKey::dm("main", "telegram", "user1", DmScope::PerPeer),
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
```

**Step 2: Run tests to verify they fail**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib routing::session_key::tests 2>&1 | tail -30`
Expected: FAIL - to_key_string and parse not defined

**Step 3: Implement to_key_string and parse**

Add to the `impl SessionKey` block:

```rust
    /// Serialize to string key for storage/lookup
    pub fn to_key_string(&self) -> String {
        match self {
            Self::Main { agent_id, main_key } => {
                format!("agent:{}:{}", agent_id, main_key)
            }
            Self::DirectMessage {
                agent_id,
                channel,
                peer_id,
                dm_scope,
            } => match dm_scope {
                DmScope::Main => format!("agent:{}:main", agent_id),
                DmScope::PerPeer => format!("agent:{}:dm:{}", agent_id, peer_id),
                DmScope::PerChannelPeer => {
                    format!("agent:{}:{}:dm:{}", agent_id, channel, peer_id)
                }
            },
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

        match rest {
            // agent:id:dm:peer (per-peer DM)
            ["dm", peer_id] => Some(Self::DirectMessage {
                agent_id,
                channel: String::new(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerPeer,
            }),

            // agent:id:channel:dm:peer (per-channel-peer DM)
            [channel, "dm", peer_id] => Some(Self::DirectMessage {
                agent_id,
                channel: channel.to_string(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerChannelPeer,
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
            }),

            _ => None,
        }
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
            }),
            Some(&["ephemeral", ephemeral_id]) => Some(Self::Ephemeral {
                agent_id,
                ephemeral_id: ephemeral_id.to_string(),
            }),
            _ => Self::parse(s),
        }
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_key_string())
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib routing::session_key::tests 2>&1 | tail -30`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/routing/session_key.rs
git commit -m "routing: add SessionKey serialization and parsing"
```

---

### Task 4: Add identity links resolution

**Files:**
- Create: `core/src/routing/identity_links.rs`
- Modify: `core/src/routing/mod.rs`

**Step 1: Create `core/src/routing/identity_links.rs` with tests**

```rust
//! Cross-channel user identity linking.
//!
//! Maps user IDs across channels to a canonical identity,
//! allowing sessions to be shared across platforms.

use std::collections::HashMap;

/// Resolve a peer ID to its canonical identity via identity links.
///
/// Checks both bare peer ID and channel-scoped peer ID.
/// Returns the canonical name if a link is found, None otherwise.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use alephcore::routing::identity_links::resolve_linked_peer_id;
///
/// let mut links = HashMap::new();
/// links.insert("john".to_string(), vec!["telegram:123".to_string(), "discord:456".to_string()]);
///
/// assert_eq!(resolve_linked_peer_id(&links, "telegram", "123"), Some("john".to_string()));
/// assert_eq!(resolve_linked_peer_id(&links, "slack", "999"), None);
/// ```
pub fn resolve_linked_peer_id(
    identity_links: &HashMap<String, Vec<String>>,
    channel: &str,
    peer_id: &str,
) -> Option<String> {
    let peer_lower = peer_id.trim().to_lowercase();
    if peer_lower.is_empty() {
        return None;
    }

    let channel_lower = channel.trim().to_lowercase();
    let scoped = if channel_lower.is_empty() {
        None
    } else {
        Some(format!("{}:{}", channel_lower, peer_lower))
    };

    for (canonical, ids) in identity_links {
        let canonical_name = canonical.trim();
        if canonical_name.is_empty() {
            continue;
        }

        for id in ids {
            let id_lower = id.trim().to_lowercase();
            if id_lower.is_empty() {
                continue;
            }

            if id_lower == peer_lower {
                return Some(canonical_name.to_string());
            }

            if let Some(ref scoped_id) = scoped {
                if &id_lower == scoped_id {
                    return Some(canonical_name.to_string());
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_links() -> HashMap<String, Vec<String>> {
        let mut links = HashMap::new();
        links.insert(
            "john".to_string(),
            vec![
                "telegram:123456".to_string(),
                "discord:789012".to_string(),
                "slack:U345678".to_string(),
            ],
        );
        links.insert(
            "alice".to_string(),
            vec![
                "telegram:654321".to_string(),
                "imessage:+1234567890".to_string(),
            ],
        );
        links
    }

    #[test]
    fn test_resolve_scoped_match() {
        let links = test_links();
        assert_eq!(
            resolve_linked_peer_id(&links, "telegram", "123456"),
            Some("john".to_string())
        );
    }

    #[test]
    fn test_resolve_cross_channel() {
        let links = test_links();
        assert_eq!(
            resolve_linked_peer_id(&links, "discord", "789012"),
            Some("john".to_string())
        );
    }

    #[test]
    fn test_resolve_no_match() {
        let links = test_links();
        assert_eq!(resolve_linked_peer_id(&links, "slack", "unknown"), None);
    }

    #[test]
    fn test_resolve_case_insensitive() {
        let links = test_links();
        assert_eq!(
            resolve_linked_peer_id(&links, "TELEGRAM", "123456"),
            Some("john".to_string())
        );
    }

    #[test]
    fn test_resolve_empty_inputs() {
        let links = test_links();
        assert_eq!(resolve_linked_peer_id(&links, "", "123456"), None);
        assert_eq!(resolve_linked_peer_id(&links, "telegram", ""), None);
    }

    #[test]
    fn test_resolve_empty_links() {
        let links = HashMap::new();
        assert_eq!(resolve_linked_peer_id(&links, "telegram", "123"), None);
    }
}
```

**Step 2: Update `core/src/routing/mod.rs`**

```rust
//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route resolution.

pub mod identity_links;
pub mod session_key;

pub use session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib routing 2>&1 | tail -30`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/routing/identity_links.rs core/src/routing/mod.rs
git commit -m "routing: add cross-channel identity links resolution"
```

---

### Task 5: Add config structures for session and bindings

**Files:**
- Create: `core/src/routing/config.rs`
- Modify: `core/src/routing/mod.rs`

**Step 1: Create `core/src/routing/config.rs`**

```rust
//! Configuration structures for routing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::session_key::DmScope;

/// Session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// DM isolation strategy
    #[serde(default)]
    pub dm_scope: DmScope,

    /// Cross-channel identity links: canonical_name -> [channel:id, ...]
    #[serde(default)]
    pub identity_links: HashMap<String, Vec<String>>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: DmScope::PerPeer,
            identity_links: HashMap::new(),
        }
    }
}

/// Route binding configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteBinding {
    pub agent_id: String,
    #[serde(rename = "match")]
    pub match_rule: MatchRule,
}

/// Match rule for route binding
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatchRule {
    /// Channel to match (telegram, discord, slack, ...)
    pub channel: Option<String>,
    /// API account ID (supports "*" wildcard)
    pub account_id: Option<String>,
    /// Peer match (specific user/group)
    pub peer: Option<PeerMatchConfig>,
    /// Discord guild ID
    pub guild_id: Option<String>,
    /// Slack team ID
    pub team_id: Option<String>,
}

/// Peer match configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMatchConfig {
    pub kind: String,
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_config_default() {
        let cfg = SessionConfig::default();
        assert_eq!(cfg.dm_scope, DmScope::PerPeer);
        assert!(cfg.identity_links.is_empty());
    }

    #[test]
    fn test_session_config_deserialize() {
        let toml_str = r#"
            dm_scope = "per-channel-peer"

            [identity_links]
            john = ["telegram:123", "discord:456"]
        "#;
        let cfg: SessionConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.dm_scope, DmScope::PerChannelPeer);
        assert_eq!(cfg.identity_links["john"].len(), 2);
    }

    #[test]
    fn test_route_binding_deserialize() {
        let toml_str = r#"
            agent_id = "work"
            [match]
            channel = "slack"
            team_id = "T12345"
        "#;
        let binding: RouteBinding = toml::from_str(toml_str).unwrap();
        assert_eq!(binding.agent_id, "work");
        assert_eq!(binding.match_rule.channel.as_deref(), Some("slack"));
        assert_eq!(binding.match_rule.team_id.as_deref(), Some("T12345"));
    }
}
```

**Step 2: Update `core/src/routing/mod.rs`**

```rust
//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route resolution.

pub mod config;
pub mod identity_links;
pub mod session_key;

pub use config::{MatchRule, PeerMatchConfig, RouteBinding, SessionConfig};
pub use session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib routing 2>&1 | tail -30`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/routing/config.rs core/src/routing/mod.rs
git commit -m "routing: add session and binding configuration structures"
```

---

### Task 6: Implement route resolver

**Files:**
- Create: `core/src/routing/resolve.rs`
- Modify: `core/src/routing/mod.rs`

**Step 1: Create `core/src/routing/resolve.rs` with tests first**

```rust
//! Hierarchical route resolution.
//!
//! Resolves incoming requests to agents using binding match priority:
//! peer → guild → team → account → channel → default.

use std::collections::HashMap;

use super::config::{MatchRule, RouteBinding, SessionConfig};
use super::identity_links::resolve_linked_peer_id;
use super::session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY};

/// Input for route resolution
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: Option<String>,
    pub peer: Option<RoutePeer>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
}

/// Peer information for routing
#[derive(Debug, Clone)]
pub struct RoutePeer {
    pub kind: RoutePeerKind,
    pub id: String,
}

/// Peer kind for routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePeerKind {
    Dm,
    Group,
    Channel,
}

/// Resolved route result
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub channel: String,
    pub account_id: String,
    pub session_key: SessionKey,
    pub main_session_key: SessionKey,
    pub matched_by: MatchedBy,
}

/// How the route was matched
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    Peer,
    Guild,
    Team,
    Account,
    Channel,
    Default,
}

/// Resolve an agent route from input
pub fn resolve_route(
    bindings: &[RouteBinding],
    session_cfg: &SessionConfig,
    default_agent: &str,
    input: &RouteInput,
) -> ResolvedRoute {
    let channel = input.channel.trim().to_lowercase();
    let account_id = input
        .account_id
        .as_deref()
        .unwrap_or("default")
        .to_string();

    // Filter bindings matching channel and account
    let candidates: Vec<&RouteBinding> = bindings
        .iter()
        .filter(|b| matches_channel(&b.match_rule, &channel))
        .filter(|b| matches_account(&b.match_rule, &account_id))
        .collect();

    let build = |agent_id: &str, matched_by: MatchedBy| -> ResolvedRoute {
        let agent_id = normalize_agent_id(agent_id);
        let session_key = build_session_key(
            &agent_id,
            &channel,
            input.peer.as_ref(),
            session_cfg.dm_scope,
            &session_cfg.identity_links,
        );
        let main_session_key = SessionKey::Main {
            agent_id: agent_id.clone(),
            main_key: DEFAULT_MAIN_KEY.to_string(),
        };
        ResolvedRoute {
            agent_id,
            channel: channel.clone(),
            account_id: account_id.clone(),
            session_key,
            main_session_key,
            matched_by,
        }
    };

    // 1. Peer match
    if let Some(peer) = &input.peer {
        if let Some(b) = candidates.iter().find(|b| matches_peer(&b.match_rule, peer)) {
            return build(&b.agent_id, MatchedBy::Peer);
        }
    }

    // 2. Guild match
    if let Some(guild_id) = &input.guild_id {
        if let Some(b) = candidates
            .iter()
            .find(|b| matches_guild(&b.match_rule, guild_id))
        {
            return build(&b.agent_id, MatchedBy::Guild);
        }
    }

    // 3. Team match
    if let Some(team_id) = &input.team_id {
        if let Some(b) = candidates
            .iter()
            .find(|b| matches_team(&b.match_rule, team_id))
        {
            return build(&b.agent_id, MatchedBy::Team);
        }
    }

    // 4. Account match (specific, not wildcard)
    if let Some(b) = candidates.iter().find(|b| {
        b.match_rule
            .account_id
            .as_ref()
            .map(|a| a != "*")
            .unwrap_or(false)
            && b.match_rule.peer.is_none()
            && b.match_rule.guild_id.is_none()
            && b.match_rule.team_id.is_none()
    }) {
        return build(&b.agent_id, MatchedBy::Account);
    }

    // 5. Channel match (wildcard account)
    if let Some(b) = candidates.iter().find(|b| {
        b.match_rule
            .account_id
            .as_ref()
            .map(|a| a == "*")
            .unwrap_or(false)
            && b.match_rule.peer.is_none()
            && b.match_rule.guild_id.is_none()
            && b.match_rule.team_id.is_none()
    }) {
        return build(&b.agent_id, MatchedBy::Channel);
    }

    // 6. Default
    build(default_agent, MatchedBy::Default)
}

fn build_session_key(
    agent_id: &str,
    channel: &str,
    peer: Option<&RoutePeer>,
    dm_scope: DmScope,
    identity_links: &HashMap<String, Vec<String>>,
) -> SessionKey {
    let Some(peer) = peer else {
        return SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: DEFAULT_MAIN_KEY.to_string(),
        };
    };

    match peer.kind {
        RoutePeerKind::Dm => {
            let peer_id = resolve_linked_peer_id(identity_links, channel, &peer.id)
                .unwrap_or_else(|| peer.id.clone());

            SessionKey::dm(agent_id, channel, &peer_id, dm_scope)
        }
        RoutePeerKind::Group => {
            SessionKey::group(agent_id, channel, PeerKind::Group, &peer.id)
        }
        RoutePeerKind::Channel => {
            SessionKey::group(agent_id, channel, PeerKind::Channel, &peer.id)
        }
    }
}

fn matches_channel(rule: &MatchRule, channel: &str) -> bool {
    rule.channel
        .as_ref()
        .map(|c| c.to_lowercase() == channel)
        .unwrap_or(false)
}

fn matches_account(rule: &MatchRule, account_id: &str) -> bool {
    match &rule.account_id {
        None => account_id == "default",
        Some(a) if a == "*" => true,
        Some(a) => a == account_id,
    }
}

fn matches_peer(rule: &MatchRule, peer: &RoutePeer) -> bool {
    rule.peer.as_ref().map_or(false, |p| {
        let kind_matches = match peer.kind {
            RoutePeerKind::Dm => p.kind.eq_ignore_ascii_case("dm"),
            RoutePeerKind::Group => p.kind.eq_ignore_ascii_case("group"),
            RoutePeerKind::Channel => p.kind.eq_ignore_ascii_case("channel"),
        };
        kind_matches && p.id.eq_ignore_ascii_case(&peer.id)
    })
}

fn matches_guild(rule: &MatchRule, guild_id: &str) -> bool {
    rule.guild_id
        .as_ref()
        .map(|g| g == guild_id)
        .unwrap_or(false)
}

fn matches_team(rule: &MatchRule, team_id: &str) -> bool {
    rule.team_id
        .as_ref()
        .map(|t| t == team_id)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::config::PeerMatchConfig;

    fn default_session_cfg() -> SessionConfig {
        SessionConfig::default()
    }

    fn telegram_binding(agent_id: &str) -> RouteBinding {
        RouteBinding {
            agent_id: agent_id.to_string(),
            match_rule: MatchRule {
                channel: Some("telegram".to_string()),
                account_id: Some("*".to_string()),
                ..Default::default()
            },
        }
    }

    fn slack_team_binding(agent_id: &str, team_id: &str) -> RouteBinding {
        RouteBinding {
            agent_id: agent_id.to_string(),
            match_rule: MatchRule {
                channel: Some("slack".to_string()),
                account_id: Some("*".to_string()),
                team_id: Some(team_id.to_string()),
                ..Default::default()
            },
        }
    }

    fn peer_binding(agent_id: &str, channel: &str, peer_kind: &str, peer_id: &str) -> RouteBinding {
        RouteBinding {
            agent_id: agent_id.to_string(),
            match_rule: MatchRule {
                channel: Some(channel.to_string()),
                account_id: Some("*".to_string()),
                peer: Some(PeerMatchConfig {
                    kind: peer_kind.to_string(),
                    id: peer_id.to_string(),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_default_route() {
        let route = resolve_route(&[], &default_session_cfg(), "main", &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: None,
            guild_id: None,
            team_id: None,
        });
        assert_eq!(route.agent_id, "main");
        assert_eq!(route.matched_by, MatchedBy::Default);
    }

    #[test]
    fn test_channel_match() {
        let bindings = vec![telegram_binding("telegram-agent")];
        let route = resolve_route(&bindings, &default_session_cfg(), "main", &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: None,
            guild_id: None,
            team_id: None,
        });
        assert_eq!(route.agent_id, "telegram-agent");
        assert_eq!(route.matched_by, MatchedBy::Channel);
    }

    #[test]
    fn test_team_match_higher_than_channel() {
        let bindings = vec![
            telegram_binding("generic"),
            slack_team_binding("work", "T12345"),
        ];
        let route = resolve_route(&bindings, &default_session_cfg(), "main", &RouteInput {
            channel: "slack".to_string(),
            account_id: None,
            peer: None,
            guild_id: None,
            team_id: Some("T12345".to_string()),
        });
        assert_eq!(route.agent_id, "work");
        assert_eq!(route.matched_by, MatchedBy::Team);
    }

    #[test]
    fn test_peer_match_highest_priority() {
        let bindings = vec![
            telegram_binding("generic"),
            peer_binding("vip-agent", "telegram", "dm", "user-vip"),
        ];
        let route = resolve_route(&bindings, &default_session_cfg(), "main", &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: Some(RoutePeer {
                kind: RoutePeerKind::Dm,
                id: "user-vip".to_string(),
            }),
            guild_id: None,
            team_id: None,
        });
        assert_eq!(route.agent_id, "vip-agent");
        assert_eq!(route.matched_by, MatchedBy::Peer);
    }

    #[test]
    fn test_dm_scope_per_peer() {
        let route = resolve_route(&[], &default_session_cfg(), "main", &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: Some(RoutePeer {
                kind: RoutePeerKind::Dm,
                id: "user123".to_string(),
            }),
            guild_id: None,
            team_id: None,
        });
        assert_eq!(route.session_key.to_key_string(), "agent:main:dm:user123");
    }

    #[test]
    fn test_dm_scope_per_channel_peer() {
        let cfg = SessionConfig {
            dm_scope: DmScope::PerChannelPeer,
            ..Default::default()
        };
        let route = resolve_route(&[], &cfg, "main", &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: Some(RoutePeer {
                kind: RoutePeerKind::Dm,
                id: "user123".to_string(),
            }),
            guild_id: None,
            team_id: None,
        });
        assert_eq!(
            route.session_key.to_key_string(),
            "agent:main:telegram:dm:user123"
        );
    }

    #[test]
    fn test_dm_scope_main_collapses() {
        let cfg = SessionConfig {
            dm_scope: DmScope::Main,
            ..Default::default()
        };
        let route = resolve_route(&[], &cfg, "main", &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: Some(RoutePeer {
                kind: RoutePeerKind::Dm,
                id: "user123".to_string(),
            }),
            guild_id: None,
            team_id: None,
        });
        assert_eq!(route.session_key.to_key_string(), "agent:main:main");
    }

    #[test]
    fn test_identity_links() {
        let mut links = HashMap::new();
        links.insert(
            "john".to_string(),
            vec!["telegram:123".to_string(), "discord:456".to_string()],
        );
        let cfg = SessionConfig {
            dm_scope: DmScope::PerPeer,
            identity_links: links,
        };
        let route = resolve_route(&[], &cfg, "main", &RouteInput {
            channel: "telegram".to_string(),
            account_id: None,
            peer: Some(RoutePeer {
                kind: RoutePeerKind::Dm,
                id: "123".to_string(),
            }),
            guild_id: None,
            team_id: None,
        });
        // Should resolve to canonical "john" instead of "123"
        assert_eq!(route.session_key.to_key_string(), "agent:main:dm:john");
    }

    #[test]
    fn test_group_session_key() {
        let route = resolve_route(&[], &default_session_cfg(), "main", &RouteInput {
            channel: "discord".to_string(),
            account_id: None,
            peer: Some(RoutePeer {
                kind: RoutePeerKind::Group,
                id: "guild456".to_string(),
            }),
            guild_id: None,
            team_id: None,
        });
        assert_eq!(
            route.session_key.to_key_string(),
            "agent:main:discord:group:guild456"
        );
    }
}
```

**Step 2: Update `core/src/routing/mod.rs`**

```rust
//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route resolution.

pub mod config;
pub mod identity_links;
pub mod resolve;
pub mod session_key;

pub use config::{MatchRule, PeerMatchConfig, RouteBinding, SessionConfig};
pub use resolve::{resolve_route, MatchedBy, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind};
pub use session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY};
```

**Step 3: Run all routing tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib routing 2>&1 | tail -30`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/routing/resolve.rs core/src/routing/mod.rs
git commit -m "routing: add hierarchical route resolver with binding priority"
```

---

### Task 7: Wire gateway to use new routing module

**Files:**
- Modify: `core/src/gateway/router.rs`
- Modify: `core/src/gateway/session_manager.rs`

**Step 1: Add re-export alias in gateway/router.rs**

At the top of `core/src/gateway/router.rs`, add a type alias and re-export so existing consumers keep working. Do NOT delete existing code yet — add compatibility bridge:

Add at the very top of the file (after the existing doc comment, before `use` statements):

```rust
// Re-export new routing types for backward compatibility.
// Existing code using gateway::router::SessionKey will continue to work.
pub use crate::routing::SessionKey as NewSessionKey;
pub use crate::routing::{DmScope, PeerKind};
```

**Step 2: Add conversion methods to the old SessionKey**

Add this impl block at the end of the file (before `#[cfg(test)]`):

```rust
impl SessionKey {
    /// Convert legacy SessionKey to new routing SessionKey
    pub fn to_new(&self) -> crate::routing::SessionKey {
        match self {
            Self::Main { agent_id, main_key } => crate::routing::SessionKey::Main {
                agent_id: agent_id.clone(),
                main_key: main_key.clone(),
            },
            Self::PerPeer { agent_id, peer_id } => crate::routing::SessionKey::DirectMessage {
                agent_id: agent_id.clone(),
                channel: String::new(),
                peer_id: peer_id.clone(),
                dm_scope: crate::routing::DmScope::PerPeer,
            },
            Self::Task { agent_id, task_type, task_id } => crate::routing::SessionKey::Task {
                agent_id: agent_id.clone(),
                task_type: task_type.clone(),
                task_id: task_id.clone(),
            },
            Self::Ephemeral { agent_id, ephemeral_id } => crate::routing::SessionKey::Ephemeral {
                agent_id: agent_id.clone(),
                ephemeral_id: ephemeral_id.clone(),
            },
        }
    }

    /// Create legacy SessionKey from new routing SessionKey
    pub fn from_new(key: &crate::routing::SessionKey) -> Self {
        match key {
            crate::routing::SessionKey::Main { agent_id, main_key } => Self::Main {
                agent_id: agent_id.clone(),
                main_key: main_key.clone(),
            },
            crate::routing::SessionKey::DirectMessage { agent_id, peer_id, .. } => Self::PerPeer {
                agent_id: agent_id.clone(),
                peer_id: peer_id.clone(),
            },
            crate::routing::SessionKey::Group { agent_id, peer_id, .. } => Self::PerPeer {
                agent_id: agent_id.clone(),
                peer_id: peer_id.clone(),
            },
            crate::routing::SessionKey::Task { agent_id, task_type, task_id } => Self::Task {
                agent_id: agent_id.clone(),
                task_type: task_type.clone(),
                task_id: task_id.clone(),
            },
            crate::routing::SessionKey::Subagent { parent_key, .. } => Self::from_new(parent_key),
            crate::routing::SessionKey::Ephemeral { agent_id, ephemeral_id } => Self::Ephemeral {
                agent_id: agent_id.clone(),
                ephemeral_id: ephemeral_id.clone(),
            },
        }
    }
}
```

**Step 3: Verify everything compiles**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo check --features gateway 2>&1 | tail -20`
Expected: Compiles

**Step 4: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --features gateway 2>&1 | tail -30`
Expected: All tests PASS (existing + new)

**Step 5: Commit**

```bash
git add core/src/gateway/router.rs
git commit -m "routing: add backward-compatible bridge between old and new SessionKey"
```

---

### Task 8: Full test pass and export verification

**Files:**
- Modify: `core/src/lib.rs` (verify routing export)

**Step 1: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --features gateway 2>&1 | tail -40`
Expected: All tests PASS

**Step 2: Verify routing module is properly exported**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo doc --no-deps --features gateway 2>&1 | tail -10`
Expected: Documentation builds without errors

**Step 3: Final commit**

```bash
git add -A
git commit -m "routing: complete Part C session key enhancement with full test coverage"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Create routing module + SessionKey types | `routing/mod.rs`, `routing/session_key.rs`, `lib.rs` |
| 2 | SessionKey constructors + agent_id | `routing/session_key.rs` |
| 3 | SessionKey serialization (to_key_string/parse) | `routing/session_key.rs` |
| 4 | Identity links resolution | `routing/identity_links.rs` |
| 5 | Config structures (SessionConfig, RouteBinding) | `routing/config.rs` |
| 6 | Route resolver with binding priority | `routing/resolve.rs` |
| 7 | Gateway backward-compatible bridge | `gateway/router.rs` |
| 8 | Full test pass | All files |
