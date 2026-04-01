# Feishu/Lark Channel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Feishu (飞书) / Lark as a new messaging channel to Aleph, supporting text + image messages via WebSocket long connection.

**Architecture:** Four new files in `src/gateway/interfaces/feishu/` following the existing Channel trait + factory pattern. `FeishuClient` encapsulates token management, WebSocket connection, and HTTP API calls. Three existing files need minor modifications to register the new channel type.

**Tech Stack:** Rust, tokio, reqwest (existing), tokio-tungstenite (existing), serde_json

**Spec:** `docs/superpowers/specs/2026-03-22-feishu-channel-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/interfaces/feishu/types.rs` | Create | API types, event enums, config struct |
| `src/gateway/interfaces/feishu/events.rs` | Create | WebSocket frame parsing → FeishuEvent |
| `src/gateway/interfaces/feishu/client.rs` | Create | FeishuClient: token, WebSocket, HTTP API |
| `src/gateway/interfaces/feishu/mod.rs` | Create | FeishuChannel, Channel trait impl, ChannelProvider |
| `src/gateway/interfaces/mod.rs` | Modify | Add `pub mod feishu;` and re-export |
| `src/gateway/handlers/channel.rs` | Modify | Add `"feishu"` arm to `create_channel_from_config()` |

---

### Task 1: Types and Config (`types.rs`)

**Files:**
- Create: `src/gateway/interfaces/feishu/types.rs`

- [ ] **Step 1: Create the types file with config and API types**

```rust
// src/gateway/interfaces/feishu/types.rs

use serde::{Deserialize, Serialize};

// ── Config ──

fn default_domain() -> String { "feishu".to_string() }
fn default_true() -> bool { true }

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
}

impl FeishuConfig {
    /// Resolve the base URL from the domain field.
    pub fn base_url(&self) -> String {
        match self.domain.as_str() {
            "feishu" => "https://open.feishu.cn".to_string(),
            "lark" => "https://open.larksuite.com".to_string(),
            custom => custom.trim_end_matches('/').to_string(),
        }
    }
}

// ── Chat Types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatType {
    P2p,
    Group,
}

// ── Mentions ──

#[derive(Debug, Clone)]
pub struct Mention {
    /// Placeholder key in message text (e.g., "@_user_1")
    pub key: String,
    /// User's open_id
    pub id: String,
    /// Display name
    pub name: String,
    /// Whether this mention refers to the bot itself
    pub is_bot: bool,
}

// ── Events ──

#[derive(Debug, Clone)]
pub enum FeishuEvent {
    MessageReceive {
        message_id: String,
        chat_id: String,
        chat_type: ChatType,
        sender_id: String,
        sender_name: Option<String>,
        message_type: String,
        content: String,
        mentions: Vec<Mention>,
        parent_id: Option<String>,
    },
    Unknown(String),
}

// ── WebSocket Frame Types ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsFrameType {
    Event,
    Ping,
    Pong,
}

// ── API Response Types ──

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub code: i32,
    pub msg: String,
    #[serde(rename = "app_access_token")]
    pub app_access_token: Option<String>,
    pub expire: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct WsEndpointResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<WsEndpointData>,
}

#[derive(Debug, Deserialize)]
pub struct WsEndpointData {
    #[serde(rename = "URL")]
    pub url: String,
    pub client_config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct BotInfoResponse {
    pub code: i32,
    pub msg: Option<String>,
    pub bot: Option<BotInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotInfo {
    pub app_name: Option<String>,
    pub open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<SendMessageData>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageData {
    pub message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadImageResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<UploadImageData>,
}

#[derive(Debug, Deserialize)]
pub struct UploadImageData {
    pub image_key: Option<String>,
}

// ── WebSocket Event Envelope ──

#[derive(Debug, Deserialize)]
pub struct WsEventEnvelope {
    pub header: Option<WsEventHeader>,
    pub event: Option<serde_json::Value>,
    /// Ping/pong frames may not have header/event
    #[serde(rename = "type")]
    pub frame_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WsEventHeader {
    pub event_id: Option<String>,
    pub event_type: Option<String>,
    pub token: Option<String>,
    pub create_time: Option<String>,
}

// ── Message Event Payload ──

#[derive(Debug, Deserialize)]
pub struct MessageEventPayload {
    pub sender: Option<MessageSender>,
    pub message: Option<MessageBody>,
}

#[derive(Debug, Deserialize)]
pub struct MessageSender {
    pub sender_id: Option<SenderIdContainer>,
    pub sender_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SenderIdContainer {
    pub open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessageBody {
    pub message_id: Option<String>,
    pub chat_id: Option<String>,
    pub chat_type: Option<String>,
    pub message_type: Option<String>,
    pub content: Option<String>,
    pub mentions: Option<Vec<MentionPayload>>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MentionPayload {
    pub key: Option<String>,
    pub id: Option<MentionId>,
    pub name: Option<String>,
    pub tenant_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MentionId {
    pub open_id: Option<String>,
}

// ── Text Content ──

#[derive(Debug, Deserialize)]
pub struct TextContent {
    pub text: Option<String>,
}
```

