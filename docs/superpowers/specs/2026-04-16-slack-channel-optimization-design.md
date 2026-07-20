# Slack Channel Optimization Design

> Date: 2026-04-16
> Scope: `src/gateway/interfaces/slack/`
> Objective: Add user allowlist and directory resolution capabilities to Aleph's native Rust Slack channel, addressing security and UX gaps compared to OpenClaw's Slack implementation.

---

## 1. Background & Motivation

Aleph's Slack channel (`src/gateway/interfaces/slack/`) is a well-structured native Rust implementation with Socket Mode WebSocket handling, exponential backoff reconnection, message debouncing, and comprehensive test coverage. However, two key capabilities present in OpenClaw's Slack implementation are missing:

1. **User allowlist**: Channel filtering exists (`allowed_channels`) but there is no per-user filtering. Any user in an allowed channel can message the bot.
2. **Directory resolution**: `sender_name` is set directly from the Slack `user_id` string, with no display name resolution. OpenClaw resolves user IDs to real names via `users.info` API.

These gaps create security and UX issues:
- Security: Malicious users in shared channels can spam the bot without restriction
- UX: Bot responses reference user IDs instead of readable names, degrading conversational clarity

This design adopts **Scheme A — Minimum Viable Improvement**: add user allowlist filtering and optional directory resolution with minimal architectural change.

---

## 2. Target File Structure

```
src/gateway/interfaces/slack/
├── mod.rs                 # SlackChannel + SlackChannelFactory (~516 lines, existing)
├── config.rs              # SlackConfig + validation (~243 lines, existing)
├── message_ops.rs         # SlackMessageOps (~1337 lines, existing)
├── directory.rs           # UserDirectory (NEW - user name resolution)
└── tests/                # Extended unit tests
```

### Boundary Rules

1. `directory.rs` is a pure utility module with no business logic
2. User allowlist filtering happens in `message_ops.rs` during event → inbound conversion
3. `sender_name` resolution is delegated to `UserDirectory` when `resolve_user_names = true`

---

## 3. Configuration Changes

### 3.1 `config.rs` — New Fields

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfig {
    // ... existing fields ...

    /// Allowed user IDs (empty = allow all users in allowed channels)
    #[serde(default)]
    pub user_allowlist: Vec<String>,

    /// Resolve user IDs to display names via users.info API
    /// Caches results with TTL to avoid rate limiting
    #[serde(default)]
    pub resolve_user_names: bool,

    /// Directory cache TTL in seconds (default: 3600)
    #[serde(default = "default_directory_ttl")]
    pub directory_ttl_secs: u64,
}

fn default_directory_ttl() -> u64 {
    3600
}
```

### 3.2 Validation

```rust
impl SlackConfig {
    pub fn validate(&self) -> Result<(), String> {
        // ... existing checks ...

        // user_allowlist: no specific validation (empty = allow all)
        // resolve_user_names: only valid if bot token has users:read scope
        Ok(())
    }

    /// Check if a user ID is allowed
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.user_allowlist.is_empty() {
            true
        } else {
            self.user_allowlist.contains(&user_id.to_string())
        }
    }
}
```

---

## 4. Directory Resolution Module

### 4.1 `directory.rs` — New File

```rust
//! User Directory Resolution
//!
//! Resolves Slack user IDs to display names via `users.info` API.
//! Results are cached with TTL to minimize API calls and avoid rate limits.

use crate::gateway::channel::ChannelError;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::RwLock;

/// Cache entry with expiration
struct CacheEntry {
    name: String,
    expires_at: tokio::time::Instant,
}

/// User directory for resolving user IDs to display names.
pub struct UserDirectory {
    client: reqwest::Client,
    bot_token: String,
    cache: RwLock<HashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl UserDirectory {
    /// Create a new UserDirectory
    pub fn new(bot_token: String, ttl_secs: u64) -> Self {
        Self {
            client: reqwest::Client::new(),
            bot_token,
            cache: RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// Resolve a user ID to display name.
    ///
    /// Returns `None` if:
    /// - User not found in Slack
    /// - API call fails
    ///
    /// Caches results for `ttl` seconds.
    pub async fn resolve(&self, user_id: &str) -> Option<String> {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(user_id) {
                if entry.expires_at > tokio::time::Instant::now() {
                    return Some(entry.name.clone());
                }
            }
        }

        // Cache miss or expired — fetch from API
        let name = self.fetch_user_name(user_id).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(
                user_id.to_string(),
                CacheEntry {
                    name: name.clone(),
                    expires_at: tokio::time::Instant::now() + self.ttl,
                },
            );
        }

        Some(name)
    }

    /// Fetch user name from Slack `users.info` API
    async fn fetch_user_name(&self, user_id: &str) -> Option<String> {
        const SLACK_API: &str = "https://slack.com/api";

        let resp: serde_json::Value = self
            .client
            .get(format!("{SLACK_API}/users.info"))
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .query(&[("user", user_id)])
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;

        if resp["ok"].as_bool() != Some(true) {
            tracing::debug!(
                "Slack users.info failed: {}",
                resp["error"].as_str().unwrap_or("unknown")
            );
            return None;
        }

        resp["user"]["profile"]["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                resp["user"]["profile"]["real_name"]
                    .as_str()
                    .map(String::from)
            })
    }

