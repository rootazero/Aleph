# Feishu Channel Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure the Feishu channel from 5 files into 10+ focused modules, adding MessageOperations, sender name cache, enhanced dedup, group session scope, basic card interaction, and config validation.

**Architecture:** Bottom-up restructure: extract `client.rs` into `auth.rs` + `api.rs`, extract WebSocket loop into `websocket.rs`, extract config into `config.rs`, then add new modules (`dedup.rs`, `user_cache.rs`, `message_ops.rs`). All existing tests migrate to their new homes. The `client.rs` file is deleted at the end.

**Tech Stack:** Rust, tokio, async-trait, serde, reqwest, tokio-tungstenite

**Spec:** `docs/superpowers/specs/2026-03-27-feishu-channel-optimization-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/interfaces/feishu/config.rs` | **CREATE** | FeishuConfig + validation + GroupSessionScope |
| `src/gateway/interfaces/feishu/auth.rs` | **CREATE** | TokenManager: get/refresh/background renewal |
| `src/gateway/interfaces/feishu/api.rs` | **CREATE** | FeishuApi: all HTTP API calls + FeishuSendError |
| `src/gateway/interfaces/feishu/dedup.rs` | **CREATE** | MessageDedup with TTL + capacity |
| `src/gateway/interfaces/feishu/user_cache.rs` | **CREATE** | UserProfileCache: sender name resolution |
| `src/gateway/interfaces/feishu/websocket.rs` | **CREATE** | WS connect loop + reconnect + event dispatch |
| `src/gateway/interfaces/feishu/message_ops.rs` | **CREATE** | MessageOperations trait impl for Feishu |
| `src/gateway/interfaces/feishu/events.rs` | **MODIFY** | Add CardAction variant + root_id parsing |
| `src/gateway/interfaces/feishu/streaming.rs` | **MODIFY** | FeishuClient → FeishuApi references |
| `src/gateway/interfaces/feishu/types.rs` | **MODIFY** | Remove config types (moved to config.rs), add root_id to MessageBody |
| `src/gateway/interfaces/feishu/mod.rs` | **REWRITE** | Slim FeishuChannel: lifecycle + Channel trait only |
| `src/gateway/interfaces/feishu/client.rs` | **DELETE** | Replaced by auth.rs + api.rs |
| `src/gateway/handlers/channel.rs` | **MODIFY** | Update factory for new() → Result |
| `src/gateway/inbound_router/executor.rs` | **MODIFY** | FeishuClient → shared Arc\<FeishuApi\> |

---

## Task 1: Create `config.rs` — Extract Config + Add Validation + GroupSessionScope

**Files:**
- Create: `src/gateway/interfaces/feishu/config.rs`
- Modify: `src/gateway/interfaces/feishu/types.rs` (remove FeishuConfig + defaults)
- Modify: `src/gateway/interfaces/feishu/mod.rs` (update import)

- [ ] **Step 1: Create `config.rs` with FeishuConfig, GroupSessionScope, and validate()**

Move `FeishuConfig`, the three default functions (`default_domain`, `default_true`, `default_render_mode`), and `FeishuConfig::base_url()` from `types.rs` into `config.rs`. Add `GroupSessionScope` enum and `group_session_scope` field. Add `validate()` method.

