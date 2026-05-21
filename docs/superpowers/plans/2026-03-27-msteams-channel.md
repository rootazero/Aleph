# Microsoft Teams Channel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Microsoft Teams as a channel in Aleph's gateway with native streaming, AI UX best practices, and message edit/delete support.

**Architecture:** Pure HTTP implementation against Bot Framework REST API v3. Reuses existing WebhookReceiver for inbound messages, extends ReplyEmitter with NativeStreamHandler for Teams streaminfo protocol. Follows the established Channel trait pattern (Telegram/Discord as references).

**Tech Stack:** Rust, axum, reqwest, jsonwebtoken, serde, tokio

**Spec:** `docs/superpowers/specs/2026-03-27-msteams-channel-design.md`

---

## File Structure

```
src/gateway/interfaces/msteams/
├── mod.rs          # MsTeamsChannel: Channel + WebhookHandler
├── config.rs       # MsTeamsConfig, validation, defaults
├── types.rs        # Bot Framework Activity/Entity types
├── auth.rs         # JWT validation (inbound) + OAuth token cache (outbound)
├── api.rs          # BotFrameworkClient: thin HTTP wrapper
├── streaming.rs    # NativeStreamHandler impl + streaminfo helpers
└── message_ops.rs  # MessageOperations trait impl
```

**Existing files to modify:**
- `Cargo.toml` — add `jsonwebtoken` dependency
- `src/gateway/interfaces/mod.rs:41` — add `pub mod msteams;` + re-export
- `src/gateway/handlers/channel.rs:287-331` — add "msteams" arm to `create_channel_from_config()`
- `src/gateway/channel.rs:291-319` — add `stream_protocol` field to `ChannelCapabilities`, add `StreamProtocol` enum, add `NativeStreamHandler` trait, add `native_stream_handler()` to Channel trait
- `src/gateway/reply_emitter.rs` — add native streaming fields and integration in `emit()`

---

## Task 1: Add `jsonwebtoken` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add jsonwebtoken crate**

In `Cargo.toml` under `[dependencies]`, add:

```toml
jsonwebtoken = "9"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: PASS (no errors)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "msteams: add jsonwebtoken dependency"
```

---

## Task 2: Activity types (types.rs)

**Files:**
- Create: `src/gateway/interfaces/msteams/types.rs`

- [ ] **Step 1: Write tests for Activity deserialization**

Create the file with types and inline tests. The Activity struct must deserialize from real Bot Framework JSON payloads. Tests verify:
- Message activity deserialization with all fields
- ConversationUpdate activity with membersAdded
- Minimal activity (only required fields)
- `build_stream_info_entity()` produces correct JSON
- `build_ai_generated_entity()` produces correct JSON
- `build_welcome_card()` produces valid Adaptive Card
- `Activity::text_message()` convenience constructor
- `strip_mentions()` removes `<at>...</at>` tags