- [ ] **Step 2: Write unit tests for config**

Add at the bottom of `types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_url_feishu() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "feishu".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            require_mention: true,
        };
        assert_eq!(config.base_url(), "https://open.feishu.cn");
    }

    #[test]
    fn test_base_url_lark() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "lark".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            require_mention: true,
        };
        assert_eq!(config.base_url(), "https://open.larksuite.com");
    }

    #[test]
    fn test_base_url_custom() {
        let config = FeishuConfig {
            app_id: "cli_test".into(),
            app_secret: "secret".into(),
            domain: "https://my.feishu.internal/".into(),
            bot_name: None,
            dm_allowed: true,
            groups_allowed: false,
            require_mention: true,
        };
        assert_eq!(config.base_url(), "https://my.feishu.internal");
    }

    #[test]
    fn test_config_deserialization_defaults() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret123"
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.domain, "feishu");
        assert!(config.dm_allowed);
        assert!(!config.groups_allowed);
        assert!(config.require_mention);
        assert!(config.bot_name.is_none());
    }

    #[test]
    fn test_config_deserialization_full() {
        let json = serde_json::json!({
            "app_id": "cli_xxx",
            "app_secret": "secret123",
            "domain": "lark",
            "bot_name": "MyBot",
            "dm_allowed": false,
            "groups_allowed": true,
            "require_mention": false
        });
        let config: FeishuConfig = serde_json::from_value(json).unwrap();
        assert_eq!(config.domain, "lark");
        assert!(!config.dm_allowed);
        assert!(config.groups_allowed);
        assert!(!config.require_mention);
        assert_eq!(config.bot_name.as_deref(), Some("MyBot"));
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib feishu::types -- --nocapture`
Expected: All 5 tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/feishu/types.rs
git commit -m "feat(feishu): add types, config, and API response structs"
```

---

### Task 2: Event Parsing (`events.rs`)

**Files:**
- Create: `src/gateway/interfaces/feishu/events.rs`
- Read: `src/gateway/interfaces/feishu/types.rs` (uses types defined in Task 1)

- [ ] **Step 1: Create the events parser**

```rust
// src/gateway/interfaces/feishu/events.rs

use super::types::*;

/// Parse a raw WebSocket text frame into a FeishuEvent.
///
/// Returns `Ok(None)` for ping/pong frames (handled separately).
/// Returns `Ok(Some(event))` for business events.
/// Returns `Err` if the JSON is malformed.
pub fn parse_ws_frame(raw: &str) -> Result<Option<FeishuEvent>, String> {
    let envelope: WsEventEnvelope = serde_json::from_str(raw)
        .map_err(|e| format!("Failed to parse WS frame: {e}"))?;

    // Handle ping/pong frames (no header)
    if let Some(frame_type) = &envelope.frame_type {
        match frame_type.as_str() {
            "ping" | "pong" => return Ok(None),
            _ => {}
        }
    }

    let header = match &envelope.header {
        Some(h) => h,
        None => return Ok(Some(FeishuEvent::Unknown("no header".to_string()))),
    };

    let event_type = header.event_type.as_deref().unwrap_or("");

    match event_type {
        "im.message.receive_v1" => parse_message_event(&envelope),
        other => Ok(Some(FeishuEvent::Unknown(other.to_string()))),
    }
}

fn parse_message_event(envelope: &WsEventEnvelope) -> Result<Option<FeishuEvent>, String> {
    let event_value = match &envelope.event {
        Some(v) => v,
        None => return Ok(Some(FeishuEvent::Unknown("message event without body".to_string()))),
    };

    let payload: MessageEventPayload = serde_json::from_value(event_value.clone())
        .map_err(|e| format!("Failed to parse message event: {e}"))?;

    let message = payload.message.as_ref()
        .ok_or_else(|| "Missing message field".to_string())?;

    let sender_id = payload.sender.as_ref()
        .and_then(|s| s.sender_id.as_ref())
        .and_then(|sid| sid.open_id.clone())
        .unwrap_or_default();

    let chat_type = match message.chat_type.as_deref() {
        Some("p2p") => ChatType::P2p,
        _ => ChatType::Group,
    };

    let mentions = message.mentions.as_ref()
        .map(|ms| ms.iter().map(|m| Mention {
            key: m.key.clone().unwrap_or_default(),
            id: m.id.as_ref().and_then(|mid| mid.open_id.clone()).unwrap_or_default(),
            name: m.name.clone().unwrap_or_default(),
            is_bot: false, // Will be determined by comparing with bot's open_id
        }).collect())
        .unwrap_or_default();

    Ok(Some(FeishuEvent::MessageReceive {
        message_id: message.message_id.clone().unwrap_or_default(),
        chat_id: message.chat_id.clone().unwrap_or_default(),
        chat_type,
        sender_id,
        sender_name: None, // Feishu doesn't include sender name in event
        message_type: message.message_type.clone().unwrap_or_default(),
        content: message.content.clone().unwrap_or_default(),
        mentions,
        parent_id: message.parent_id.clone(),
    }))
}