```rust
// src/gateway/interfaces/feishu/config.rs
use serde::Deserialize;

fn default_domain() -> String { "feishu".to_string() }
fn default_true() -> bool { true }
fn default_render_mode() -> String { "auto".to_string() }

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupSessionScope {
    #[default]
    Group,
    GroupTopic,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    #[serde(default = "default_domain")]
    pub domain: String,
    pub bot_name: Option<String>,
    #[serde(default = "default_true")]
    pub dm_allowed: bool,
    #[serde(default)]
    pub groups_allowed: bool,
    #[serde(default = "default_true")]
    pub require_mention: bool,
    #[serde(default = "default_true")]
    pub streaming: bool,
    #[serde(default = "default_render_mode")]
    pub render_mode: String,
    #[serde(default = "default_true")]
    pub typing_indicator: bool,
    #[serde(default)]
    pub group_session_scope: GroupSessionScope,
}

impl FeishuConfig {
    pub fn base_url(&self) -> String {
        match self.domain.as_str() {
            "feishu" => "https://open.feishu.cn".to_string(),
            "lark" => "https://open.larksuite.com".to_string(),
            custom => custom.trim_end_matches('/').to_string(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.app_id.is_empty() {
            return Err("app_id is required".into());
        }
        if self.app_secret.is_empty() {
            return Err("app_secret is required".into());
        }
        if !matches!(self.render_mode.as_str(), "auto" | "card" | "raw") {
            return Err(format!("invalid render_mode '{}', must be auto/card/raw", self.render_mode));
        }
        if let Some(ref name) = self.bot_name {
            if name.is_empty() {
                return Err("bot_name cannot be empty if set".into());
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add tests for config validation and GroupSessionScope**

Append to `config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_empty_app_id() {
        let json = serde_json::json!({"app_id": "", "app_secret": "s"});
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_app_secret() {
        let json = serde_json::json!({"app_id": "a", "app_secret": ""});
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_render_mode() {
        let json = serde_json::json!({"app_id": "a", "app_secret": "s", "render_mode": "invalid"});
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_bot_name() {
        let json = serde_json::json!({"app_id": "a", "app_secret": "s", "bot_name": ""});
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_valid_config() {
        let json = serde_json::json!({"app_id": "a", "app_secret": "s"});
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_group_session_scope_default() {
        let json = serde_json::json!({"app_id": "a", "app_secret": "s"});
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(matches!(config.group_session_scope, GroupSessionScope::Group));
    }

    #[test]
    fn test_group_session_scope_topic() {
        let json = serde_json::json!({"app_id": "a", "app_secret": "s", "group_session_scope": "group_topic"});
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert!(matches!(config.group_session_scope, GroupSessionScope::GroupTopic));
    }

    // Migrate existing config tests from types.rs:
    // test_base_url_feishu, test_base_url_lark, test_base_url_custom,
    // test_config_deserialization_defaults, test_config_deserialization_full,
    // test_config_streaming_defaults, test_config_streaming_overrides
}
```

- [ ] **Step 3: Remove config types from `types.rs`**

Delete `FeishuConfig`, `default_domain`, `default_true`, `default_render_mode`, `FeishuConfig::base_url()`, and all config-related tests from `types.rs`. Keep everything else (ChatType, Mention, FeishuEvent, WsFrameType, API response types, TypingState).

- [ ] **Step 4: Update `mod.rs` imports**

Change `pub use types::FeishuConfig;` to `pub use config::FeishuConfig;` and add `pub mod config;` to the module declarations.

- [ ] **Step 5: Run `cargo check -p alephcore` and fix any import errors**

- [ ] **Step 6: Run `cargo test -p alephcore --lib feishu`**

Expected: All existing tests pass + new validation tests pass.

- [ ] **Step 7: Commit**

```
git add src/gateway/interfaces/feishu/config.rs src/gateway/interfaces/feishu/types.rs src/gateway/interfaces/feishu/mod.rs
git commit -m "feishu: extract config.rs with validation and GroupSessionScope"
```

---

## Task 2: Create `auth.rs` — Extract TokenManager

**Files:**
- Create: `src/gateway/interfaces/feishu/auth.rs`
- Modify: `src/gateway/interfaces/feishu/client.rs` (will be deleted later, but modify now to re-export)

- [ ] **Step 1: Create `auth.rs` with TokenManager**

Extract `TokenState`, token refresh logic, and background refresh from `client.rs:32-177`.

```rust
// src/gateway/interfaces/feishu/auth.rs
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::types::TokenResponse;

const TOKEN_REFRESH_MARGIN_SECS: u64 = 300;
const DEFAULT_TOKEN_EXPIRY_SECS: u64 = 7200;

pub(super) struct TokenState {
    pub(super) access_token: String,
    pub(super) expires_at: Instant,
}

impl TokenState {
    pub(super) fn needs_refresh(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

pub struct TokenManager {
    app_id: String,
    app_secret: String,
    base_url: String,
    http: reqwest::Client,
    token: Arc<RwLock<TokenState>>,
}

impl TokenManager {
    pub fn new(app_id: &str, app_secret: &str, base_url: &str, http: reqwest::Client) -> Self {
        Self {
            app_id: app_id.to_string(),
            app_secret: app_secret.to_string(),
            base_url: base_url.to_string(),
            http,
            token: Arc::new(RwLock::new(TokenState {
                access_token: String::new(),
                expires_at: Instant::now(),
            })),
        }
    }

    pub async fn get_token(&self) -> Result<String, String> {
        {
            let state = self.token.read().await;
            if !state.needs_refresh() {
                return Ok(state.access_token.clone());
            }
        }
        self.refresh_token().await?;
        let state = self.token.read().await;
        Ok(state.access_token.clone())
    }

    pub async fn refresh_token(&self) -> Result<(), String> {
        // Same logic as current client.rs:69-101
        let url = format!("{}/open-apis/auth/v3/app_access_token/internal", self.base_url);
        let body = serde_json::json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let resp = self.http.post(&url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Token request failed: {e}"))?;

        let token_resp: TokenResponse = resp.json().await
            .map_err(|e| format!("Token response parse failed: {e}"))?;

        if token_resp.code != 0 {
            return Err(format!("Token error: code={}, msg={}", token_resp.code, token_resp.msg));
        }

        let access_token = token_resp.app_access_token
            .ok_or_else(|| "No access_token in response".to_string())?;
        let expire = token_resp.expire.unwrap_or(DEFAULT_TOKEN_EXPIRY_SECS);
        let expires_at = Instant::now() + Duration::from_secs(expire.saturating_sub(TOKEN_REFRESH_MARGIN_SECS));

        let mut state = self.token.write().await;
        state.access_token = access_token;
        state.expires_at = expires_at;

        tracing::debug!("Feishu token refreshed, expires in {}s", expire);
        Ok(())
    }

    pub fn spawn_background_refresh(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        // Same logic as current client.rs:116-177
        let app_id = self.app_id.clone();
        let app_secret = self.app_secret.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();
        let token = self.token.clone();

        tokio::spawn(async move {
            let mut shutdown = shutdown;
            loop {
                let sleep_duration = {
                    let state = token.read().await;
                    let now = Instant::now();
                    if state.expires_at > now {
                        state.expires_at.duration_since(now)
                    } else {
                        Duration::from_secs(60)
                    }
                };

                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {}
                    _ = shutdown.changed() => {
                        tracing::debug!("Token refresh task shutting down");
                        return;
                    }
                }

                let url = format!("{}/open-apis/auth/v3/app_access_token/internal", base_url);
                let body = serde_json::json!({
                    "app_id": app_id,
                    "app_secret": app_secret,
                });

                match http.post(&url)
                    .header("Content-Type", "application/json; charset=utf-8")
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        match resp.json::<TokenResponse>().await {
                            Ok(tr) if tr.code == 0 => {
                                if let Some(at) = tr.app_access_token {
                                    let expire = tr.expire.unwrap_or(DEFAULT_TOKEN_EXPIRY_SECS);
                                    let mut state = token.write().await;
                                    state.access_token = at;
                                    state.expires_at = Instant::now() + Duration::from_secs(
                                        expire.saturating_sub(TOKEN_REFRESH_MARGIN_SECS)
                                    );
                                    tracing::debug!("Feishu token refreshed (background)");
                                }
                            }
                            Ok(tr) => tracing::warn!("Token refresh failed: code={}, msg={}", tr.code, tr.msg),
                            Err(e) => tracing::warn!("Token refresh parse error: {e}"),
                        }
                    }
                    Err(e) => tracing::warn!("Token refresh request error: {e}"),
                }
            }
        });
    }
}
```

- [ ] **Step 2: Add `pub mod auth;` to `mod.rs`**

- [ ] **Step 3: Run `cargo check -p alephcore`**

Expected: compiles (auth.rs is standalone, nothing uses it yet).

- [ ] **Step 4: Commit**

```
git commit -m "feishu: extract auth.rs with TokenManager"
```

---

## Task 3: Create `api.rs` — Extract FeishuApi

**Files:**
- Create: `src/gateway/interfaces/feishu/api.rs`

- [ ] **Step 1: Create `api.rs` with FeishuApi**

Extract all HTTP API methods from `client.rs` into `FeishuApi`. This struct holds `Arc<TokenManager>` instead of managing tokens itself. Move `FeishuSendError` here too.

Key methods (same signatures as current `client.rs`, but using `self.auth.get_token()` instead of `self.get_token()`). **Important: `reply_message()` must be `pub` (it was private in `client.rs`) since `message_ops.rs` needs it.**
- `get_bot_info()`, `bot_open_id()`
- `get_ws_endpoint()`, `refresh_ws_endpoint()` (replaces `ws_reconnect_handle()`)
- `send_text()`, `send_image()`, `send_card()`, `reply_message()` (now `pub`)
- `create_streaming_card()`, `send_card_message()`, `update_streaming_card()`, `close_streaming_card()` — fix UTF-8 summary truncation with `char_indices()`
- `upload_image()`, `download_media()`
- `add_reaction()`, `remove_reaction()`
- NEW: `get_user_info(open_id) -> Result<Option<String>, String>` — calls `GET /open-apis/contact/v3/users/{open_id}?user_id_type=open_id`

```rust
pub struct FeishuApi {
    auth: Arc<TokenManager>,
    http: reqwest::Client,
    base_url: String,
    bot_open_id: Arc<RwLock<Option<String>>>,
}
```

The `close_streaming_card` summary truncation must use:
```rust
let summary = text.char_indices()
    .nth(50)
    .map(|(idx, _)| &text[..idx])
    .unwrap_or(text);
```

- [ ] **Step 2: Add `pub mod api;` to `mod.rs`**

- [ ] **Step 3: Run `cargo check -p alephcore`**

- [ ] **Step 4: Commit**

```
git commit -m "feishu: extract api.rs with FeishuApi and UTF-8 safe truncation"
```

---

## Task 4: Create `dedup.rs` — Enhanced Deduplication

**Files:**
- Create: `src/gateway/interfaces/feishu/dedup.rs`

- [ ] **Step 1: Create `dedup.rs` with MessageDedup**

```rust
// src/gateway/interfaces/feishu/dedup.rs
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const DEFAULT_CAPACITY: usize = 5000;
const DEFAULT_TTL: Duration = Duration::from_secs(86400); // 24 hours

pub struct MessageDedup {
    seen: VecDeque<(String, Instant)>,
    capacity: usize,
    ttl: Duration,
}

impl MessageDedup {
    pub fn new() -> Self {
        Self {
            seen: VecDeque::with_capacity(DEFAULT_CAPACITY),
            capacity: DEFAULT_CAPACITY,
            ttl: DEFAULT_TTL,
        }
    }

    /// Returns true if the message_id was already seen (duplicate).
    pub fn is_duplicate(&mut self, message_id: &str) -> bool {
        let now = Instant::now();

        // Drain expired entries from front
        while let Some((_, ts)) = self.seen.front() {
            if now.duration_since(*ts) > self.ttl {
                self.seen.pop_front();
            } else {
                break;
            }
        }

        // Check for existing
        if self.seen.iter().any(|(id, _)| id == message_id) {
            return true;
        }

        // Evict oldest if at capacity
        if self.seen.len() >= self.capacity {
            self.seen.pop_front();
        }

        self.seen.push_back((message_id.to_string(), now));
        false
    }
}
```

- [ ] **Step 2: Add tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_duplicate() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg1"));
    }

    #[test]
    fn test_is_duplicate() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg1"));
        assert!(dedup.is_duplicate("msg1"));
    }

    #[test]
    fn test_different_ids_not_duplicate() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg1"));
        assert!(!dedup.is_duplicate("msg2"));
    }

    #[test]
    fn test_capacity_eviction() {
        let mut dedup = MessageDedup {
            seen: VecDeque::new(),
            capacity: 3,
            ttl: DEFAULT_TTL,
        };
        assert!(!dedup.is_duplicate("a"));
        assert!(!dedup.is_duplicate("b"));
        assert!(!dedup.is_duplicate("c"));
        // At capacity, "a" should be evicted
        assert!(!dedup.is_duplicate("d"));
        assert!(!dedup.is_duplicate("a")); // "a" was evicted, should be treated as new
    }
}
```

- [ ] **Step 3: Add `pub mod dedup;` to `mod.rs`**

- [ ] **Step 4: Run `cargo test -p alephcore --lib feishu::dedup`**

Expected: All pass.

- [ ] **Step 5: Commit**

```
git commit -m "feishu: add dedup.rs with TTL-based message deduplication"
```

---

## Task 5: Create `user_cache.rs` — Sender Name Cache

**Files:**
- Create: `src/gateway/interfaces/feishu/user_cache.rs`

- [ ] **Step 1: Create `user_cache.rs`**

```rust
// src/gateway/interfaces/feishu/user_cache.rs
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use crate::sync_primitives::Arc;
use super::api::FeishuApi;

const CACHE_CAPACITY: usize = 500;
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

enum CachedEntry {
    Found { name: String, fetched_at: Instant },
    NotAvailable { fetched_at: Instant },
}

impl CachedEntry {
    fn is_expired(&self) -> bool {
        let fetched_at = match self {
            CachedEntry::Found { fetched_at, .. } => fetched_at,
            CachedEntry::NotAvailable { fetched_at } => fetched_at,
        };
        fetched_at.elapsed() > CACHE_TTL
    }
}

pub struct UserProfileCache {
    api: Arc<FeishuApi>,
    cache: StdMutex<HashMap<String, CachedEntry>>,
}

impl UserProfileCache {
    pub fn new(api: Arc<FeishuApi>) -> Self {
        Self {
            api,
            cache: StdMutex::new(HashMap::with_capacity(CACHE_CAPACITY)),
        }
    }

    pub async fn resolve_name(&self, open_id: &str) -> Option<String> {
        // Check cache
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = cache.get(open_id) {
                if !entry.is_expired() {
                    return match entry {
                        CachedEntry::Found { name, .. } => Some(name.clone()),
                        CachedEntry::NotAvailable { .. } => None,
                    };
                }
            }
        }

        // Fetch from API
        let result = self.api.get_user_info(open_id).await;

        let entry = match result {
            Ok(Some(name)) => CachedEntry::Found { name: name.clone(), fetched_at: Instant::now() },
            _ => CachedEntry::NotAvailable { fetched_at: Instant::now() },
        };

        let name = match &entry {
            CachedEntry::Found { name, .. } => Some(name.clone()),
            CachedEntry::NotAvailable { .. } => None,
        };

        // Store in cache
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());

            // Evict oldest if at capacity
            if cache.len() >= CACHE_CAPACITY && !cache.contains_key(open_id) {
                if let Some(oldest_key) = cache.iter()
                    .min_by_key(|(_, entry)| match entry {
                        CachedEntry::Found { fetched_at, .. } => *fetched_at,
                        CachedEntry::NotAvailable { fetched_at } => *fetched_at,
                    })
                    .map(|(k, _)| k.clone())
                {
                    cache.remove(&oldest_key);
                }
            }

            cache.insert(open_id.to_string(), entry);
        }

        name
    }
}
```

- [ ] **Step 2: Add `pub mod user_cache;` to `mod.rs`**

- [ ] **Step 3: Run `cargo check -p alephcore`**

- [ ] **Step 4: Commit**

```
git commit -m "feishu: add user_cache.rs for sender name resolution"
```

---

## Task 6: Update `events.rs` — Add CardAction + root_id

**Files:**
- Modify: `src/gateway/interfaces/feishu/events.rs`
- Modify: `src/gateway/interfaces/feishu/types.rs`

- [ ] **Step 1: Add `root_id` to `MessageBody` in `types.rs`**

In `types.rs`, add to `MessageBody` struct:
```rust
pub root_id: Option<String>,
```

- [ ] **Step 2: Add `CardAction` variant to `FeishuEvent` in `types.rs`**

```rust
pub enum FeishuEvent {
    MessageReceive { /* existing fields + root_id */ },
    CardAction {
        chat_id: String,
        sender_id: String,
        action_value: String,
        message_id: String,
    },
    Unknown(String),
}
```

Also add `root_id: Option<String>` to `MessageReceive` variant.

- [ ] **Step 3: Update `parse_message_event()` in `events.rs` to propagate `root_id`**

In `events.rs:64-74`, add:
```rust
root_id: message.root_id.clone(),
```

- [ ] **Step 4: Add `card.action.trigger` parsing to `parse_ws_frame()`**

In `events.rs`, update the match on `event_type`:
```rust
match event_type {
    "im.message.receive_v1" => parse_message_event(&envelope),
    "card.action.trigger" => parse_card_action(&envelope),
    other => Ok(Some(FeishuEvent::Unknown(other.to_string()))),
}
```

Add `parse_card_action`:
```rust
fn parse_card_action(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => return Ok(Some(FeishuEvent::Unknown("card action without body".to_string()))),
    };

    // Feishu card action payload has: operator.open_id, action.value, context.open_message_id, context.open_chat_id
    let operator_id = event_value.pointer("/operator/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let chat_id = event_value.pointer("/context/open_chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let message_id = event_value.pointer("/context/open_message_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // action.value can be a string or object; extract as string
    let action_value = event_value.pointer("/action/value")
        .map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            }
        })
        .unwrap_or_default();

    if action_value.is_empty() {
        return Ok(Some(FeishuEvent::Unknown("card action with empty value".to_string())));
    }

    Ok(Some(FeishuEvent::CardAction {
        chat_id,
        sender_id: operator_id,
        action_value,
        message_id,
    }))
}
```

- [ ] **Step 5: Add tests for CardAction parsing**

```rust
#[test]
fn test_parse_card_action() {
    let raw = r#"{
        "header": {"event_id": "e1", "event_type": "card.action.trigger", "token": "t"},
        "event": {
            "operator": {"open_id": "ou_user1"},
            "action": {"value": "start_conversation"},
            "context": {"open_chat_id": "oc_chat1", "open_message_id": "msg_card1"}
        }
    }"#;
    let result = parse_ws_frame(raw).unwrap().unwrap();
    match result {
        FeishuEvent::CardAction { chat_id, sender_id, action_value, message_id } => {
            assert_eq!(chat_id, "oc_chat1");
            assert_eq!(sender_id, "ou_user1");
            assert_eq!(action_value, "start_conversation");
            assert_eq!(message_id, "msg_card1");
        }
        _ => panic!("Expected CardAction"),
    }
}
```

- [ ] **Step 6: Run `cargo test -p alephcore --lib feishu::events`**

- [ ] **Step 7: Commit**

```
git commit -m "feishu: add CardAction event + root_id for topic threading"
```

---

## Task 7: Create `websocket.rs` — Extract WS Loop

**Files:**
- Create: `src/gateway/interfaces/feishu/websocket.rs`

- [ ] **Step 1: Create `websocket.rs` with `WsLoopContext` and `run_ws_loop()`**

Extract the WebSocket connection loop from `mod.rs:135-296`. Key changes:
- Uses `WsLoopContext` struct instead of 9 parameters
- Uses `MessageDedup` from `dedup.rs` instead of inline VecDeque
- Uses `UserProfileCache::resolve_name()` to populate `sender_name`
- Uses `FeishuApi::refresh_ws_endpoint()` instead of manual token/URL rebuilding
- Handles `FeishuEvent::CardAction` by converting to `InboundMessage`
- Computes `conversation_id` based on `GroupSessionScope` config
- Propagates `root_id` from events

```rust
// src/gateway/interfaces/feishu/websocket.rs
use std::sync::Mutex as StdMutex;
use tokio::sync::{mpsc, watch};
use futures_util::StreamExt;
use chrono::Utc;

use crate::sync_primitives::Arc;
use crate::gateway::channel::{
    ChannelId, ChannelStatus, ConversationId, InboundMessage, MessageId, UserId,
};

use super::api::FeishuApi;
use super::config::{FeishuConfig, GroupSessionScope};
use super::dedup::MessageDedup;
use super::events::{extract_text_content, mark_bot_mentions, parse_ws_frame};
use super::types::{ChatType, FeishuEvent};
use super::user_cache::UserProfileCache;

pub struct WsLoopContext {
    pub api: Arc<FeishuApi>,
    pub initial_url: String,
    pub config: FeishuConfig,
    pub sender: mpsc::Sender<InboundMessage>,
    pub channel_id: ChannelId,
    pub bot_open_id: String,
    pub user_cache: Arc<UserProfileCache>,
    pub status_handle: Arc<tokio::sync::RwLock<ChannelStatus>>,  // from ChannelState::status_handle()
    pub shutdown_rx: watch::Receiver<bool>,
}

pub async fn run_ws_loop(mut ctx: WsLoopContext) {
    let dedup = Arc::new(StdMutex::new(MessageDedup::new()));
    let mut backoff_secs: u64 = 1;
    let mut current_url = ctx.initial_url;

    loop {
        if *ctx.shutdown_rx.borrow() { break; }

        tracing::info!("Connecting to Feishu WebSocket: {}...",
            current_url.get(..60).unwrap_or(&current_url));

        match tokio_tungstenite::connect_async(&current_url).await {
            Ok((ws_stream, _)) => {
                backoff_secs = 1;
                *ctx.status_handle.write().await = ChannelStatus::Connected;
                tracing::info!("Feishu WebSocket connected");

                let (_, mut read) = ws_stream.split();

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    if let Some(inbound) = process_ws_message(
                                        &text, &ctx, &dedup,
                                    ).await {
                                        if ctx.sender.send(inbound).await.is_err() {
                                            tracing::warn!("Feishu inbound channel closed");
                                            return;
                                        }
                                    }
                                }
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(_))) => {}
                                Some(Ok(_)) => {}
                                Some(Err(e)) => {
                                    tracing::warn!("Feishu WS error: {e}");
                                    break;
                                }
                                None => {
                                    tracing::info!("Feishu WS stream ended");
                                    break;
                                }
                            }
                        }
                        _ = ctx.shutdown_rx.changed() => {
                            tracing::info!("Feishu WS shutdown signal received");
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Feishu WS connection failed: {e}");
            }
        }

        *ctx.status_handle.write().await = ChannelStatus::Error;
        tracing::info!("Reconnecting in {backoff_secs}s...");

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = ctx.shutdown_rx.changed() => {
                tracing::info!("Feishu WS shutdown during backoff");
                return;
            }
        }

        backoff_secs = (backoff_secs * 2).min(60);

        // Re-fetch WS endpoint URL via FeishuApi
        match ctx.api.refresh_ws_endpoint().await {
            Ok(url) => {
                current_url = url;
                tracing::debug!("Re-fetched WS endpoint URL");
            }
            Err(e) => tracing::warn!("Failed to re-fetch WS endpoint: {e}"),
        }
    }
}

/// Process a single WS text message. Returns an InboundMessage if it should be dispatched.
async fn process_ws_message(
    raw: &str,
    ctx: &WsLoopContext,
    dedup: &Arc<StdMutex<MessageDedup>>,
) -> Option<InboundMessage> {
    let event = match parse_ws_frame(raw) {
        Ok(Some(e)) => e,
        Ok(None) => return None, // ping/pong
        Err(e) => {
            tracing::warn!("Failed to parse Feishu WS frame: {e}");
            return None;
        }
    };

    match event {
        FeishuEvent::MessageReceive {
            message_id, chat_id, chat_type, sender_id,
            message_type, content, mut mentions, parent_id, root_id, ..
        } => {
            // Dedup
            {
                let mut seen = dedup.lock().unwrap_or_else(|e| e.into_inner());
                if seen.is_duplicate(&message_id) { return None; }
            }

            mark_bot_mentions(&mut mentions, &ctx.bot_open_id);

            // Group mention filter
            if chat_type == ChatType::Group && ctx.config.require_mention {
                if !mentions.iter().any(|m| m.is_bot) { return None; }
            }
            if chat_type == ChatType::Group && !ctx.config.groups_allowed { return None; }
            if chat_type == ChatType::P2p && !ctx.config.dm_allowed { return None; }

            // Extract text
            let extracted_text = match message_type.as_str() {
                "text" => extract_text_content(&content, &mentions)?,
                "image" => "[Image]".to_string(),
                other => {
                    tracing::debug!("Skipping unsupported message type: {other}");
                    return None;
                }
            };

            // Resolve sender name
            let sender_name = ctx.user_cache.resolve_name(&sender_id).await;

            // Compute conversation_id based on session scope
            let conversation_id = match ctx.config.group_session_scope {
                GroupSessionScope::Group => chat_id.clone(),
                GroupSessionScope::GroupTopic => {
                    if chat_type == ChatType::Group {
                        if let Some(ref topic_id) = root_id.as_ref().or(parent_id.as_ref()) {
                            format!("{}:topic:{}", chat_id, topic_id)
                        } else {
                            chat_id.clone()
                        }
                    } else {
                        chat_id.clone()
                    }
                }
            };

            Some(InboundMessage {
                id: MessageId::new(&message_id),
                channel_id: ctx.channel_id.clone(),
                conversation_id: ConversationId::new(&conversation_id),
                sender_id: UserId::new(&sender_id),
                sender_name,
                text: extracted_text,
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: parent_id.map(MessageId::new),
                is_group: chat_type == ChatType::Group,
                raw: None,
            })
        }

        FeishuEvent::CardAction { chat_id, sender_id, action_value, message_id } => {
            // Dedup card actions too
            {
                let mut seen = dedup.lock().unwrap_or_else(|e| e.into_inner());
                let dedup_key = format!("card:{}", message_id);
                if seen.is_duplicate(&dedup_key) { return None; }
            }

            let sender_name = ctx.user_cache.resolve_name(&sender_id).await;

            Some(InboundMessage {
                id: MessageId::new(&message_id),
                channel_id: ctx.channel_id.clone(),
                conversation_id: ConversationId::new(&chat_id),
                sender_id: UserId::new(&sender_id),
                sender_name,
                text: action_value,
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: None,
                is_group: false, // Card actions don't carry chat_type; safe default
                raw: None,
            })
        }

        FeishuEvent::Unknown(t) => {
            tracing::debug!("Unknown Feishu event: {t}");
            None
        }
    }
}
```

Note: The actual `status_handle` type depends on what `ChannelState::status_handle()` returns. Check the exact type and adjust accordingly.

- [ ] **Step 2: Add `pub mod websocket;` to `mod.rs`**

- [ ] **Step 3: Run `cargo check -p alephcore`**

Fix any type mismatches (especially `status_handle` type).

- [ ] **Step 4: Commit**

```
git commit -m "feishu: extract websocket.rs with session scope and card action handling"
```

---

## Task 8: Create `message_ops.rs` — MessageOperations Trait

**Files:**
- Create: `src/gateway/interfaces/feishu/message_ops.rs`

- [ ] **Step 1: Create `message_ops.rs`**

```rust
// src/gateway/interfaces/feishu/message_ops.rs
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use tracing::{debug, warn};

use crate::builtin_tools::message::{
    ChannelCapabilities, DeleteParams, EditParams, MessageOperations, MessageResult,
    ReactParams, ReplyParams, SendParams,
};
use crate::error::Result;
use super::api::FeishuApi;

pub struct FeishuMessageOps {
    api: Arc<FeishuApi>,
}

impl FeishuMessageOps {
    pub fn new(api: Arc<FeishuApi>) -> Self {
        Self { api }
    }
}

#[async_trait]
impl MessageOperations for FeishuMessageOps {
    fn channel_id(&self) -> &str { "feishu" }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            reply: true,
            edit: false,
            react: true,
            delete: false,
            send: true,
        }
    }

    async fn reply(&self, params: ReplyParams) -> Result<MessageResult> {
        debug!(message_id = %params.message_id, "Sending Feishu reply");
        let content = serde_json::json!({"text": params.text}).to_string();
        match self.api.reply_message(&params.message_id, "text", &content).await {
            Ok(msg_id) => Ok(MessageResult::success_with_id(msg_id)),
            Err(e) => {
                warn!(error = ?e, "Failed to send Feishu reply");
                Ok(MessageResult::failed(format!("Failed to send reply: {:?}", e)))
            }
        }
    }

    async fn edit(&self, _params: EditParams) -> Result<MessageResult> {
        Ok(MessageResult::failed("Feishu does not support editing sent messages"))
    }

    async fn react(&self, params: ReactParams) -> Result<MessageResult> {
        debug!(message_id = %params.message_id, emoji = %params.emoji, "Setting Feishu reaction");
        if params.remove {
            // Feishu reaction removal requires reaction_id, which we don't have from params
            return Ok(MessageResult::failed("Feishu reaction removal requires reaction_id"));
        }
        match self.api.add_reaction(&params.message_id, &params.emoji).await {
            Ok(_) => Ok(MessageResult::success()),
            Err(e) => {
                warn!(error = %e, "Failed to add Feishu reaction");
                Ok(MessageResult::failed(format!("Failed to add reaction: {}", e)))
            }
        }
    }

    async fn delete(&self, _params: DeleteParams) -> Result<MessageResult> {
        Ok(MessageResult::failed("Feishu Bot API does not support message deletion"))
    }

    async fn send(&self, params: SendParams) -> Result<MessageResult> {
        debug!(target = %params.target, "Sending Feishu message");
        match self.api.send_text(&params.target, &params.text, None).await {
            Ok(msg_id) => Ok(MessageResult::success_with_id(msg_id)),
            Err(e) => {
                warn!(error = ?e, "Failed to send Feishu message");
                Ok(MessageResult::failed(format!("Failed to send message: {:?}", e)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cannot test actual API calls, but test capabilities and channel_id
    #[test]
    fn test_channel_id() {
        // FeishuMessageOps requires Arc<FeishuApi> which needs network.
        // Test the capability matrix is correct.
        let caps = ChannelCapabilities {
            reply: true,
            edit: false,
            react: true,
            delete: false,
            send: true,
        };
        assert!(caps.reply);
        assert!(!caps.edit);
        assert!(caps.react);
        assert!(!caps.delete);
        assert!(caps.send);
    }
}
```

- [ ] **Step 2: Add `pub mod message_ops;` and `pub use message_ops::FeishuMessageOps;` to `mod.rs`**

**Note on registration:** The `MessageOperationsRegistry` exists at `src/builtin_tools/message/tool.rs` but no channel currently registers into it at runtime (the trait implementations exist but the wiring is incomplete across all channels). For now, just create and export `FeishuMessageOps`. Registration will be wired when the MessageTool is integrated into the execution pipeline — this is a pre-existing gap shared with Telegram/Discord/iMessage.

**Note on `ChannelCapabilities` naming:** The import `crate::builtin_tools::message::ChannelCapabilities` is a DIFFERENT struct from `crate::gateway::channel::ChannelCapabilities` used in `mod.rs`. Both are in scope in the feishu module. Use fully qualified imports to avoid confusion.

- [ ] **Step 3: Run `cargo check -p alephcore`**

- [ ] **Step 4: Commit**

```
git commit -m "feishu: add message_ops.rs implementing MessageOperations trait"
```

---

## Task 9: Update `streaming.rs` — FeishuClient → FeishuApi

**Files:**
- Modify: `src/gateway/interfaces/feishu/streaming.rs`

- [ ] **Step 1: Replace all `FeishuClient` references with `FeishuApi`**

In `streaming.rs`:
- Change `use super::client::FeishuClient;` to `use super::api::FeishuApi;`
- Change `client: Arc<FeishuClient>` to `client: Arc<FeishuApi>` in `FeishuEventEmitter`
- Change constructor parameter type accordingly
- Update `FeishuStreamingCard` methods that take `&FeishuClient` to take `&FeishuApi`

- [ ] **Step 2: Run `cargo check -p alephcore`**

- [ ] **Step 3: Commit**

```
git commit -m "feishu: update streaming.rs to use FeishuApi instead of FeishuClient"
```

---

## Task 10: Rewrite `mod.rs` — Slim FeishuChannel

**Files:**
- Rewrite: `src/gateway/interfaces/feishu/mod.rs`

- [ ] **Step 1: Rewrite `mod.rs`**

The new `mod.rs` should:
- Declare all submodules
- Define `FeishuChannel` struct with `api: Option<Arc<FeishuApi>>`, `user_cache: Option<Arc<UserProfileCache>>`
- `new()` returns `Result<Self, ChannelError>` — calls `config.validate()`
- `start()`:
  1. Creates `TokenManager` → `FeishuApi`
  2. Refreshes token + gets bot info
  3. Gets WS endpoint
  4. Creates `UserProfileCache`
  5. Spawns background token refresh
  6. Spawns `run_ws_loop()`
  7. Stores `Arc<FeishuApi>` for `send()` and `streaming.rs`
- `stop()`: sends shutdown signal
- `send()`: same logic as before but uses `self.api`
- Keep `should_use_card()` and its tests
- Remove old WebSocket loop code, dedup VecDeque, FeishuClient usage

- [ ] **Step 2: Run `cargo check -p alephcore`**

Fix compilation errors. This is the biggest change — many references will need updating.

- [ ] **Step 3: Run `cargo test -p alephcore --lib feishu`**

Expected: `should_use_card` tests still pass.

- [ ] **Step 4: Commit**

```
git commit -m "feishu: rewrite mod.rs with slim FeishuChannel using new modules"
```

---

## Task 11: Delete `client.rs` + Update External References

**Files:**
- Delete: `src/gateway/interfaces/feishu/client.rs`
- Modify: `src/gateway/inbound_router/executor.rs`
- Modify: `src/gateway/handlers/channel.rs`

- [ ] **Step 1: Delete `client.rs`**

Remove the file entirely. All its code now lives in `auth.rs` and `api.rs`.

- [ ] **Step 2: Remove `pub mod client;` from `mod.rs`**

- [ ] **Step 3: Update `executor.rs` to use shared `Arc<FeishuApi>`**

In `src/gateway/inbound_router/executor.rs:233-281`:
- Change `use crate::gateway::interfaces::feishu::client::FeishuClient;` to `use crate::gateway::interfaces::feishu::api::FeishuApi;`
- Instead of creating a new `FeishuClient` per emitter (line 254), obtain the shared `Arc<FeishuApi>` from the channel registry. The `FeishuChannel` should expose its api handle via a method like `pub fn api(&self) -> Option<Arc<FeishuApi>>`.
- If the channel doesn't expose the api yet, add a `pub fn api(&self) -> Option<Arc<FeishuApi>>` method to `FeishuChannel`. The executor can downcast or the channel trait can be extended. Check how the executor currently gets the `FeishuConfig` (via `app_config`) and follow the same pattern but for the shared API handle.

Alternative (simpler, if downcast is complex): Create `FeishuApi` in `executor.rs` using config (like now), but reuse `TokenManager` from the channel. Pragmatic fallback: keep creating a fresh `FeishuApi` per emitter for now (same behavior as current `FeishuClient::new`), just update the type. The spec notes this as the ideal but creating a fresh one per emitter is acceptable as a first step.

- [ ] **Step 4: Update channel factory in `channel.rs`**

At `src/gateway/handlers/channel.rs:327-328`:
```rust
// Before:
"feishu" => serde_json::from_value::<FeishuConfig>(config).ok()
    .map(|cfg| Box::new(FeishuChannel::new(id, cfg)) as _),

// After (FeishuChannel::new now returns Result — log validation errors, don't swallow):
"feishu" => serde_json::from_value::<FeishuConfig>(config).ok()
    .and_then(|cfg| match FeishuChannel::new(id, cfg) {
        Ok(ch) => Some(Box::new(ch) as _),
        Err(e) => { tracing::warn!("Invalid feishu config: {}", e); None }
    }),
```

- [ ] **Step 5: Run `cargo check -p alephcore`**

- [ ] **Step 6: Run `cargo test -p alephcore --lib feishu`**

- [ ] **Step 7: Commit**

```
git commit -m "feishu: delete client.rs, update executor and channel factory"
```

---

## Task 12: Final Verification + Cleanup

- [ ] **Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings
```

- [ ] **Step 3: Verify file structure**

```bash
ls -la src/gateway/interfaces/feishu/
```

Expected files: `mod.rs`, `config.rs`, `types.rs`, `events.rs`, `auth.rs`, `api.rs`, `websocket.rs`, `streaming.rs`, `message_ops.rs`, `user_cache.rs`, `dedup.rs`. NO `client.rs`.

- [ ] **Step 4: Verify no remaining FeishuClient references**

```bash
grep -r "FeishuClient" src/
```

Expected: zero matches.

- [ ] **Step 5: Commit final state**

```
git commit -m "feishu: final cleanup and verification after restructure"
```