```rust
//! Bot Framework Activity Types
//!
//! Minimal structural types for the Bot Framework v3 protocol.

use serde::{Deserialize, Serialize};

/// Bot Framework Activity (inbound and outbound)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(rename = "type")]
    pub activity_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<ChannelAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<ChannelAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<ActivityAttachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members_added: Option<Vec<ChannelAccount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members_removed: Option<Vec<ChannelAccount>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_data: Option<serde_json::Value>,
}

impl Activity {
    /// Create a simple text message activity
    pub fn text_message(text: &str) -> Self {
        Self {
            activity_type: "message".into(),
            text: Some(text.into()),
            text_format: Some("markdown".into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelAccount {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "aadObjectId", skip_serializing_if = "Option::is_none")]
    pub aad_object_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationAccount {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "conversationType", skip_serializing_if = "Option::is_none")]
    pub conversation_type: Option<String>,
    #[serde(rename = "tenantId", skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(rename = "isGroup", skip_serializing_if = "Option::is_none")]
    pub is_group: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityAttachment {
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActivityResponse {
    pub id: String,
}

/// Inject AI-generated content entity into activity
pub fn inject_ai_entity(activity: &mut Activity) {
    let entity = build_ai_generated_entity();
    match activity.entities {
        Some(ref mut entities) => entities.push(entity),
        None => activity.entities = Some(vec![entity]),
    }
}

/// Build AI-generated content entity for Teams native labeling
pub fn build_ai_generated_entity() -> serde_json::Value {
    serde_json::json!({
        "type": "https://schema.org/Message",
        "@type": "Message",
        "@id": "",
        "additionalType": ["AIGeneratedContent"]
    })
}

/// Build streaminfo entity for Teams streaming protocol
pub fn build_stream_info_entity(
    stream_id: Option<&str>,
    stream_type: &str,
    sequence: u32,
) -> serde_json::Value {
    let mut entity = serde_json::json!({
        "type": "streaminfo",
        "streamType": stream_type,
        "streamSequence": sequence,
    });
    if let Some(id) = stream_id {
        entity["streamId"] = serde_json::Value::String(id.into());
    }
    entity
}

/// Build welcome Adaptive Card with prompt starters
pub fn build_welcome_card(bot_name: &str, prompt_starters: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "AdaptiveCard",
        "version": "1.5",
        "body": [
            {
                "type": "TextBlock",
                "text": format!("Hi! I'm {}.", bot_name),
                "weight": "bolder",
                "size": "medium"
            },
            {
                "type": "TextBlock",
                "text": "I can help you with questions, tasks, and more. Here are some things to try:",
                "wrap": true
            }
        ],
        "actions": prompt_starters.iter().map(|label| {
            serde_json::json!({
                "type": "Action.Submit",
                "title": label,
                "data": { "msteams": { "type": "imBack", "value": label } }
            })
        }).collect::<Vec<_>>()
    })
}

/// Strip `<at>...</at>` mention tags from message text
///
/// Note: `regex` is already a dependency in Cargo.toml (line 39)
pub fn strip_mentions(text: &str) -> String {
    use std::sync::LazyLock;
    // Teams inserts <at>BotName</at> in group chats
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"<at>[^<]*</at>\s*").unwrap()
    });
    RE.replace_all(text, "").trim().to_string()
}

/// Informative status text options (randomized)
const STATUS_TEXTS: &[&str] = &[
    "Thinking...",
    "Working on that...",
    "Checking the details...",
    "Putting an answer together...",
];

/// Pick a random informative status text
pub fn pick_status_text() -> &'static str {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize;
    STATUS_TEXTS[seed % STATUS_TEXTS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activity_text_message() {
        let a = Activity::text_message("hello");
        assert_eq!(a.activity_type, "message");
        assert_eq!(a.text.as_deref(), Some("hello"));
        assert_eq!(a.text_format.as_deref(), Some("markdown"));
    }

    #[test]
    fn test_deserialize_message_activity() {
        let json = r#"{
            "type": "message",
            "id": "1234",
            "serviceUrl": "https://smba.trafficmanager.net/amer/",
            "channelId": "msteams",
            "from": {"id": "user-aad-id", "name": "John", "aadObjectId": "aad-123"},
            "conversation": {"id": "19:conv@thread.v2", "conversationType": "personal"},
            "recipient": {"id": "bot-id", "name": "Aleph"},
            "text": "Hello bot"
        }"#;
        let activity: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(activity.activity_type, "message");
        assert_eq!(activity.text.as_deref(), Some("Hello bot"));
        assert_eq!(activity.from.as_ref().unwrap().aad_object_id.as_deref(), Some("aad-123"));
        assert_eq!(activity.conversation.as_ref().unwrap().conversation_type.as_deref(), Some("personal"));
    }

    #[test]
    fn test_deserialize_conversation_update() {
        let json = r#"{
            "type": "conversationUpdate",
            "membersAdded": [{"id": "bot-id", "name": "Aleph"}],
            "conversation": {"id": "19:conv@thread.v2"},
            "recipient": {"id": "bot-id"}
        }"#;
        let activity: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(activity.activity_type, "conversationUpdate");
        assert_eq!(activity.members_added.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn test_deserialize_minimal_activity() {
        let json = r#"{"type": "typing"}"#;
        let activity: Activity = serde_json::from_str(json).unwrap();
        assert_eq!(activity.activity_type, "typing");
        assert!(activity.text.is_none());
    }

    #[test]
    fn test_build_stream_info_entity() {
        let e = build_stream_info_entity(None, "informative", 0);
        assert_eq!(e["type"], "streaminfo");
        assert_eq!(e["streamType"], "informative");
        assert!(e.get("streamId").is_none());

        let e2 = build_stream_info_entity(Some("stream-1"), "streaming", 3);
        assert_eq!(e2["streamId"], "stream-1");
        assert_eq!(e2["streamSequence"], 3);
    }

    #[test]
    fn test_build_ai_generated_entity() {
        let e = build_ai_generated_entity();
        assert_eq!(e["type"], "https://schema.org/Message");
        assert_eq!(e["additionalType"][0], "AIGeneratedContent");
    }

    #[test]
    fn test_build_welcome_card() {
        let card = build_welcome_card("Aleph", &["What can you do?", "Help me"]);
        assert_eq!(card["type"], "AdaptiveCard");
        assert_eq!(card["version"], "1.5");
        assert_eq!(card["actions"].as_array().unwrap().len(), 2);
        assert_eq!(card["actions"][0]["title"], "What can you do?");
    }

    #[test]
    fn test_inject_ai_entity() {
        let mut a = Activity::text_message("test");
        assert!(a.entities.is_none());
        inject_ai_entity(&mut a);
        assert_eq!(a.entities.as_ref().unwrap().len(), 1);
        inject_ai_entity(&mut a);
        assert_eq!(a.entities.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_strip_mentions() {
        assert_eq!(strip_mentions("<at>Aleph</at> hello world"), "hello world");
        assert_eq!(strip_mentions("hello world"), "hello world");
        assert_eq!(strip_mentions("<at>Bot</at>  <at>User</at> hi"), "hi");
        assert_eq!(strip_mentions(""), "");
    }

    #[test]
    fn test_pick_status_text() {
        let text = pick_status_text();
        assert!(STATUS_TEXTS.contains(&text));
    }
}
```