/// Extract text from a Feishu message content JSON string.
///
/// For "text" type: parses `{"text": "..."}` and returns the text.
/// Removes bot mention placeholders from the text.
pub fn extract_text_content(content: &str, mentions: &[Mention]) -> Option<String> {
    let parsed: TextContent = serde_json::from_str(content).ok()?;
    let mut text = parsed.text?;

    // Remove bot mention placeholders (e.g., "@_user_1 ")
    for mention in mentions {
        if mention.is_bot {
            // Remove the placeholder and any trailing space
            text = text.replace(&format!("{} ", mention.key), "");
            text = text.replace(&mention.key, "");
        }
    }

    let text = text.trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// Mark mentions that refer to the bot, given the bot's open_id.
pub fn mark_bot_mentions(mentions: &mut [Mention], bot_open_id: &str) {
    for mention in mentions.iter_mut() {
        if mention.id == bot_open_id {
            mention.is_bot = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ping_frame() {
        let raw = r#"{"type": "ping"}"#;
        let result = parse_ws_frame(raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_pong_frame() {
        let raw = r#"{"type": "pong"}"#;
        let result = parse_ws_frame(raw).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_unknown_event() {
        let raw = r#"{
            "header": {"event_id": "e1", "event_type": "im.chat.created_v1", "token": "t"},
            "event": {}
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::Unknown(t) => assert_eq!(t, "im.chat.created_v1"),
            _ => panic!("Expected Unknown event"),
        }
    }

    #[test]
    fn test_parse_message_receive_p2p() {
        let raw = r#"{
            "header": {
                "event_id": "evt_123",
                "event_type": "im.message.receive_v1",
                "token": "tok",
                "create_time": "1700000000"
            },
            "event": {
                "sender": {
                    "sender_id": {"open_id": "ou_user1"},
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "msg_001",
                    "chat_id": "oc_chat1",
                    "chat_type": "p2p",
                    "message_type": "text",
                    "content": "{\"text\":\"Hello\"}",
                    "mentions": null,
                    "parent_id": null
                }
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::MessageReceive {
                message_id, chat_id, chat_type, sender_id, message_type, content, mentions, parent_id, ..
            } => {
                assert_eq!(message_id, "msg_001");
                assert_eq!(chat_id, "oc_chat1");
                assert_eq!(chat_type, ChatType::P2p);
                assert_eq!(sender_id, "ou_user1");
                assert_eq!(message_type, "text");
                assert_eq!(content, "{\"text\":\"Hello\"}");
                assert!(mentions.is_empty());
                assert!(parent_id.is_none());
            }
            _ => panic!("Expected MessageReceive"),
        }
    }

    #[test]
    fn test_parse_message_receive_group_with_mention() {
        let raw = r#"{
            "header": {
                "event_id": "evt_456",
                "event_type": "im.message.receive_v1",
                "token": "tok"
            },
            "event": {
                "sender": {
                    "sender_id": {"open_id": "ou_user2"},
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "msg_002",
                    "chat_id": "oc_group1",
                    "chat_type": "group",
                    "message_type": "text",
                    "content": "{\"text\":\"@_user_1 What is Rust?\"}",
                    "mentions": [
                        {"key": "@_user_1", "id": {"open_id": "ou_bot"}, "name": "Aleph"}
                    ],
                    "parent_id": "msg_001"
                }
            }
        }"#;
        let result = parse_ws_frame(raw).unwrap().unwrap();
        match result {
            FeishuEvent::MessageReceive {
                chat_type, mentions, parent_id, ..
            } => {
                assert_eq!(chat_type, ChatType::Group);
                assert_eq!(mentions.len(), 1);
                assert_eq!(mentions[0].key, "@_user_1");
                assert_eq!(mentions[0].id, "ou_bot");
                assert_eq!(mentions[0].name, "Aleph");
                assert_eq!(parent_id, Some("msg_001".to_string()));
            }
            _ => panic!("Expected MessageReceive"),
        }
    }

    #[test]
    fn test_extract_text_content_plain() {
        let content = r#"{"text": "Hello world"}"#;
        let result = extract_text_content(content, &[]);
        assert_eq!(result, Some("Hello world".to_string()));
    }

    #[test]
    fn test_extract_text_content_with_bot_mention() {
        let content = r#"{"text": "@_user_1 What is Rust?"}"#;
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            id: "ou_bot".into(),
            name: "Aleph".into(),
            is_bot: true,
        }];
        let result = extract_text_content(content, &mentions);
        assert_eq!(result, Some("What is Rust?".to_string()));
    }

    #[test]
    fn test_extract_text_content_only_mention() {
        let content = r#"{"text": "@_user_1"}"#;
        let mentions = vec![Mention {
            key: "@_user_1".into(),
            id: "ou_bot".into(),
            name: "Aleph".into(),
            is_bot: true,
        }];
        let result = extract_text_content(content, &mentions);
        assert!(result.is_none());
    }

    #[test]
    fn test_mark_bot_mentions() {
        let mut mentions = vec![
            Mention { key: "@_user_1".into(), id: "ou_bot".into(), name: "Bot".into(), is_bot: false },
            Mention { key: "@_user_2".into(), id: "ou_human".into(), name: "Human".into(), is_bot: false },
        ];
        mark_bot_mentions(&mut mentions, "ou_bot");
        assert!(mentions[0].is_bot);
        assert!(!mentions[1].is_bot);
    }

    #[test]
    fn test_parse_invalid_json() {
        let raw = "not json at all";
        let result = parse_ws_frame(raw);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib feishu::events -- --nocapture`
Expected: All 9 tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/feishu/events.rs
git commit -m "feat(feishu): add WebSocket event parsing and text extraction"
```

---

### Task 3: Feishu HTTP Client (`client.rs`)

**Files:**
- Create: `src/gateway/interfaces/feishu/client.rs`
- Read: `src/gateway/interfaces/feishu/types.rs`

**Reference docs:**
- Feishu Token API: `POST /open-apis/auth/v3/app_access_token/internal`
- Feishu WS endpoint: `POST /open-apis/callback/ws/endpoint`
- Feishu Send Message: `POST /open-apis/im/v1/messages?receive_id_type=chat_id`
- Feishu Reply: `POST /open-apis/im/v1/messages/{message_id}/reply`
- Feishu Upload Image: `POST /open-apis/im/v1/images`
- Feishu Download: `GET /open-apis/im/v1/messages/{message_id}/resources/{file_key}?type=image`
- Feishu Bot Info: `GET /open-apis/bot/v3/info`

- [ ] **Step 1: Create the client**

```rust
// src/gateway/interfaces/feishu/client.rs

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use reqwest::multipart;

use super::types::*;

const TOKEN_REFRESH_MARGIN_SECS: u64 = 300; // Refresh 5 min before expiry
const DEFAULT_TOKEN_EXPIRY_SECS: u64 = 7200; // 2 hours

/// Error type for send operations, distinguishes rate limiting.
#[derive(Debug)]
pub enum FeishuSendError {
    RateLimited { retry_after_secs: u64 },
    Other(String),
}

/// Feishu API client handling token lifecycle and HTTP calls.
pub struct FeishuClient {
    app_id: String,
    app_secret: String,
    base_url: String,
    http: reqwest::Client,
    token: Arc<RwLock<TokenState>>,
    bot_open_id: Arc<RwLock<Option<String>>>,
}

pub(super) struct TokenState {
    pub(super) access_token: String,
    pub(super) expires_at: Instant,
}

impl TokenState {
    fn needs_refresh(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

impl FeishuClient {
    /// Create a new client. Does NOT acquire a token yet — call `refresh_token()` first.
    pub fn new(config: &FeishuConfig) -> Self {
        let http = reqwest::Client::new();

        Self {
            app_id: config.app_id.clone(),
            app_secret: config.app_secret.clone(),
            base_url: config.base_url(),
            http,
            token: Arc::new(RwLock::new(TokenState {
                access_token: String::new(),
                expires_at: Instant::now(), // Immediately expired → forces first refresh
            })),
            bot_open_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get the bot's open_id (available after get_bot_info).
    pub async fn bot_open_id(&self) -> Option<String> {
        self.bot_open_id.read().await.clone()
    }

    // ── Token Management ──

    /// Acquire or refresh the app_access_token.
    pub async fn refresh_token(&self) -> Result<(), String> {
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

    /// Get the current access token, refreshing if needed.
    async fn get_token(&self) -> Result<String, String> {
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

    /// Spawn a background task that refreshes the token before expiry.
    pub fn spawn_token_refresh(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        let app_id = self.app_id.clone();
        let app_secret = self.app_secret.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();
        let token = self.token.clone();

        tokio::spawn(async move {
            let mut shutdown = shutdown;
            loop {
                // Sleep until near expiry
                let sleep_duration = {
                    let state = token.read().await;
                    let now = Instant::now();
                    if state.expires_at > now {
                        state.expires_at.duration_since(now)
                    } else {
                        Duration::from_secs(60) // Fallback interval
                    }
                };

                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {}
                    _ = shutdown.changed() => {
                        tracing::debug!("Token refresh task shutting down");
                        return;
                    }
                }

                // Refresh
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

    // ── Bot Info ──

    /// Get bot info (health check + retrieve bot open_id).
    pub async fn get_bot_info(&self) -> Result<BotInfo, String> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/bot/v3/info", self.base_url);

        let resp = self.http.get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Bot info request failed: {e}"))?;

        let info: BotInfoResponse = resp.json().await
            .map_err(|e| format!("Bot info parse failed: {e}"))?;

        if info.code != 0 {
            return Err(format!("Bot info error: code={}, msg={}", info.code, info.msg.unwrap_or_default()));
        }

        let bot = info.bot.ok_or_else(|| "No bot info in response".to_string())?;

        // Cache bot open_id
        if let Some(ref oid) = bot.open_id {
            *self.bot_open_id.write().await = Some(oid.clone());
        }

        Ok(bot)
    }

    // ── WebSocket Endpoint ──

    /// Get a cloneable handle for the WS reconnect loop.
    /// Returns (http_client, base_url, token_arc) so the spawned task can re-fetch endpoints.
    pub fn ws_reconnect_handle(&self) -> (reqwest::Client, String, Arc<RwLock<TokenState>>) {
        (self.http.clone(), self.base_url.clone(), self.token.clone())
    }

    /// Get the WebSocket endpoint URL for event subscription.
    pub async fn get_ws_endpoint(&self) -> Result<String, String> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/callback/ws/endpoint", self.base_url);

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| format!("WS endpoint request failed: {e}"))?;

        let ws_resp: WsEndpointResponse = resp.json().await
            .map_err(|e| format!("WS endpoint parse failed: {e}"))?;

        if ws_resp.code != 0 {
            return Err(format!("WS endpoint error: code={}, msg={}", ws_resp.code, ws_resp.msg));
        }

        let data = ws_resp.data.ok_or_else(|| "No data in WS endpoint response".to_string())?;
        Ok(data.url)
    }

    // ── Send Messages ──

    /// Send a text message to a chat.
    pub async fn send_text(&self, chat_id: &str, text: &str, reply_to: Option<&str>) -> Result<String, FeishuSendError> {
        if let Some(msg_id) = reply_to {
            return self.reply_message(msg_id, "text", &serde_json::json!({"text": text}).to_string()).await;
        }

        let token = self.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "text",
            "content": serde_json::json!({"text": text}).to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send text failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    /// Send an image message to a chat.
    pub async fn send_image(&self, chat_id: &str, image_key: &str, reply_to: Option<&str>) -> Result<String, FeishuSendError> {
        if let Some(msg_id) = reply_to {
            return self.reply_message(msg_id, "image", &serde_json::json!({"image_key": image_key}).to_string()).await;
        }

        let token = self.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages?receive_id_type=chat_id", self.base_url);

        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "image",
            "content": serde_json::json!({"image_key": image_key}).to_string(),
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Send image failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    /// Reply to a specific message.
    async fn reply_message(&self, message_id: &str, msg_type: &str, content: &str) -> Result<String, FeishuSendError> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/im/v1/messages/{}/reply", self.base_url, message_id);

        let body = serde_json::json!({
            "msg_type": msg_type,
            "content": content,
        });

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| FeishuSendError::Other(format!("Reply message failed: {e}")))?;

        self.parse_send_response(resp).await
    }

    async fn parse_send_response(&self, resp: reqwest::Response) -> Result<String, FeishuSendError> {
        // Check for rate limiting
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5);
            return Err(FeishuSendError::RateLimited { retry_after_secs: retry_after });
        }

        let send_resp: SendMessageResponse = resp.json().await
            .map_err(|e| FeishuSendError::Other(format!("Send response parse failed: {e}")))?;

        if send_resp.code != 0 {
            return Err(FeishuSendError::Other(format!("Send error: code={}, msg={}", send_resp.code, send_resp.msg)));
        }

        let msg_id = send_resp.data
            .and_then(|d| d.message_id)
            .unwrap_or_default();

        Ok(msg_id)
    }

    // ── Media ──

    /// Upload an image and return the image_key.
    pub async fn upload_image(&self, data: Vec<u8>, filename: &str) -> Result<String, String> {
        let token = self.get_token().await?;
        let url = format!("{}/open-apis/im/v1/images", self.base_url);

        let part = multipart::Part::bytes(data)
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .map_err(|e| format!("Multipart error: {e}"))?;

        let form = multipart::Form::new()
            .text("image_type", "message")
            .part("image", part);

        let resp = self.http.post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Upload image failed: {e}"))?;

        let upload_resp: UploadImageResponse = resp.json().await
            .map_err(|e| format!("Upload response parse failed: {e}"))?;

        if upload_resp.code != 0 {
            return Err(format!("Upload error: code={}, msg={}", upload_resp.code, upload_resp.msg));
        }

        upload_resp.data
            .and_then(|d| d.image_key)
            .ok_or_else(|| "No image_key in upload response".to_string())
    }

    /// Download media (image/file) from a message.
    pub async fn download_media(&self, message_id: &str, file_key: &str) -> Result<Vec<u8>, String> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{}/resources/{}?type=image",
            self.base_url, message_id, file_key
        );

        let resp = self.http.get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| format!("Download media failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Download media HTTP {}", resp.status()));
        }

        resp.bytes().await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Download media read failed: {e}"))
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: Compiles without errors (client.rs is only used internally, no external integration yet)

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/feishu/client.rs
git commit -m "feat(feishu): add FeishuClient with token, HTTP API, and media support"
```

---

### Task 4: Channel Implementation (`mod.rs`)

**Files:**
- Create: `src/gateway/interfaces/feishu/mod.rs`
- Read: `src/gateway/channel.rs:418-546` (Channel, ChannelProvider, ChannelFactory traits)
- Read: `src/thinker/interaction.rs` (InteractionManifest, InteractionParadigm)
- Reference: `src/gateway/interfaces/telegram/mod.rs` (pattern to follow)

- [ ] **Step 1: Create mod.rs with FeishuChannel**

```rust
// src/gateway/interfaces/feishu/mod.rs

pub mod types;
pub mod events;
pub mod client;

use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use chrono::Utc;
use tokio::sync::watch;
use async_trait::async_trait;
use futures_util::StreamExt;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelInfo, ChannelId,
    ChannelProvider, ChannelResult, ChannelState, ChannelStatus, ConversationId,
    InboundMessage, MessageId, OutboundMessage, SendResult, UserId,
};
use crate::thinker::interaction::{
    InteractionConstraints, InteractionManifest, InteractionParadigm,
};

pub use types::FeishuConfig;
use client::{FeishuClient, FeishuSendError};
use events::{extract_text_content, mark_bot_mentions, parse_ws_frame};
use types::{ChatType, FeishuEvent};

/// Map FeishuSendError to ChannelError
fn map_send_error(e: FeishuSendError) -> ChannelError {
    match e {
        FeishuSendError::RateLimited { retry_after_secs } => {
            ChannelError::RateLimited { retry_after_secs }
        }
        FeishuSendError::Other(msg) => ChannelError::SendFailed(msg),
    }
}

const DEDUP_CAPACITY: usize = 1000;

pub struct FeishuChannel {
    info: ChannelInfo,
    config: FeishuConfig,
    channel_state: ChannelState,
    client: Option<FeishuClient>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl FeishuChannel {
    pub fn new(id: impl Into<String>, config: FeishuConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Feishu".to_string(),
            channel_type: "feishu".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            client: None,
            shutdown_tx: None,
        }
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: true,
            audio: false,
            video: false,
            reactions: false,
            replies: true,
            editing: false,
            deletion: false,
            typing_indicator: false,
            read_receipts: false,
            rich_text: false,
            max_message_length: 4096,
            max_attachment_size: 20 * 1024 * 1024,
        }
    }

}

#[async_trait]
impl Channel for FeishuChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.channel_state.set_status(ChannelStatus::Connecting).await;

        // Create client and acquire token
        let client = FeishuClient::new(&self.config);
        client.refresh_token().await
            .map_err(|e| ChannelError::AuthFailed(format!("Token acquisition failed: {e}")))?;

        // Validate credentials
        let bot_info = client.get_bot_info().await
            .map_err(|e| ChannelError::AuthFailed(format!("Bot info failed: {e}")))?;
        tracing::info!("Feishu bot connected: {:?}", bot_info.app_name);

        let bot_open_id = client.bot_open_id().await.unwrap_or_default();

        // Get WebSocket endpoint
        let ws_url = client.get_ws_endpoint().await
            .map_err(|e| ChannelError::Internal(format!("WS endpoint failed: {e}")))?;

        // Spawn shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Spawn token refresh
        client.spawn_token_refresh(shutdown_rx.clone());

        // Spawn WebSocket event loop
        let sender = self.channel_state.sender();
        let channel_id = self.info.id.clone();
        let config = self.config.clone();
        let status_handle = self.channel_state.status_handle();
        let dedup = std::sync::Arc::new(StdMutex::new(VecDeque::<String>::with_capacity(DEDUP_CAPACITY)));
        let (ws_http, ws_base_url, ws_token) = client.ws_reconnect_handle();

        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            let mut backoff_secs: u64 = 1;
            let mut current_url = ws_url;

            loop {
                // Check shutdown before connecting
                if *shutdown_rx.borrow() {
                    break;
                }

                tracing::info!("Connecting to Feishu WebSocket: {}...", current_url.get(..60).unwrap_or(&current_url));

                match tokio_tungstenite::connect_async(&current_url).await {
                    Ok((ws_stream, _)) => {
                        backoff_secs = 1; // Reset backoff on successful connection
                        *status_handle.write().await = ChannelStatus::Connected;
                        tracing::info!("Feishu WebSocket connected");

                        let (_, mut read) = ws_stream.split();

                        loop {
                            tokio::select! {
                                msg = read.next() => {
                                    match msg {
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                            match parse_ws_frame(&text) {
                                                Ok(Some(FeishuEvent::MessageReceive {
                                                    message_id, chat_id, chat_type, sender_id,
                                                    sender_name, message_type, content, mut mentions,
                                                    parent_id,
                                                })) => {
                                                    // Dedup check
                                                    {
                                                        let mut seen = dedup.lock().unwrap_or_else(|e| e.into_inner());
                                                        if seen.iter().any(|id| id == &message_id) {
                                                            continue;
                                                        }
                                                        if seen.len() >= DEDUP_CAPACITY {
                                                            seen.pop_front();
                                                        }
                                                        seen.push_back(message_id.clone());
                                                    }

                                                    // Mark bot mentions
                                                    mark_bot_mentions(&mut mentions, &bot_open_id);

                                                    // Group chat: check if mention required
                                                    if chat_type == ChatType::Group && config.require_mention {
                                                        let bot_mentioned = mentions.iter().any(|m| m.is_bot);
                                                        if !bot_mentioned {
                                                            continue;
                                                        }
                                                    }

                                                    // Group chat: skip if groups not allowed
                                                    if chat_type == ChatType::Group && !config.groups_allowed {
                                                        continue;
                                                    }

                                                    // DM: skip if DMs not allowed
                                                    if chat_type == ChatType::P2p && !config.dm_allowed {
                                                        continue;
                                                    }

                                                    // Extract text content
                                                    let text = match message_type.as_str() {
                                                        "text" => {
                                                            match extract_text_content(&content, &mentions) {
                                                                Some(t) => t,
                                                                None => continue, // Empty after mention removal
                                                            }
                                                        }
                                                        "image" => {
                                                            // For image-only messages, use placeholder text
                                                            "[Image]".to_string()
                                                        }
                                                        other => {
                                                            tracing::debug!("Skipping unsupported message type: {other}");
                                                            continue;
                                                        }
                                                    };

                                                    let inbound = InboundMessage {
                                                        id: MessageId::new(&message_id),
                                                        channel_id: channel_id.clone(),
                                                        conversation_id: ConversationId::new(&chat_id),
                                                        sender_id: UserId::new(&sender_id),
                                                        sender_name,
                                                        text,
                                                        attachments: Vec::new(), // TODO: download images for image messages
                                                        timestamp: Utc::now(),
                                                        reply_to: parent_id.map(MessageId::new),
                                                        is_group: chat_type == ChatType::Group,
                                                        raw: None,
                                                    };

                                                    if sender.send(inbound).await.is_err() {
                                                        tracing::warn!("Feishu inbound channel closed");
                                                        return;
                                                    }
                                                }
                                                Ok(Some(FeishuEvent::Unknown(t))) => {
                                                    tracing::debug!("Unknown Feishu event: {t}");
                                                }
                                                Ok(None) => {
                                                    // Ping/pong — handled by tungstenite automatically
                                                }
                                                Err(e) => {
                                                    tracing::warn!("Failed to parse Feishu WS frame: {e}");
                                                }
                                            }
                                        }
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(_))) => {
                                            // Tungstenite handles pong automatically
                                        }
                                        Some(Ok(_)) => {} // Binary, Close, etc.
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
                                _ = shutdown_rx.changed() => {
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

                // Reconnect with backoff
                *status_handle.write().await = ChannelStatus::Error;
                tracing::info!("Reconnecting in {backoff_secs}s...");

                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Feishu WS shutdown during backoff");
                        return;
                    }
                }

                backoff_secs = (backoff_secs * 2).min(60);

                // Re-fetch WS endpoint URL (old URLs may be expired)
                let token = ws_token.read().await.access_token.clone();
                let url = format!("{}/open-apis/callback/ws/endpoint", ws_base_url);
                match ws_http.post(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json; charset=utf-8")
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if let Ok(ws_resp) = resp.json::<super::types::WsEndpointResponse>().await {
                            if ws_resp.code == 0 {
                                if let Some(data) = ws_resp.data {
                                    current_url = data.url;
                                    tracing::debug!("Re-fetched WS endpoint URL");
                                }
                            }
                        }
                    }
                    Err(e) => tracing::warn!("Failed to re-fetch WS endpoint: {e}"),
                }
            }
        });

        self.client = Some(client);
        self.channel_state.set_status(ChannelStatus::Connected).await;

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        self.client = None;
        self.channel_state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let client = self.client.as_ref()
            .ok_or_else(|| ChannelError::NotConnected("Client not initialized".to_string()))?;

        let chat_id = message.conversation_id.as_str();
        let reply_to = message.reply_to.as_ref().map(|id| id.as_str());

        // Check for image attachments
        let has_image = message.attachments.iter().any(|a| a.mime_type.starts_with("image/"));

        let msg_id = if has_image {
            // Upload first image and send
            if let Some(attachment) = message.attachments.iter().find(|a| a.mime_type.starts_with("image/")) {
                let image_data = attachment.data.clone()
                    .ok_or_else(|| ChannelError::SendFailed("Image attachment has no data".to_string()))?;
                let filename = attachment.filename.as_deref().unwrap_or("image.png");
                let image_key = client.upload_image(image_data, filename).await
                    .map_err(|e| ChannelError::SendFailed(e))?;
                client.send_image(chat_id, &image_key, reply_to).await
                    .map_err(map_send_error)?
            } else {
                unreachable!()
            }
        } else {
            // Send text
            // If text is empty, skip
            if message.text.is_empty() {
                return Err(ChannelError::SendFailed("Empty message".to_string()));
            }
            client.send_text(chat_id, &message.text, reply_to).await
                .map_err(map_send_error)?
        };

        // Also send text alongside image if both present
        if has_image && !message.text.is_empty() {
            let _ = client.send_text(chat_id, &message.text, reply_to).await;
        }

        Ok(SendResult {
            message_id: MessageId::new(msg_id),
            timestamp: Utc::now(),
        })
    }
}

impl ChannelProvider for FeishuChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest::new(InteractionParadigm::Messaging)
            .with_constraints(
                InteractionConstraints::new()
                    .max_output_chars(4096)
                    .supports_streaming(false)
                    .prefer_compact(false),
            )
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: Compiles (mod.rs is not yet wired into the interfaces module, but should compile in isolation once wired)

Note: If there are compilation issues, they will surface in the next task when we add the module to `interfaces/mod.rs`. Fix any issues before proceeding.

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/feishu/mod.rs
git commit -m "feat(feishu): add FeishuChannel with Channel and ChannelProvider impls"
```

---

### Task 5: Wire Into Aleph (Registration)

**Files:**
- Modify: `src/gateway/interfaces/mod.rs:39` (add `pub mod feishu;` + re-export)
- Modify: `src/gateway/handlers/channel.rs:283-324` (add `"feishu"` arm)

- [ ] **Step 1: Add module declaration to `interfaces/mod.rs`**

After line 39 (`pub mod nostr;`), add:

```rust
pub mod feishu;
```

After line 57 (`pub use nostr::{...};`), add:

```rust
pub use feishu::{FeishuChannel, FeishuConfig};
```

- [ ] **Step 2: Add `"feishu"` arm to `create_channel_from_config` in `handlers/channel.rs`**

At line 295 (after the nostr import), add:

```rust
    use crate::gateway::interfaces::feishu::{FeishuChannel, FeishuConfig};
```

At line 321 (before `_ => None`), add:

```rust
        "feishu" => serde_json::from_value::<FeishuConfig>(config).ok()
            .map(|cfg| Box::new(FeishuChannel::new(id, cfg)) as _),
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: Compiles without errors. Fix any type mismatches or missing imports.

- [ ] **Step 4: Run all existing tests to verify no regressions**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests pass (plus new feishu tests)

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/mod.rs src/gateway/handlers/channel.rs
git commit -m "feat(feishu): wire Feishu channel into factory and module registry"
```

---

### Task 6: Final Verification and Doc Update

**Files:**
- Read: `src/gateway/interfaces/mod.rs` (verify module doc comment)
- Modify: `src/gateway/interfaces/mod.rs:1-22` (add Feishu to doc comment list)

- [ ] **Step 1: Update module doc comment**

In `src/gateway/interfaces/mod.rs`, add to the doc comment list (after Nostr line):

```rust
//! - **Feishu**: Feishu/Lark Bot WebSocket + REST API integration
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass including the new feishu tests

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all`
Expected: No warnings in feishu module

- [ ] **Step 4: Fix any clippy warnings**

Address any warnings found, then re-run clippy to confirm clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(feishu): final cleanup and doc update"
```

---

## Verification Checklist

After all tasks complete:

- [ ] `cargo check -p alephcore` passes
- [ ] `cargo test -p alephcore --lib` passes (all existing + new feishu tests)
- [ ] `cargo clippy -p alephcore` clean
- [ ] 4 new files created in `src/gateway/interfaces/feishu/`
- [ ] `create_channel_from_config` has `"feishu"` arm
- [ ] Module doc comment lists Feishu
- [ ] Config deserialization works with defaults
- [ ] Event parsing covers: text, image, ping/pong, unknown, malformed
- [ ] @mention extraction tested
- [ ] Domain → base_url mapping tested (feishu, lark, custom)