    /// Clear the entire cache (useful for testing)
    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
    }

    /// Return cache size (useful for metrics)
    pub async fn cache_size(&self) -> usize {
        self.cache.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_expiration() {
        let dir = UserDirectory::new("xoxb-test".to_string(), 0); // 0 second TTL = immediate expiry
        let dir = Arc::new(dir);

        // First resolve should miss cache
        // Note: In real tests, we'd mock the HTTP client
    }
}
```

---

## 5. Message Operations Changes

### 5.1 `message_ops.rs` — Modified Functions

#### User Allowlist Check

Add to `convert_event_to_inbound`:

```rust
pub fn convert_event_to_inbound(
    event: &serde_json::Value,
    channel_id: &ChannelId,
    bot_user_id: &str,
    config: &SlackConfig,
) -> Option<InboundMessage> {
    // ... existing channel filtering ...

    // NEW: User allowlist check
    if !config.is_user_allowed(user_id) {
        tracing::debug!(
            "Slack: user {} not in user_allowlist, filtering",
            user_id
        );
        return None;
    }

    // ... rest of function ...
}
```

Add to `convert_app_mention_to_inbound`:

```rust
pub fn convert_app_mention_to_inbound(
    event: &serde_json::Value,
    channel_id: &ChannelId,
    bot_user_id: &str,
    config: &SlackConfig,
) -> Option<InboundMessage> {
    // ... existing checks ...

    // NEW: User allowlist check
    let user_id = event["user"].as_str()?;
    if !config.is_user_allowed(user_id) {
        tracing::debug!(
            "Slack: user {} not in user_allowlist, filtering mention",
            user_id
        );
        return None;
    }

    // ... rest of function ...
}
```

#### Directory Resolution

Extend `run_socket_mode_loop` signature to accept optional `UserDirectory`:

```rust
pub async fn run_socket_mode_loop(
    client: reqwest::Client,
    app_token: String,
    bot_user_id: Arc<RwLock<Option<String>>>,
    channel_id: ChannelId,
    config: SlackConfig,
    inbound_tx: InboundMessageSender,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    // NEW: Optional user directory for name resolution
    user_directory: Option<Arc<UserDirectory>>,
) {
    // ... in the message handling loop ...
    if let Some(inbound) = inbound {
        // NEW: Resolve sender name if enabled
        let sender_name = if config.resolve_user_names {
            if let Some(ref dir) = user_directory {
                dir.resolve(inbound.sender_id.as_str()).await
            } else {
                None
            }
        } else {
            None
        };

        let final_inbound = if let Some(name) = sender_name {
            InboundMessage {
                sender_name: Some(name),
                ..inbound
            }
        } else {
            inbound
        };

        // Use final_inbound for debouncer.enqueue
    }
}
```

---

## 6. Module Exports

### 6.1 `mod.rs` — Updated Exports

```rust
//! Slack Channel Implementation
//!
//! ... existing docs ...

pub mod config;
pub mod message_ops;
pub mod directory; // NEW

pub use config::SlackConfig;
pub use message_ops::SlackMessageOps;
pub use directory::UserDirectory; // NEW
```

---

## 7. Test Coverage

### 7.1 New Test Cases

```rust
// config.rs tests
#[test]
fn test_user_allowlist_empty_allows_all() {
    let config = SlackConfig::default();
    assert!(config.is_user_allowed("U123"));
    assert!(config.is_user_allowed("U456"));
}

#[test]
fn test_user_allowlist_restricts() {
    let config = SlackConfig {
        user_allowlist: vec!["U123".to_string()],
        ..Default::default()
    };
    assert!(config.is_user_allowed("U123"));
    assert!(!config.is_user_allowed("U456"));
}

// message_ops.rs tests
#[test]
fn test_convert_filters_user_not_in_allowlist() {
    let event = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "Hello",
        "ts": "1700000000.000100"
    });

    let channel_id = ChannelId::new("slack");
    let config = SlackConfig {
        user_allowlist: vec!["U123".to_string()],
        ..Default::default()
    };

    let msg = SlackMessageOps::convert_event_to_inbound(&event, &channel_id, "B123", &config);
    assert!(msg.is_none());
}

// directory.rs tests
#[tokio::test]
async fn test_directory_resolve_returns_cached() {
    let dir = UserDirectory::new("xoxb-test".to_string(), 3600);
    // Test cache behavior
}
```

---

## 8. Risk Assessment

| Risk | Level | Mitigation |
|------|-------|------------|
| Rate limiting from `users.info` | Medium | Cache with TTL; graceful fallback to user_id |
| User directory service unavailable | Low | Non-fatal; logs warning, falls back to user_id |
| Allowlist misconfiguration | Low | Empty = allow all (backward compatible) |
| Performance regression | Low | Directory resolution is async + cached |

---

## 9. Backward Compatibility

- `user_allowlist: []` (default) maintains existing behavior
- `resolve_user_names: false` (default) maintains existing behavior
- No changes to existing message flow when new config fields are absent

---

## 10. Milestones

| Phase | Description | Effort |
|-------|-------------|--------|
| 1 | Add `user_allowlist` config + filtering in `message_ops.rs` | ~1 hour |
| 2 | Create `directory.rs` with caching + tests | ~2 hours |
| 3 | Wire directory into `run_socket_mode_loop`, update `mod.rs` exports | ~1 hour |
| 4 | Extended unit tests + `cargo clippy` + `cargo test` | ~1 hour |

**Total estimated effort**: ~5 hours

---

## 11. Open Questions

1. Should `user_allowlist` support wildcards/regex, or just exact matches?
2. Should directory resolution also cache failures (user not found), or only successful lookups?
3. Should we expose `UserDirectory` metrics (cache hit/miss ratio) via the gateway status endpoint?

---

## 12. References

- OpenClaw Slack implementation: `extensions/slack/src/channel.ts`
- Aleph Slack current implementation: `src/gateway/interfaces/slack/mod.rs`
- Similar pattern in Discord: `src/gateway/interfaces/discord/` (user allowlist exists)