- [ ] **Step 2: Create mod.rs stub so the module compiles**

Create `src/gateway/interfaces/msteams/mod.rs`:
```rust
//! Microsoft Teams Channel
//!
//! Pure HTTP implementation against Bot Framework REST API v3.

pub mod types;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib msteams::types`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/msteams/
git commit -m "msteams: add Bot Framework Activity types and entity builders"
```

---

## Task 3: Configuration (config.rs)

**Files:**
- Create: `src/gateway/interfaces/msteams/config.rs`
- Modify: `src/gateway/interfaces/msteams/mod.rs` — add `pub mod config;`

- [ ] **Step 1: Write config with tests**

```rust
//! Microsoft Teams Channel Configuration

use serde::{Deserialize, Serialize};

/// Teams channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsTeamsConfig {
    /// Azure Bot App ID
    pub app_id: String,
    /// Azure Bot App Password (client secret)
    pub app_password: String,
    /// Azure AD Tenant ID (default: "common")
    #[serde(default = "default_tenant")]
    pub tenant_id: String,
    /// Allowed user AAD IDs (empty = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Allow group/team messages
    #[serde(default = "default_true")]
    pub groups_allowed: bool,
    /// Webhook path
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,
    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,
    /// Maximum retries for failed messages
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_tenant() -> String { "common".into() }
fn default_true() -> bool { true }
fn default_webhook_path() -> String { "/msteams/messages".into() }
fn default_max_retries() -> u32 { 3 }

impl Default for MsTeamsConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            app_password: String::new(),
            tenant_id: default_tenant(),
            allowed_users: Vec::new(),
            groups_allowed: true,
            webhook_path: default_webhook_path(),
            send_typing: true,
            max_retries: 3,
        }
    }
}

impl MsTeamsConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.app_id.is_empty() {
            return Err("app_id is required".into());
        }
        if self.app_password.is_empty() {
            return Err("app_password is required".into());
        }
        if !self.webhook_path.starts_with('/') {
            return Err("webhook_path must start with '/'".into());
        }
        Ok(())
    }

    /// Check if a user AAD ID is allowed
    pub fn is_user_allowed(&self, aad_id: &str) -> bool {
        self.allowed_users.is_empty() || self.allowed_users.iter().any(|id| id == aad_id)
    }

    /// Check if a conversation type is allowed
    pub fn is_conversation_allowed(&self, conversation_type: Option<&str>) -> bool {
        match conversation_type {
            Some("groupChat") | Some("channel") => self.groups_allowed,
            _ => true, // personal or unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MsTeamsConfig::default();
        assert_eq!(config.tenant_id, "common");
        assert!(config.groups_allowed);
        assert_eq!(config.webhook_path, "/msteams/messages");
    }

    #[test]
    fn test_validate() {
        let mut config = MsTeamsConfig::default();
        assert!(config.validate().is_err());

        config.app_id = "app-id".into();
        assert!(config.validate().is_err()); // still no password

        config.app_password = "secret".into();
        assert!(config.validate().is_ok());

        config.webhook_path = "no-slash".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_is_user_allowed() {
        let mut config = MsTeamsConfig::default();
        assert!(config.is_user_allowed("anyone")); // empty = allow all

        config.allowed_users = vec!["user-1".into(), "user-2".into()];
        assert!(config.is_user_allowed("user-1"));
        assert!(!config.is_user_allowed("user-3"));
    }

    #[test]
    fn test_is_conversation_allowed() {
        let mut config = MsTeamsConfig::default();
        assert!(config.is_conversation_allowed(Some("personal")));
        assert!(config.is_conversation_allowed(Some("groupChat")));

        config.groups_allowed = false;
        assert!(config.is_conversation_allowed(Some("personal")));
        assert!(!config.is_conversation_allowed(Some("groupChat")));
        assert!(!config.is_conversation_allowed(Some("channel")));
    }

    #[test]
    fn test_deserialize_from_json() {
        let json = r#"{"app_id": "my-app", "app_password": "secret"}"#;
        let config: MsTeamsConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.app_id, "my-app");
        assert_eq!(config.tenant_id, "common"); // default
        assert!(config.groups_allowed); // default
    }
}
```

- [ ] **Step 2: Add `pub mod config;` to mod.rs**

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib msteams::config`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/msteams/
git commit -m "msteams: add channel configuration with validation"
```

---

## Task 4: Authentication (auth.rs)

**Files:**
- Create: `src/gateway/interfaces/msteams/auth.rs`
- Modify: `src/gateway/interfaces/msteams/mod.rs` — add `pub mod auth;`

- [ ] **Step 1: Implement TokenCache and JwtValidator**

TokenCache handles outbound Bot token (OAuth2 client_credentials). JwtValidator handles inbound JWT verification (JWKS key caching).

Key details:
- `TokenCache::get_token()` — returns cached token or refreshes via POST to `login.microsoftonline.com`
- `JwtValidator::validate()` — synchronous validation against pre-fetched JWKS keys. Returns `bool`. Logs failure reasons via `tracing::warn!`
- `JwtValidator::refresh_keys()` — async fetch of JWKS from Microsoft OpenID metadata endpoint
- Allowed service URL domains: `*.botframework.com`, `smba.trafficmanager.net`

**Sync/async constraint**: `WebhookHandler::verify()` is `fn` (not async), so JWKS keys must use `std::sync::RwLock<Vec<DecodingKey>>` (not tokio RwLock). Keys are pre-fetched in `MsTeamsChannel::start()` and refreshed by a background `tokio::spawn` task every 24 hours. If validation fails due to unknown key ID, the request is rejected (Microsoft will retry), and the background task is signaled to refresh immediately.

```rust
//! Microsoft Teams Authentication
//!
//! - Inbound: JWT validation of Bot Framework webhook requests
//! - Outbound: OAuth2 token cache for Bot Framework REST API calls

use std::time::{Duration, Instant};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation, Algorithm};
use reqwest::Client;
use tokio::sync::RwLock;
use tracing::{debug, warn, info};

use crate::gateway::channel::ChannelError;

// ... (full implementation per spec section A2)
// TokenCache with RwLock<Option<CachedToken>>
// JwtValidator with RwLock<Vec<DecodingKey>> + last_refresh timestamp
// Allowed issuers: "https://api.botframework.com", "https://sts.windows.net/{tenant}/"
// Validate: audience == app_id, exp > now, signature via JWKS
// Service URL validation: must match allowed domains
```

Write unit tests with test JWT tokens (self-signed with known keys for testing):
- `test_token_cache_creates_new_token` (mock HTTP)
- `test_token_cache_returns_cached` (doesn't re-fetch within TTL)
- `test_jwt_validation_rejects_expired`
- `test_jwt_validation_rejects_wrong_audience`
- `test_service_url_validation`

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib msteams::auth`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/msteams/auth.rs
git commit -m "msteams: add JWT validation and OAuth token cache"
```

---

## Task 5: Bot Framework REST API client (api.rs)

**Files:**
- Create: `src/gateway/interfaces/msteams/api.rs`
- Modify: `src/gateway/interfaces/msteams/mod.rs` — add `pub mod api;`

- [ ] **Step 1: Implement BotFrameworkClient**

Thin HTTP wrapper using `reqwest::Client` + `TokenCache`:
- `send_activity()` — POST to `/v3/conversations/{id}/activities`
- `reply_to_activity()` — POST to `/v3/conversations/{id}/activities/{activityId}`
- `update_activity()` — PUT to `/v3/conversations/{id}/activities/{activityId}`
- `delete_activity()` — DELETE to `/v3/conversations/{id}/activities/{activityId}`
- `send_typing()` — POST typing activity

All methods:
1. Get token from `TokenCache`
2. Set `Authorization: Bearer {token}` header
3. POST/PUT/DELETE with JSON body
4. Map HTTP errors to `ChannelError` variants (401→AuthFailed, 429→RateLimited, etc.)

Write tests:
- `test_send_activity_url_construction` (verify URL format without actual HTTP)
- `test_error_mapping` (map HTTP status codes to ChannelError)

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib msteams::api`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/msteams/api.rs
git commit -m "msteams: add Bot Framework REST API client"
```

---

## Task 6: Core abstractions — StreamProtocol + NativeStreamHandler

**Files:**
- Modify: `src/gateway/channel.rs:291-319` — add `stream_protocol` to `ChannelCapabilities`
- Modify: `src/gateway/channel.rs` — add `StreamProtocol` enum, `NativeStreamHandler` trait, `native_stream_handler()` default method

- [ ] **Step 1: Add StreamProtocol enum**

After `ChannelCapabilities` struct definition in `channel.rs`, add:

```rust
/// Streaming protocol supported by a channel
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamProtocol {
    /// No streaming — buffer and send on completion
    None,
    /// Send initial message, then repeatedly edit it (Telegram, Discord)
    EditBased,
    /// Channel handles streaming natively (Teams streaminfo)
    Native,
}

impl Default for StreamProtocol {
    fn default() -> Self { Self::None }
}
```

Add to `ChannelCapabilities`:
```rust
    /// Streaming protocol supported by this channel
    #[serde(default)]
    pub stream_protocol: StreamProtocol,
```

- [ ] **Step 2: Add NativeStreamHandler trait**

After the `Channel` trait definition in `channel.rs`:

```rust
/// Handler for channels that support native streaming protocols (e.g., Teams streaminfo)
#[async_trait]
pub trait NativeStreamHandler: Send + Sync {
    /// Start streaming — send initial status indicator, returns stream_id
    async fn stream_start(&self, conversation_id: &ConversationId, status_text: &str) -> ChannelResult<String>;
    /// Send accumulated text as streaming update
    async fn stream_update(&self, conversation_id: &ConversationId, stream_id: &str, text: &str, sequence: u32) -> ChannelResult<()>;
    /// Finalize stream with complete message
    async fn stream_finalize(&self, conversation_id: &ConversationId, stream_id: &str, message: OutboundMessage) -> ChannelResult<SendResult>;
}
```

Add default method to `Channel` trait:
```rust
    /// Get native stream handler (if channel supports StreamProtocol::Native)
    fn native_stream_handler(&self) -> Option<Arc<dyn NativeStreamHandler>> {
        None
    }
```

Note: returns `Arc<dyn NativeStreamHandler>` (not `&dyn`) so ReplyEmitter can hold it across await points.

**Important**: The existing typewriter mode uses `config.stream_enabled` (boolean) to decide behavior, NOT `stream_protocol`. So adding `StreamProtocol` with default `None` does NOT break existing Telegram/Discord streaming. The `stream_protocol` field is used ONLY by the new ReplyEmitter native path (Task 9). Updating Telegram/Discord to declare `EditBased` is informational and can be done later.

- [ ] **Step 3: Verify existing tests still pass**

Run: `cargo test -p alephcore --lib channel`
Expected: All existing tests PASS (StreamProtocol defaults to None, no breaking changes)

- [ ] **Step 4: Commit**

```bash
git add src/gateway/channel.rs
git commit -m "msteams: add StreamProtocol enum and NativeStreamHandler trait to channel abstraction"
```

---

## Task 7: Channel implementation (mod.rs)

**Files:**
- Modify: `src/gateway/interfaces/msteams/mod.rs` — full MsTeamsChannel implementation

- [ ] **Step 1: Implement MsTeamsChannel**

The Channel trait implementation:
- `info()` / `state()` — standard ChannelInfo/ChannelState returns
- `start()` — initialize auth (refresh JWKS keys, get initial bot token), register webhook handler, set status Connected
- `stop()` — set status Disconnected
- `send()` — build outbound Activity from OutboundMessage, inject AI entity, send via BotFrameworkClient
- `send_typing()` — delegate to BotFrameworkClient
- `edit()` — delegate to BotFrameworkClient.update_activity
- `delete()` — delegate to BotFrameworkClient.delete_activity
- `native_stream_handler()` — return `Some(Arc::clone(&self))` (MsTeamsChannel implements NativeStreamHandler in streaming.rs)

Also implement `WebhookHandler`:
- `verify()` — extract Bearer token from Authorization header, validate JWT synchronously
- `handle()` — parse Activity, route by type:
  - `"message"` → check access control (allowed_users, groups_allowed), strip mentions, convert to InboundMessage, cache ConversationReference
  - `"conversationUpdate"` + bot in membersAdded → send welcome card
  - Other → log and return empty vec
- `path()` — return `self.config.webhook_path`

ConversationReference cache: `RwLock<HashMap<String, ConversationReference>>` with LRU eviction at 10,000 entries.

- [ ] **Step 2: Run compilation check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Write integration test for WebhookHandler**

Test the webhook handler with a mock JWT (bypass validation for this test) and a real Activity JSON payload. Verify:
- Valid message → produces InboundMessage with correct fields
- Group message with `groups_allowed=false` → rejected (empty vec)
- Message with `<at>Bot</at>` mention → mention stripped
- conversationUpdate → returns empty vec (welcome card is fire-and-forget side effect)

Run: `cargo test -p alephcore --lib msteams`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/msteams/
git commit -m "msteams: implement Channel trait with WebhookHandler and access control"
```

---

## Task 8: Native streaming implementation (streaming.rs)

**Files:**
- Create: `src/gateway/interfaces/msteams/streaming.rs`
- Modify: `src/gateway/interfaces/msteams/mod.rs` — add `pub mod streaming;`

- [ ] **Step 1: Implement NativeStreamHandler for MsTeamsChannel**

Implement the three methods using `build_stream_info_entity()` from types.rs:
- `stream_start()` — send typing + informative streaminfo → return activity ID
- `stream_update()` — send typing + streaming streaminfo with sequence
- `stream_finalize()` — send message + final streaminfo + AI entity

- [ ] **Step 2: Write tests**

Test entity construction in stream methods (mock the HTTP client to capture outbound activities):
- `test_stream_start_sends_informative_entity`
- `test_stream_update_sends_streaming_entity_with_sequence`
- `test_stream_finalize_sends_final_entity_with_ai_label`

Run: `cargo test -p alephcore --lib msteams::streaming`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/msteams/streaming.rs
git commit -m "msteams: implement NativeStreamHandler with Teams streaminfo protocol"
```

---

## Task 9: ReplyEmitter native streaming integration

**Files:**
- Modify: `src/gateway/reply_emitter.rs`

- [ ] **Step 1: Add native streaming fields to ReplyEmitter**

Add to the struct:
```rust
/// Native stream handler (if channel supports StreamProtocol::Native)
native_handler: Option<Arc<dyn crate::gateway::channel::NativeStreamHandler>>,
/// Active native stream state
native_stream_state: Mutex<Option<NativeStreamState>>,
```

Add state struct:
```rust
struct NativeStreamState {
    stream_id: String,
    sequence: u32,
    last_update: Instant,
}
```

Initialize both as `None`/`false` in constructors. Add a setter:
```rust
pub fn with_native_handler(mut self, handler: Arc<dyn NativeStreamHandler>) -> Self {
    self.native_handler = Some(handler);
    self
}
```

- [ ] **Step 1.5: Add `get_native_stream_handler()` to ChannelRegistry**

In `src/gateway/channel_registry.rs`, add a method that queries a channel for its native stream handler:
```rust
/// Get native stream handler for a channel (if it supports StreamProtocol::Native)
pub async fn get_native_stream_handler(&self, channel_id: &ChannelId) -> Option<Arc<dyn NativeStreamHandler>> {
    let channels = self.channels.read().await;
    channels.get(channel_id)
        .and_then(|handle| handle.channel.native_stream_handler())
}
```

- [ ] **Step 1.6: Wire handler at ReplyEmitter construction site**

Find where `ReplyEmitter::with_config()` or `ReplyEmitter::new()` is called (grep for `ReplyEmitter::new\|ReplyEmitter::with_config` in the gateway code). At that callsite, after constructing the emitter, add:
```rust
// After: let emitter = ReplyEmitter::with_config(...);
let emitter = if let Some(handler) = channel_registry.get_native_stream_handler(&route.channel_id).await {
    emitter.with_native_handler(handler)
} else {
    emitter
};
```

This ensures the native handler is only wired when the channel actually provides one.

- [ ] **Step 2: Add native streaming logic to emit()**

In `StreamEvent::ResponseChunk` handler, after the existing buffer accumulation (`self.buffer.lock().await.push_str(&content)`) and before the typewriter mode comment, add the native streaming path from spec section B4.

In `StreamEvent::RunComplete` handler, before the existing buffer flush, add native stream finalization from spec section B4.

Use `AtomicBool` for the disable flag instead of setting `self.native_handler = None` (since `emit()` takes `&self`):
```rust
/// Whether native streaming has been disabled due to error
native_disabled: AtomicBool,
```

- [ ] **Step 3: Verify no regressions**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests PASS (native_handler is None by default, no behavior change)

- [ ] **Step 4: Commit**

```bash
git add src/gateway/reply_emitter.rs
git commit -m "msteams: integrate NativeStreamHandler into ReplyEmitter"
```

---

## Task 10: MessageOperations implementation (message_ops.rs)

**Files:**
- Create: `src/gateway/interfaces/msteams/message_ops.rs`
- Modify: `src/gateway/interfaces/msteams/mod.rs` — add `pub mod message_ops;`

- [ ] **Step 1: Implement MsTeamsMessageOps**

Following the exact trait from `builtin_tools/message/types.rs`:
- `channel_id()` → "msteams"
- `capabilities()` → reply=true, edit=true, react=false, delete=true, send=true
- `reply()` → resolve ConversationReference, reply_to_activity with AI entity
- `edit()` → resolve ConversationReference, update_activity
- `react()` → return MessageResult::failed (Phase 2)
- `delete()` → resolve ConversationReference, delete_activity
- `send()` → resolve ConversationReference, send_activity with AI entity

- [ ] **Step 2: Write tests**

```rust
#[test]
fn test_capabilities() {
    // Verify capabilities match spec: reply, edit, delete, send = true; react = false
}

#[test]
fn test_channel_id() {
    // Verify returns "msteams"
}
```

Run: `cargo test -p alephcore --lib msteams::message_ops`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/msteams/message_ops.rs
git commit -m "msteams: implement MessageOperations for edit/delete/reply/send"
```

---

## Task 11: Wire into channel registry + MessageOps registration

**Files:**
- Modify: `src/gateway/interfaces/mod.rs:41` — add module + re-exports
- Modify: `src/gateway/handlers/channel.rs:287-331` — add "msteams" factory arm
- Modify: wherever Telegram/Discord register their `MessageOps` (follow existing pattern)

- [ ] **Step 1: Add module to interfaces/mod.rs**

After `pub mod feishu;` (line 41), add:
```rust
pub mod msteams;
```

After the feishu re-export (line 60), add:
```rust
pub use msteams::{MsTeamsChannel, MsTeamsConfig};
```

- [ ] **Step 2: Add "msteams" to create_channel_from_config**

In `src/gateway/handlers/channel.rs`, add to the import block (after feishu import, ~line 300):
```rust
use crate::gateway::interfaces::msteams::{MsTeamsChannel, MsTeamsConfig};
```

Add match arm before `_ => None` (~line 328):
```rust
"msteams" => serde_json::from_value::<MsTeamsConfig>(config).ok()
    .and_then(|cfg| {
        if let Err(e) = cfg.validate() {
            tracing::warn!("Invalid msteams config: {}", e);
            return None;
        }
        Some(Box::new(MsTeamsChannel::new(id, cfg)) as _)
    }),
```

- [ ] **Step 3: Register MsTeamsMessageOps with MessageOperationsRegistry**

The `MessageOperationsRegistry` (in `builtin_tools/message/tool.rs`) must know about the Teams adapter. Search for where `TelegramMessageOps` gets registered (grep for `TelegramMessageOps::new` outside of test code and `message_ops.*register`) and follow the same pattern for `MsTeamsMessageOps`.

If there is no centralized registration point, add registration in `MsTeamsChannel::start()`:
```rust
// In MsTeamsChannel::start(), after auth init:
let msg_ops = Arc::new(MsTeamsMessageOps::new(
    Arc::clone(&self.client),
    Arc::clone(&self.conversation_refs),
));
// Register with the message operations registry (pass via AppContext or similar)
```

**Note:** The implementer must trace the actual registration path. Grep for `register.*MessageOps\|message_ops.*register\|TelegramMessageOps` in the startup code to find the pattern.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/mod.rs src/gateway/handlers/channel.rs
git commit -m "msteams: wire Teams channel into channel registry and MessageOps"
```

---

## Task 12: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests PASS (existing + new msteams tests)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Compile check for release**

Run: `cargo check -p alephcore --release`
Expected: PASS

- [ ] **Step 4: Final commit if any cleanup needed**

```bash
git add src/gateway/interfaces/msteams/ src/gateway/channel.rs src/gateway/reply_emitter.rs src/gateway/channel_registry.rs
git commit -m "msteams: final cleanup and lint fixes"
```
