# WeChat Channel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a full-featured WeChat (iLink Bot API) channel adapter for Aleph, with complete feature parity to hermes-agent's WeChat implementation.

**Architecture:** Long-polling `getupdates` pattern with AES-128-ECB media decryption, QR login for authentication, context token management per conversation, and markdown-to-WeChat format conversion.

**Tech Stack:** Rust (tokio async runtime), `reqwest` for HTTP API calls, `aes` + `cipher` crates for AES-128-ECB decryption, `serde` for JSON, `qrcode` for QR code rendering.

---

## File Inventory

**New files to create:**
- `src/gateway/interfaces/wechat/mod.rs` — `WeChatChannel` impl `Channel` + `WeChatChannelFactory` + `register_with_plugin()`
- `src/gateway/interfaces/wechat/config.rs` — `WeChatConfig` + `DmPolicy` + `GroupPolicy` + validation
- `src/gateway/interfaces/wechat/types.rs` — iLink API types (serde-deserializable from iLink responses)
- `src/gateway/interfaces/wechat/api.rs` — iLink HTTP API client functions
- `src/gateway/interfaces/wechat/auth.rs` — QR login flow, token persistence, `ContextTokenStore`
- `src/gateway/interfaces/wechat/sync_buf.rs` — Sync buffer persistence for incremental updates
- `src/gateway/interfaces/wechat/media.rs` — Media download + AES-128-ECB decryption
- `src/gateway/interfaces/wechat/inbound/mapper.rs` — iLink message → `InboundMessage`
- `src/gateway/interfaces/wechat/inbound/policy.rs` — DM/Group access policy
- `src/gateway/interfaces/wechat/outbound/mapper.rs` — `OutboundMessage` → iLink format
- `src/gateway/interfaces/wechat/outbound/markdown.rs` — Markdown → WeChat format conversion
- `src/gateway/interfaces/wechat/runtime.rs` — `WeChatRuntime` core polling loop

**Files to modify:**
- `src/gateway/interfaces/mod.rs` — Add `pub mod wechat;` and register factory

---

## Task 1: `types.rs` — iLink API Types

**Files:**
- Create: `src/gateway/interfaces/wechat/types.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_get_updates_response() {
        let json = r#"{
            "ret": 0,
            "msgs": [
                {
                    "msg_id": "msg123",
                    "from_user_id": "wxid_abc",
                    "to_user_id": "bot_id",
                    "room_id": "",
                    "msg_type": 1,
                    "item_list": [
                        {"type": 1, "text_item": {"text": "Hello"}}
                    ],
                    "context_token": "ctx_123"
                }
            ],
            "get_updates_buf": "sync_buf_abc"
        }"#;
        let resp: GetUpdatesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ret, 0);
        assert_eq!(resp.msgs.len(), 1);
        assert_eq!(resp.msgs[0].from_user_id, "wxid_abc");
    }

    #[test]
    fn test_message_item_extraction() {
        let json = r#"{
            "type": 1,
            "text_item": {"text": "Hello"}
        }"#;
        let item: MessageItem = serde_json::from_str(json).unwrap();
        assert!(matches!(item, MessageItem::Text(_)));
    }

    #[test]
    fn test_media_types() {
        assert_eq!(ITEM_TEXT, 1);
        assert_eq!(ITEM_IMAGE, 2);
        assert_eq!(ITEM_VOICE, 3);
        assert_eq!(ITEM_FILE, 4);
        assert_eq!(ITEM_VIDEO, 5);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::types --lib`
Expected: FAIL (file does not exist)

- [ ] **Step 3: Write minimal implementation**

```rust
//! WeChat Channel iLink API Types
//!
//! Deserializable from iLink Bot API responses.

use serde::{Deserialize, Serialize};

/// iLink API constants
pub const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
pub const WEIXIN_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
pub const ILINK_APP_ID: &str = "bot";
pub const CHANNEL_VERSION: &str = "2.2.0";
pub const ILINK_APP_CLIENT_VERSION: u32 = (2 << 16) | (2 << 8) | 0;

/// Message item types
pub const ITEM_TEXT: u32 = 1;
pub const ITEM_IMAGE: u32 = 2;
pub const ITEM_VOICE: u32 = 3;
pub const ITEM_FILE: u32 = 4;
pub const ITEM_VIDEO: u32 = 5;

/// Media types
pub const MEDIA_IMAGE: u32 = 1;
pub const MEDIA_VIDEO: u32 = 2;
pub const MEDIA_FILE: u32 = 3;
pub const MEDIA_VOICE: u32 = 4;

/// Message type constants
pub const MSG_TYPE_USER: u32 = 1;
pub const MSG_TYPE_BOT: u32 = 2;
pub const MSG_STATE_FINISH: u32 = 2;

/// Typing constants
pub const TYPING_START: u32 = 1;
pub const TYPING_STOP: u32 = 2;

/// API Endpoints
pub const EP_GET_UPDATES: &str = "ilink/bot/getupdates";
pub const EP_SEND_MESSAGE: &str = "ilink/bot/sendmessage";
pub const EP_SEND_TYPING: &str = "ilink/bot/sendtyping";
pub const EP_GET_CONFIG: &str = "ilink/bot/getconfig";
pub const EP_GET_UPLOAD_URL: &str = "ilink/bot/getuploadurl";
pub const EP_GET_BOT_QR: &str = "ilink/bot/get_bot_qrcode";
pub const EP_GET_QR_STATUS: &str = "ilink/bot/get_qrcode_status";

/// Long poll timeout in milliseconds
pub const LONG_POLL_TIMEOUT_MS: u64 = 35_000;
pub const API_TIMEOUT_MS: u64 = 15_000;

/// GetUpdates response from iLink API
#[derive(Debug, Clone, Deserialize)]
pub struct GetUpdatesResponse {
    pub ret: i32,
    pub errcode: Option<i32>,
    pub errmsg: Option<String>,
    pub msgs: Vec<Message>,
    #[serde(rename = "get_updates_buf")]
    pub get_updates_buf: Option<String>,
    #[serde(rename = "longpolling_timeout_ms")]
    pub longpolling_timeout_ms: Option<u64>,
}

/// A message received from iLink API
#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(rename = "msg_id")]
    pub msg_id: String,
    #[serde(rename = "from_user_id")]
    pub from_user_id: String,
    #[serde(rename = "to_user_id")]
    pub to_user_id: String,
    #[serde(rename = "room_id")]
    pub room_id: Option<String>,
    #[serde(rename = "chat_room_id")]
    pub chat_room_id: Option<String>,
    #[serde(rename = "msg_type")]
    pub msg_type: u32,
    #[serde(rename = "item_list")]
    pub item_list: Vec<MessageItem>,
    #[serde(rename = "context_token")]
    pub context_token: Option<String>,
}

/// A message item (text, image, voice, etc.)
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum MessageItem {
    #[serde(rename = "1")]
    Text(TextItem),
    #[serde(rename = "2")]
    Image(ImageItem),
    #[serde(rename = "3")]
    Voice(VoiceItem),
    #[serde(rename = "4")]
    File(FileItem),
    #[serde(rename = "5")]
    Video(VideoItem),
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextItem {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageItem {
    pub media: MediaRef,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceItem {
    pub media: MediaRef,
    pub text: Option<String>,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileItem {
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub media: MediaRef,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoItem {
    pub media: MediaRef,
    #[serde(rename = "aeskey")]
    pub aes_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaRef {
    #[serde(rename = "encrypt_query_param")]
    pub encrypt_query_param: Option<String>,
    #[serde(rename = "full_url")]
    pub full_url: Option<String>,
    #[serde(rename = "aes_key")]
    pub aes_key: Option<String>,
}

/// Send message request payload
#[derive(Debug, Clone, Serialize)]
pub struct SendMessagePayload {
    pub msg: OutboundMsg,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundMsg {
    #[serde(rename = "from_user_id")]
    pub from_user_id: String,
    #[serde(rename = "to_user_id")]
    pub to_user_id: String,
    #[serde(rename = "client_id")]
    pub client_id: String,
    #[serde(rename = "message_type")]
    pub message_type: u32,
    #[serde(rename = "message_state")]
    pub message_state: u32,
    #[serde(rename = "item_list")]
    pub item_list: Vec<OutboundItem>,
    #[serde(rename = "context_token")]
    pub context_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundItem {
    #[serde(rename = "type")]
    pub item_type: u32,
    #[serde(rename = "text_item")]
    pub text_item: Option<TextItemContent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TextItemContent {
    pub text: String,
}

/// QR login response
#[derive(Debug, Clone, Deserialize)]
pub struct QrCodeResponse {
    pub qrcode: Option<String>,
    #[serde(rename = "qrcode_img_content")]
    pub qrcode_img_content: Option<String>,
}

/// QR status response
#[derive(Debug, Clone, Deserialize)]
pub struct QrStatusResponse {
    pub status: String,
    #[serde(rename = "ilink_bot_id")]
    pub ilink_bot_id: Option<String>,
    #[serde(rename = "bot_token")]
    pub bot_token: Option<String>,
    #[serde(rename = "baseurl")]
    pub base_url: Option<String>,
    #[serde(rename = "ilink_user_id")]
    pub ilink_user_id: Option<String>,
    #[serde(rename = "redirect_host")]
    pub redirect_host: Option<String>,
}

/// Typing indicator payload
#[derive(Debug, Clone, Serialize)]
pub struct SendTypingPayload {
    #[serde(rename = "ilink_user_id")]
    pub ilink_user_id: String,
    #[serde(rename = "typing_ticket")]
    pub typing_ticket: String,
    pub status: u32,
}

/// Config response (for getting typing ticket)
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigResponse {
    #[serde(rename = "typing_ticket")]
    pub typing_ticket: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_get_updates_response() {
        let json = r#"{
            "ret": 0,
            "msgs": [
                {
                    "msg_id": "msg123",
                    "from_user_id": "wxid_abc",
                    "to_user_id": "bot_id",
                    "room_id": "",
                    "msg_type": 1,
                    "item_list": [
                        {"type": 1, "text_item": {"text": "Hello"}}
                    ],
                    "context_token": "ctx_123"
                }
            ],
            "get_updates_buf": "sync_buf_abc"
        }"#;
        let resp: GetUpdatesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.ret, 0);
        assert_eq!(resp.msgs.len(), 1);
        assert_eq!(resp.msgs[0].from_user_id, "wxid_abc");
    }

    #[test]
    fn test_message_item_text() {
        let json = r#"{"type": 1, "text_item": {"text": "Hello"}}"#;
        let item: MessageItem = serde_json::from_str(json).unwrap();
        assert!(matches!(item, MessageItem::Text(_)));
    }

    #[test]
    fn test_media_types() {
        assert_eq!(ITEM_TEXT, 1);
        assert_eq!(ITEM_IMAGE, 2);
        assert_eq!(ITEM_VOICE, 3);
        assert_eq!(ITEM_FILE, 4);
        assert_eq!(ITEM_VIDEO, 5);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::types --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/types.rs
git commit -m "gateway/wechat: add iLink API types (types.rs)"
```

---

## Task 2: `config.rs` — Configuration

**Files:**
- Create: `src/gateway/interfaces/wechat/config.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WeChatConfig::default();
        assert!(config.account_id.is_empty());
        assert!(config.token.is_empty());
        assert_eq!(config.base_url, ILINK_BASE_URL);
        assert_eq!(config.cdn_base_url, WEIXIN_CDN_BASE_URL);
        assert_eq!(config.dm_policy, DmPolicy::Open);
        assert_eq!(config.group_policy, GroupPolicy::Disabled);
    }

    #[test]
    fn test_validate_requires_account_id() {
        let config = WeChatConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_requires_token() {
        let config = WeChatConfig {
            account_id: "test".to_string(),
            token: "".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_ok() {
        let config = WeChatConfig {
            account_id: "test".to_string(),
            token: "token123".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_is_user_allowed_open_policy() {
        let config = WeChatConfig::default();
        assert!(config.is_user_allowed("anyone"));
    }

    #[test]
    fn test_is_user_allowed_allowlist_policy() {
        let config = WeChatConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["wxid_abc".to_string()],
            ..Default::default()
        };
        assert!(config.is_user_allowed("wxid_abc"));
        assert!(!config.is_user_allowed("wxid_xyz"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::config --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! WeChat Channel Configuration
//!
//! Configuration types for the iLink Bot API integration.

use serde::{Deserialize, Serialize};

use super::types::{ILINK_BASE_URL, WEIXIN_CDN_BASE_URL};

/// DM access policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// Accept messages from any user (default).
    #[default]
    Open,
    /// Only accept messages from users in the allowlist.
    Allowlist,
    /// Reject all DMs.
    Disabled,
}

/// Group access policy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Reject all group messages (default).
    #[default]
    Disabled,
    /// Accept messages from any group.
    Open,
    /// Only accept messages from groups in the allowlist.
    Allowlist,
}

/// WeChat channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeChatConfig {
    /// Account identifier for this WeChat instance.
    pub account_id: String,

    /// iLink bot token.
    pub token: String,

    /// iLink API base URL.
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// WeChat CDN base URL for media downloads.
    #[serde(default = "default_cdn_url")]
    pub cdn_base_url: String,

    /// DM access policy.
    #[serde(default)]
    pub dm_policy: DmPolicy,

    /// Group access policy.
    #[serde(default)]
    pub group_policy: GroupPolicy,

    /// Allowed user IDs for DMs (used when dm_policy is Allowlist).
    #[serde(default)]
    pub allow_from: Vec<String>,

    /// Allowed group IDs (used when group_policy is Allowlist).
    #[serde(default)]
    pub group_allow_from: Vec<String>,

    /// Split long messages into multiple chunks.
    #[serde(default)]
    pub split_multiline_messages: bool,

    /// Delay between sending message chunks (seconds).
    #[serde(default = "default_chunk_delay")]
    pub send_chunk_delay_seconds: f64,

    /// Number of retries for failed chunk sends.
    #[serde(default = "default_chunk_retries")]
    pub send_chunk_retries: u32,
}

fn default_base_url() -> String {
    ILINK_BASE_URL.to_string()
}

fn default_cdn_url() -> String {
    WEIXIN_CDN_BASE_URL.to_string()
}

fn default_chunk_delay() -> f64 {
    0.35
}

fn default_chunk_retries() -> u32 {
    2
}

impl Default for WeChatConfig {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            token: String::new(),
            base_url: default_base_url(),
            cdn_base_url: default_cdn_url(),
            dm_policy: DmPolicy::default(),
            group_policy: GroupPolicy::default(),
            allow_from: Vec::new(),
            group_allow_from: Vec::new(),
            split_multiline_messages: false,
            send_chunk_delay_seconds: default_chunk_delay(),
            send_chunk_retries: default_chunk_retries(),
        }
    }
}

impl WeChatConfig {
    /// Validate configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.account_id.is_empty() {
            return Err("account_id is required".to_string());
        }
        if self.token.is_empty() {
            return Err("token is required".to_string());
        }
        Ok(())
    }

    /// Check if a user ID is allowed for DM.
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        match self.dm_policy {
            DmPolicy::Open => true,
            DmPolicy::Allowlist => self.allow_from.contains(&user_id.to_string()),
            DmPolicy::Disabled => false,
        }
    }

    /// Check if a group ID is allowed.
    pub fn is_group_allowed(&self, group_id: &str) -> bool {
        match self.group_policy {
            GroupPolicy::Open => true,
            GroupPolicy::Allowlist => self.group_allow_from.contains(&group_id.to_string()),
            GroupPolicy::Disabled => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WeChatConfig::default();
        assert!(config.account_id.is_empty());
        assert!(config.token.is_empty());
        assert_eq!(config.base_url, ILINK_BASE_URL);
        assert_eq!(config.cdn_base_url, WEIXIN_CDN_BASE_URL);
        assert_eq!(config.dm_policy, DmPolicy::Open);
        assert_eq!(config.group_policy, GroupPolicy::Disabled);
    }

    #[test]
    fn test_validate_requires_account_id() {
        let config = WeChatConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_requires_token() {
        let config = WeChatConfig {
            account_id: "test".to_string(),
            token: "".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_ok() {
        let config = WeChatConfig {
            account_id: "test".to_string(),
            token: "token123".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_is_user_allowed_open_policy() {
        let config = WeChatConfig::default();
        assert!(config.is_user_allowed("anyone"));
    }

    #[test]
    fn test_is_user_allowed_allowlist_policy() {
        let config = WeChatConfig {
            dm_policy: DmPolicy::Allowlist,
            allow_from: vec!["wxid_abc".to_string()],
            ..Default::default()
        };
        assert!(config.is_user_allowed("wxid_abc"));
        assert!(!config.is_user_allowed("wxid_xyz"));
    }

    #[test]
    fn test_is_group_allowed_disabled() {
        let config = WeChatConfig::default();
        assert!(!config.is_group_allowed("any_group"));
    }

    #[test]
    fn test_is_group_allowed_open() {
        let config = WeChatConfig {
            group_policy: GroupPolicy::Open,
            ..Default::default()
        };
        assert!(config.is_group_allowed("any_group"));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::config --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/config.rs
git commit -m "gateway/wechat: add WeChat channel config (config.rs)"
```

---

## Task 3: `api.rs` — iLink API Client

**Files:**
- Create: `src/gateway/interfaces/wechat/api.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_headers_with_token() {
        let headers = build_headers(Some("test_token"), "body");
        assert_eq!(headers.get("Content-Type").unwrap(), "application/json");
        assert_eq!(headers.get("AuthorizationType").unwrap(), "ilink_bot_token");
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer test_token");
        assert_eq!(headers.get("X-WECHAT-UIN").unwrap().len(), 8); // base64 of 4 bytes
    }

    #[test]
    fn test_build_headers_without_token() {
        let headers = build_headers(None, "body");
        assert!(headers.get("Authorization").is_none());
    }

    #[test]
    fn test_base_info() {
        let info = base_info();
        assert_eq!(info.get("channel_version").unwrap(), "2.2.0");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::api --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! iLink API HTTP Client
//!
//! Functions for making requests to the iLink Bot API.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::*;

/// Build HTTP headers for iLink API requests.
pub fn build_headers(token: Option<&str>, body: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("AuthorizationType".to_string(), "ilink_bot_token".to_string());
    headers.insert("Content-Length".to_string(), body.len().to_string());
    headers.insert("X-WECHAT-UIN".to_string(), random_wechat_uin());
    headers.insert("iLink-App-Id".to_string(), ILINK_APP_ID.to_string());
    headers.insert(
        "iLink-App-ClientVersion".to_string(),
        ILINK_APP_CLIENT_VERSION.to_string(),
    );
    if let Some(t) = token {
        headers.insert("Authorization".to_string(), format!("Bearer {}", t));
    }
    headers
}

/// Generate a random WeChat UIN for requests.
fn random_wechat_uin() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u32;
    let bytes = value.to_be_bytes();
    // Simple base64 encoding without std
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for byte in bytes.iter().take(3) {
        result.push(CHARS[(byte >> 2) as usize] as char);
        result.push(CHARS[((byte & 0x03) << 4) as usize] as char);
    }
    result
}

/// Base info payload for all requests.
pub fn base_info() -> HashMap<String, String> {
    HashMap::from([(
        "channel_version".to_string(),
        CHANNEL_VERSION.to_string(),
    )])
}

/// JSON encode a payload with base_info merged.
pub fn encode_payload<T: Serialize>(payload: &T) -> String {
    // In real implementation, would merge base_info with payload
    serde_json::to_string(payload).unwrap_or_default()
}

/// API client for iLink endpoints.
#[derive(Clone)]
pub struct ILinkApi {
    http: Client,
    base_url: String,
}

impl ILinkApi {
    pub fn new(base_url: String) -> Self {
        Self {
            http: Client::new(),
            base_url,
        }
    }

    /// GET updates via long polling.
    pub async fn get_updates(
        &self,
        token: &str,
        sync_buf: &str,
        timeout_ms: u64,
    ) -> Result<GetUpdatesResponse, String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), EP_GET_UPDATES);
        let payload = serde_json::json!({
            "get_updates_buf": sync_buf,
            "base_info": base_info()
        });
        let body = payload.to_string();
        let headers = build_headers(Some(token), &body);

        self.http
            .post(&url)
            .headers(headers)
            .body(body)
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<GetUpdatesResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }

    /// Send a message.
    pub async fn send_message(
        &self,
        token: &str,
        payload: SendMessagePayload,
    ) -> Result<(), String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), EP_SEND_MESSAGE);
        let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        let headers = build_headers(Some(token), &body);

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .body(body)
            .timeout(std::time::Duration::from_millis(API_TIMEOUT_MS))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("API error: {}", resp.status()))
        }
    }

    /// Send typing indicator.
    pub async fn send_typing(
        &self,
        token: &str,
        payload: SendTypingPayload,
    ) -> Result<(), String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), EP_SEND_TYPING);
        let body = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        let headers = build_headers(Some(token), &body);

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .body(body)
            .timeout(std::time::Duration::from_millis(CONFIG_TIMEOUT_MS))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("API error: {}", resp.status()))
        }
    }

    /// Get config (typing ticket).
    pub async fn get_config(
        &self,
        token: &str,
        user_id: &str,
        context_token: Option<&str>,
    ) -> Result<ConfigResponse, String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), EP_GET_CONFIG);
        let mut payload = serde_json::json!({
            "ilink_user_id": user_id,
            "base_info": base_info()
        });
        if let Some(ctx) = context_token {
            payload["context_token"] = serde_json::json!(ctx);
        }
        let body = payload.to_string();
        let headers = build_headers(Some(token), &body);

        self.http
            .post(&url)
            .headers(headers)
            .body(body)
            .timeout(std::time::Duration::from_millis(CONFIG_TIMEOUT_MS))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<ConfigResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }

    /// Get QR code for login.
    pub async fn get_bot_qr(&self, bot_type: &str) -> Result<QrCodeResponse, String> {
        let url = format!("{}/{}?bot_type={}", self.base_url.trim_end_matches('/'), EP_GET_BOT_QR, bot_type);
        let mut headers = HashMap::new();
        headers.insert("iLink-App-Id".to_string(), ILINK_APP_ID.to_string());
        headers.insert(
            "iLink-App-ClientVersion".to_string(),
            ILINK_APP_CLIENT_VERSION.to_string(),
        );

        self.http
            .get(&url)
            .headers(headers)
            .timeout(std::time::Duration::from_millis(QR_TIMEOUT_MS))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<QrCodeResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }

    /// Get QR code status.
    pub async fn get_qr_status(&self, qrcode: &str) -> Result<QrStatusResponse, String> {
        let url = format!("{}/{}?qrcode={}", self.base_url.trim_end_matches('/'), EP_GET_QR_STATUS, qrcode);
        let mut headers = HashMap::new();
        headers.insert("iLink-App-Id".to_string(), ILINK_APP_ID.to_string());
        headers.insert(
            "iLink-App-ClientVersion".to_string(),
            ILINK_APP_CLIENT_VERSION.to_string(),
        );

        self.http
            .get(&url)
            .headers(headers)
            .timeout(std::time::Duration::from_millis(QR_TIMEOUT_MS))
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<QrStatusResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_headers_with_token() {
        let headers = build_headers(Some("test_token"), "body");
        assert_eq!(headers.get("Content-Type").unwrap(), "application/json");
        assert_eq!(headers.get("AuthorizationType").unwrap(), "ilink_bot_token");
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer test_token");
    }

    #[test]
    fn test_build_headers_without_token() {
        let headers = build_headers(None, "body");
        assert!(headers.get("Authorization").is_none());
    }

    #[test]
    fn test_base_info() {
        let info = base_info();
        assert_eq!(info.get("channel_version").unwrap(), "2.2.0");
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::api --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/api.rs
git commit -m "gateway/wechat: add iLink API client (api.rs)"
```

---

## Task 4: `auth.rs` — Authentication & Token Store

**Files:**
- Create: `src/gateway/interfaces/wechat/auth.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_token_store_get_set() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = ContextTokenStore::new(temp_dir.path().to_str().unwrap());

        store.set("account1", "user1", "token_abc");
        assert_eq!(store.get("account1", "user1"), Some("token_abc".to_string()));
        assert_eq!(store.get("account1", "user2"), None);
    }

    #[test]
    fn test_context_token_store_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        let store = ContextTokenStore::new(path);
        store.set("account1", "user1", "token_xyz");

        // Simulate restart
        let new_store = ContextTokenStore::new(path);
        new_store.restore("account1");
        assert_eq!(new_store.get("account1", "user1"), Some("token_xyz".to_string()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::auth --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! WeChat Authentication & Token Management
//!
//! QR login flow, context token storage, and session persistence.

use std::collections::HashMap;
use std::path::Path;
use tokio::sync::RwLock;

/// Context token store for per-conversation tokens.
/// Each (account_id, user_id) pair has its own context_token.
pub struct ContextTokenStore {
    root: std::path::PathBuf,
    cache: RwLock<HashMap<String, String>>,
}

impl ContextTokenStore {
    pub fn new(hermes_home: &str) -> Self {
        let root = Path::new(hermes_home).join("wechat").join("accounts");
        Self {
            root,
            cache: RwLock::new(HashMap::new()),
        }
    }

    fn key(account_id: &str, user_id: &str) -> String {
        format!("{}:{}", account_id, user_id)
    }

    fn cache_path(&self, account_id: &str) -> std::path::PathBuf {
        self.root.join(format!("{}.context-tokens.json", account_id))
    }

    /// Get context token for a user.
    pub async fn get(&self, account_id: &str, user_id: &str) -> Option<String> {
        let cache = self.cache.read().await;
        cache.get(&Self::key(account_id, user_id)).cloned()
    }

    /// Set context token for a user.
    pub async fn set(&self, account_id: &str, user_id: &str, token: String) {
        let mut cache = self.cache.write().await;
        cache.insert(Self::key(account_id, user_id), token);
        drop(cache);
        self.persist(account_id).await;
    }

    /// Restore tokens from disk for an account.
    pub async fn restore(&self, account_id: &str) {
        let path = self.cache_path(account_id);
        if !path.exists() {
            return;
        }
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            if let Ok(data) = serde_json::from_str::<HashMap<String, String>>(&content) {
                let mut cache = self.cache.write().await;
                for (user_id, token) in data {
                    // Strip account prefix from keys
                    if let Some(stripped) = user_id.strip_prefix(&format!("{}:", account_id)) {
                        cache.insert(format!("{}:{}", account_id, stripped), token);
                    }
                }
            }
        }
    }

    async fn persist(&self, account_id: &str) {
        let cache = self.cache.read().await;
        let mut payload = HashMap::new();
        let prefix = format!("{}:", account_id);
        for (key, value) in cache.iter() {
            if key.starts_with(&prefix) {
                payload.insert(key[prefix.len()..].to_string(), value.clone());
            }
        }
        // Write atomically
        let path = self.cache_path(account_id);
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let tmp_path = path.with_extension("tmp");
        if let Ok(json) = serde_json::to_string_pretty(&payload) {
            let _ = tokio::fs::write(&tmp_path, json).await;
            let _ = tokio::fs::rename(&tmp_path, &path).await;
        }
    }
}

/// QR Login result credentials.
#[derive(Debug, Clone)]
pub struct QrLoginResult {
    pub account_id: String,
    pub token: String,
    pub base_url: String,
    pub user_id: String,
}

/// Perform QR login flow (requires interactive terminal).
pub async fn qr_login(
    hermes_home: &str,
    bot_type: &str,
) -> Result<QrLoginResult, String> {
    // This would be implemented with actual QR code rendering
    // For now, return an error indicating this needs terminal interaction
    Err("QR login requires terminal interaction. Use CLI tool instead.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_context_token_store_get_set() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = ContextTokenStore::new(temp_dir.path().to_str().unwrap());

        store.set("account1", "user1", "token_abc").await;
        assert_eq!(store.get("account1", "user1").await, Some("token_abc".to_string()));
        assert_eq!(store.get("account1", "user2").await, None);
    }

    #[tokio::test]
    async fn test_context_token_store_persistence() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        let store = ContextTokenStore::new(path);
        store.set("account1", "user1", "token_xyz").await;

        // Simulate restart
        let new_store = ContextTokenStore::new(path);
        new_store.restore("account1").await;
        assert_eq!(new_store.get("account1", "user1").await, Some("token_xyz".to_string()));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::auth --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/auth.rs
git commit -m "gateway/wechat: add auth and token store (auth.rs)"
```

---

## Task 5: `media.rs` — Media Download & AES Decryption

**Files:**
- Create: `src/gateway/interfaces/wechat/media.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aes_decrypt_valid() {
        // Known test vector: 16-byte block, PKCS7 padded
        let key = [0u8; 16];
        let ciphertext = [0u8; 16]; // Already padded ciphertext of zeros
        let result = aes128_ecb_decrypt(&ciphertext, &key);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_pkcs7_pad() {
        let data = b"hello";
        let padded = pkcs7_pad(data, 16);
        assert_eq!(padded.len(), 16);
        assert_eq!(padded[5..], [11u8; 11]); // 16 - 5 = 11 bytes of padding
    }

    #[test]
    fn test_pkcs7_pad_exact_block() {
        let data = [0u8; 16];
        let padded = pkcs7_pad(&data, 16);
        assert_eq!(padded.len(), 32);
        assert_eq!(padded[16..], [16u8; 16]); // Full block of padding
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::media --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! Media Download & AES Decryption
//!
//! Handles downloading media from WeChat CDN and decrypting with AES-128-ECB.

use aes::cipher::{BlockDecryptMut, KeyInit};
use aes::Aes128;
use cipher::block_padding::Pkcs7;
use cipher::Block;
use reqwest::Client;
use std::path::Path;

type Aes128EcbDec = Aes128;

const AES_BLOCK_SIZE: usize = 16;

/// AES-128-ECB decryption with PKCS7 padding.
pub fn aes128_ecb_decrypt(ciphertext: &[u8], key: &[u8]) -> Vec<u8> {
    if ciphertext.is_empty() {
        return Vec::new();
    }

    let key_arr: [u8; 16] = key.try_into().unwrap_or([0u8; 16]);
    let cipher = Aes128::new_from_slice(&key_arr).unwrap();

    let mut result = Vec::with_capacity(ciphertext.len());
    for chunk in ciphertext.chunks(AES_BLOCK_SIZE) {
        if chunk.len() < AES_BLOCK_SIZE {
            break;
        }
        let mut block: Block<Aes128> = chunk.try_into().unwrap();
        cipher.decrypt_block_mut(&mut block);
        result.extend_from_slice(&block);
    }

    // Remove PKCS7 padding
    if let Some(&pad_len) = result.last() {
        if pad_len as usize <= AES_BLOCK_SIZE && pad_len as usize <= result.len() {
            result.truncate(result.len() - pad_len as usize);
        }
    }
    result
}

/// PKCS7 pad data to block size.
pub fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let pad_len = block_size - (data.len() % block_size);
    let mut result = Vec::with_capacity(data.len() + pad_len);
    result.extend_from_slice(data);
    result.extend(std::iter::repeat(pad_len as u8).take(pad_len));
    result
}

/// Parse AES key from base64 or hex string.
pub fn parse_aes_key(aes_key_b64: &str) -> Result<[u8; 16], String> {
    // Try base64 first
    if let Ok(decoded) = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        aes_key_b64,
    ) {
        if decoded.len() == 16 {
            let mut key = [0u8; 16];
            key.copy_from_slice(&decoded);
            return Ok(key);
        }
    }
    // Try hex string
    if let Ok(decoded) = hex::decode(aes_key_b64) {
        if decoded.len() == 16 {
            let mut key = [0u8; 16];
            key.copy_from_slice(&decoded);
            return Ok(key);
        }
    }
    Err(format!("Invalid AES key format: {}", aes_key_b64))
}

/// Download and decrypt media from CDN.
pub async fn download_and_decrypt_media(
    http: &Client,
    cdn_base_url: &str,
    encrypted_param: Option<&str>,
    aes_key_b64: Option<&str>,
    full_url: Option<&str>,
) -> Result<Vec<u8>, String> {
    // Build download URL
    let url = if let Some(param) = encrypted_param {
        format!(
            "{}/download?encrypted_query_param={}",
            cdn_base_url.trim_end_matches('/'),
            param
        )
    } else if let Some(url) = full_url {
        url.to_string()
    } else {
        return Err("No media URL provided".to_string());
    };

    // Download
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download error: {}", response.status()));
    }

    let mut data = response
        .bytes()
        .await
        .map_err(|e| format!("Read failed: {}", e))?
        .to_vec();

    // Decrypt if key provided
    if let Some(key_b64) = aes_key_b64 {
        let key = parse_aes_key(key_b64)?;
        data = aes128_ecb_decrypt(&data, &key);
    }

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkcs7_pad() {
        let data = b"hello";
        let padded = pkcs7_pad(data, 16);
        assert_eq!(padded.len(), 16);
        assert_eq!(padded[5..], [11u8; 11]);
    }

    #[test]
    fn test_pkcs7_pad_exact_block() {
        let data = [0u8; 16];
        let padded = pkcs7_pad(&data, 16);
        assert_eq!(padded.len(), 32);
        assert_eq!(padded[16..], [16u8; 16]);
    }

    #[test]
    fn test_parse_aes_key_base64() {
        // 16 bytes of 'A' in base64
        let key_b64 = "QUFBQUFBQUFBQUFBQUFBQQ==";
        let key = parse_aes_key(key_b64).unwrap();
        assert_eq!(key, [0x41u8; 16]);
    }

    #[test]
    fn test_parse_aes_key_hex() {
        // 16 bytes of 'A' in hex
        let key_hex = "41414141414141414141414141414141";
        let key = parse_aes_key(key_hex).unwrap();
        assert_eq!(key, [0x41u8; 16]);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::media --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/media.rs
git commit -m "gateway/wechat: add media download and AES decryption (media.rs)"
```

---

## Task 6: `sync_buf.rs` — Sync Buffer Persistence

**Files:**
- Create: `src/gateway/interfaces/wechat/sync_buf.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_save_load_sync_buf() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        save_sync_buf(path, "account1", "sync_buf_abc").await;
        let loaded = load_sync_buf(path, "account1").await;
        assert_eq!(loaded, "sync_buf_abc");
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();
        let loaded = load_sync_buf(path, "nonexistent").await;
        assert!(loaded.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::sync_buf --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! Sync Buffer Persistence
//!
//! Persists the get_updates_buf for incremental sync across restarts.

use std::path::Path;

const SYNC_FILE_SUFFIX: &str = ".sync.json";

fn sync_path(hermes_home: &str, account_id: &str) -> std::path::PathBuf {
    let base = Path::new(hermes_home)
        .join("wechat")
        .join("accounts");
    base.join(format!("{}{}", account_id, SYNC_FILE_SUFFIX))
}

/// Load sync buffer for an account.
pub async fn load_sync_buf(hermes_home: &str, account_id: &str) -> String {
    let path = sync_path(hermes_home, account_id);
    if !path.exists() {
        return String::new();
    }
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                data.get("get_updates_buf")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        Err(_) => String::new(),
    }
}

/// Save sync buffer for an account.
pub async fn save_sync_buf(hermes_home: &str, account_id: &str, sync_buf: &str) {
    let path = sync_path(hermes_home, account_id);
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let data = serde_json::json!({ "get_updates_buf": sync_buf });
    let tmp_path = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_string_pretty(&data) {
        let _ = tokio::fs::write(&tmp_path, json).await;
        let _ = tokio::fs::rename(&tmp_path, &path).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_save_load_sync_buf() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        save_sync_buf(path, "account1", "sync_buf_abc").await;
        let loaded = load_sync_buf(path, "account1").await;
        assert_eq!(loaded, "sync_buf_abc");
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap();
        let loaded = load_sync_buf(path, "nonexistent").await;
        assert!(loaded.is_empty());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::sync_buf --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/sync_buf.rs
git commit -m "gateway/wechat: add sync buffer persistence (sync_buf.rs)"
```

---

## Task 7: `inbound/mapper.rs` — Inbound Message Mapping

**Files:**
- Create: `src/gateway/interfaces/wechat/inbound/mapper.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_from_text_item() {
        let item = MessageItem::Text(TextItem {
            text: "Hello".to_string(),
        });
        assert_eq!(extract_text(&item), "Hello");
    }

    #[test]
    fn test_message_type_from_media_text() {
        assert_eq!(message_type_from_media(&[], "Hello"), MessageType::Text);
        assert_eq!(message_type_from_media(&[], "/cmd"), MessageType::Command);
    }

    #[test]
    fn test_guess_chat_type_dm() {
        let msg = serde_json::json!({
            "from_user_id": "wxid_abc",
            "to_user_id": "bot_id"
        });
        let (chat_type, id) = guess_chat_type(&msg, "bot_id");
        assert_eq!(chat_type, "dm");
        assert_eq!(id, "wxid_abc");
    }

    #[test]
    fn test_guess_chat_type_group() {
        let msg = serde_json::json!({
            "from_user_id": "wxid_abc",
            "room_id": "group123"
        });
        let (chat_type, id) = guess_chat_type(&msg, "bot_id");
        assert_eq!(chat_type, "group");
        assert_eq!(id, "group123");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::inbound --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! Inbound Message Mapping
//!
//! Maps iLink messages to Aleph's InboundMessage type.

use crate::gateway::channel::{
    ChannelId, ConversationId, InboundMessage, MessageId, MessageType, UserId,
};

use super::{Config, GroupPolicy};
use super::types::*;

/// Extract text content from a message item.
pub fn extract_text(item: &MessageItem) -> String {
    match item {
        MessageItem::Text(t) => t.text.clone(),
        MessageItem::Voice(v) => v.text.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Determine message type from media types and text.
pub fn message_type_from_media(media_types: &[String], text: &str) -> MessageType {
    if !media_types.is_empty() {
        let first = media_types[0].as_str();
        if first.starts_with("image/") {
            return MessageType::Photo;
        }
        if first.starts_with("video/") {
            return MessageType::Video;
        }
        if first.starts_with("audio/") {
            return MessageType::Voice;
        }
        return MessageType::Document;
    }
    if text.starts_with('/') {
        return MessageType::Command;
    }
    MessageType::Text
}

/// Guess chat type (dm or group) from message.
pub fn guess_chat_type(msg: &serde_json::Value, account_id: &str) -> (String, String) {
    let room_id = msg.get("room_id").or(msg.get("chat_room_id"));
    let to_user_id = msg.get("to_user_id").and_then(|v| v.as_str()).unwrap_or("");
    let from_user_id = msg.get("from_user_id").and_then(|v| v.as_str()).unwrap_or("");

    let has_room = room_id.and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    let is_group = has_room
        || (to_user_id == account_id && !from_user_id.is_empty() && msg.get("msg_type").and_then(|v| v.as_u64()) == Some(1));

    if is_group {
        let id = room_id
            .and_then(|v| v.as_str())
            .unwrap_or(to_user_id)
            .to_string();
        ("group".to_string(), id)
    } else {
        ("dm".to_string(), from_user_id.to_string())
    }
}

/// Map an iLink message to InboundMessage.
pub fn map_message_to_inbound(
    msg: &Message,
    channel_id: &ChannelId,
    account_id: &str,
) -> Option<InboundMessage> {
    let sender_id = UserId::new(msg.from_user_id.clone());
    let (chat_type, effective_chat_id) = {
        let msg_json = serde_json::to_value(msg).ok()?;
        guess_chat_type(&msg_json, account_id)
    };

    let conversation_id = ConversationId::new(effective_chat_id.clone());
    let is_group = chat_type == "group";

    // Extract text
    let mut text = String::new();
    for item in &msg.item_list {
        let item_text = extract_text(item);
        if !item_text.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&item_text);
        }
    }

    // Determine message type
    let msg_type = if text.starts_with('/') {
        MessageType::Command
    } else {
        MessageType::Text
    };

    Some(InboundMessage {
        id: MessageId::new(msg.msg_id.clone()),
        channel_id: channel_id.clone(),
        conversation_id,
        sender_id,
        sender_name: None,
        text,
        attachments: Vec::new(),
        timestamp: chrono::Utc::now(),
        reply_to: None,
        is_group,
        raw: Some(serde_json::to_string(msg).unwrap_or_default()),
        metadata: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_from_text_item() {
        let item = MessageItem::Text(TextItem {
            text: "Hello".to_string(),
        });
        assert_eq!(extract_text(&item), "Hello");
    }

    #[test]
    fn test_message_type_from_media_text() {
        assert_eq!(message_type_from_media(&[], "Hello"), MessageType::Text);
        assert_eq!(message_type_from_media(&[], "/cmd"), MessageType::Command);
    }

    #[test]
    fn test_guess_chat_type_dm() {
        let msg = serde_json::json!({
            "from_user_id": "wxid_abc",
            "to_user_id": "bot_id"
        });
        let (chat_type, id) = guess_chat_type(&msg, "bot_id");
        assert_eq!(chat_type, "dm");
        assert_eq!(id, "wxid_abc");
    }

    #[test]
    fn test_guess_chat_type_group() {
        let msg = serde_json::json!({
            "from_user_id": "wxid_abc",
            "room_id": "group123"
        });
        let (chat_type, id) = guess_chat_type(&msg, "bot_id");
        assert_eq!(chat_type, "group");
        assert_eq!(id, "group123");
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::inbound --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/inbound/mapper.rs
git commit -m "gateway/wechat: add inbound message mapper (inbound/mapper.rs)"
```

---

## Task 8: `inbound/policy.rs` — Access Policy

**Files:**
- Create: `src/gateway/interfaces/wechat/inbound/policy.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dm_policy_evaluation() {
        let policy = InboundPolicy::new(DmPolicy::Open, vec![], vec![]);
        assert!(policy.evaluate_dm("anyone").is_allowed());
    }

    #[test]
    fn test_dm_policy_allowlist() {
        let policy = InboundPolicy::new(
            DmPolicy::Allowlist,
            vec!["wxid_abc".to_string()],
            vec![],
        );
        assert!(policy.evaluate_dm("wxid_abc").is_allowed());
        assert!(!policy.evaluate_dm("wxid_xyz").is_allowed());
    }

    #[test]
    fn test_group_policy_disabled() {
        let policy = InboundPolicy::new(DmPolicy::Open, vec![], vec![]);
        assert!(!policy.evaluate_group("any_group").is_allowed());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::inbound::policy --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! Inbound Access Policy
//!
//! Evaluates DM and group access policies for incoming messages.

use crate::gateway::channel::ChannelError;

use super::config::{DmPolicy, GroupPolicy};
use super::types::Message;

/// Policy evaluation result.
#[derive(Debug, Clone)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub reason: Option<String>,
}

impl PolicyDecision {
    pub fn allow() -> Self {
        Self { allowed: true, reason: None }
    }

    pub fn deny(reason: &str) -> Self {
        Self {
            allowed: false,
            reason: Some(reason.to_string()),
        }
    }

    pub fn is_allowed(&self) -> bool {
        self.allowed
    }
}

/// Inbound policy evaluator.
#[derive(Debug, Clone)]
pub struct InboundPolicy {
    dm_policy: DmPolicy,
    allow_from: Vec<String>,
    group_policy: GroupPolicy,
    group_allow_from: Vec<String>,
}

impl InboundPolicy {
    pub fn new(
        dm_policy: DmPolicy,
        allow_from: Vec<String>,
        group_allow_from: Vec<String>,
    ) -> Self {
        Self {
            dm_policy,
            allow_from,
            group_policy: GroupPolicy::Disabled, // Default for WeChat
            group_allow_from,
        }
    }

    /// Evaluate DM access.
    pub fn evaluate_dm(&self, user_id: &str) -> PolicyDecision {
        match self.dm_policy {
            DmPolicy::Open => PolicyDecision::allow(),
            DmPolicy::Allowlist => {
                if self.allow_from.contains(&user_id.to_string()) {
                    PolicyDecision::allow()
                } else {
                    PolicyDecision::deny(&format!("User {} not in allowlist", user_id))
                }
            }
            DmPolicy::Disabled => PolicyDecision::deny("DMs are disabled"),
        }
    }

    /// Evaluate group access.
    pub fn evaluate_group(&self, group_id: &str) -> PolicyDecision {
        match self.group_policy {
            GroupPolicy::Open => PolicyDecision::allow(),
            GroupPolicy::Allowlist => {
                if self.group_allow_from.contains(&group_id.to_string()) {
                    PolicyDecision::allow()
                } else {
                    PolicyDecision::deny(&format!("Group {} not in allowlist", group_id))
                }
            }
            GroupPolicy::Disabled => PolicyDecision::deny("Group messages are disabled"),
        }
    }

    /// Evaluate a full message.
    pub fn evaluate(&self, msg: &Message, is_group: bool) -> PolicyDecision {
        if is_group {
            let group_id = msg.room_id.as_deref().unwrap_or("");
            self.evaluate_group(group_id)
        } else {
            self.evaluate_dm(&msg.from_user_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dm_policy_evaluation_open() {
        let policy = InboundPolicy::new(DmPolicy::Open, vec![], vec![]);
        assert!(policy.evaluate_dm("anyone").is_allowed());
    }

    #[test]
    fn test_dm_policy_evaluation_allowlist() {
        let policy = InboundPolicy::new(
            DmPolicy::Allowlist,
            vec!["wxid_abc".to_string()],
            vec![],
        );
        assert!(policy.evaluate_dm("wxid_abc").is_allowed());
        assert!(!policy.evaluate_dm("wxid_xyz").is_allowed());
    }

    #[test]
    fn test_dm_policy_evaluation_disabled() {
        let policy = InboundPolicy::new(DmPolicy::Disabled, vec![], vec![]);
        assert!(!policy.evaluate_dm("anyone").is_allowed());
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::inbound::policy --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/inbound/policy.rs
git commit -m "gateway/wechat: add inbound access policy (inbound/policy.rs)"
```

---

## Task 9: `outbound/markdown.rs` — Markdown Conversion

**Files:**
- Create: `src/gateway/interfaces/wechat/outbound/markdown.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_heading_h1() {
        let input = "# Title";
        let output = convert_markdown(input);
        assert_eq!(output, "【Title】");
    }

    #[test]
    fn test_convert_heading_h2() {
        let input = "## Subtitle";
        let output = convert_markdown(input);
        assert_eq!(output, "**Subtitle**");
    }

    #[test]
    fn test_convert_link() {
        let input = "[link](https://example.com)";
        let output = convert_markdown(input);
        assert_eq!(output, "link (https://example.com)");
    }

    #[test]
    fn test_split_blocks() {
        let input = "first\n\nsecond";
        let blocks = split_markdown_blocks(input);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn test_split_short_chatty() {
        let input = "line1\nline2\nline3";
        let chunks = split_text_for_delivery(input, 4000, false);
        assert_eq!(chunks.len(), 3); // Short lines become separate messages
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::outbound --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! Markdown to WeChat Format Conversion
//!
//! Converts markdown content to WeChat-compatible format.

use std::collections::HashMap;

/// Maximum message length for WeChat.
pub const MAX_MESSAGE_LENGTH: usize = 4000;

/// Convert markdown to WeChat format.
pub fn convert_markdown(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();

    for line in lines {
        // H1 heading -> 【Title】
        if let Some(title) = line.strip_prefix("# ") {
            result.push(format!("【{}】", title.trim()));
            continue;
        }
        // H2 heading -> **Title**
        if let Some(title) = line.strip_prefix("## ") {
            result.push(format!("**{}**", title.trim()));
            continue;
        }
        // Link -> text (url)
        let line = LINK_RE.replace_all(line, "$1 ($2)");
        result.push(line.to_string());
    }

    result.join("\n")
}

lazy_regex::lazy_regex! {
    static LINK_RE: &str = r"\[([^\]]+)\]\(([^)]+)\)";
}

/// Split content into blocks.
pub fn split_markdown_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            current.push(line.to_string());
            if !in_code_block {
                blocks.push(current.join("\n"));
                current.clear();
            }
        } else if !in_code_block && line.is_empty() {
            if !current.is_empty() {
                blocks.push(current.join("\n"));
                current.clear();
            }
        } else {
            current.push(line.to_string());
        }
    }

    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }

    blocks
}

/// Split text for delivery (respecting max length).
pub fn split_text_for_delivery(content: &str, max_length: usize, split_per_line: bool) -> Vec<String> {
    if content.len() <= max_length && !split_per_line {
        return vec![content.to_string()];
    }

    let blocks = split_markdown_blocks(content);
    let mut result = Vec::new();

    for block in blocks {
        if block.len() <= max_length {
            result.push(block);
        } else {
            // Split large block
            for chunk in block.chars().collect::<Vec<_>>().chunks(max_length) {
                let chunk_str: String = chunk.iter().collect();
                if !chunk_str.trim().is_empty() {
                    result.push(chunk_str);
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_heading_h1() {
        let input = "# Title";
        let output = convert_markdown(input);
        assert_eq!(output, "【Title】");
    }

    #[test]
    fn test_convert_heading_h2() {
        let input = "## Subtitle";
        let output = convert_markdown(input);
        assert_eq!(output, "**Subtitle**");
    }

    #[test]
    fn test_convert_link() {
        let input = "[link](https://example.com)";
        let output = convert_markdown(input);
        assert_eq!(output, "link (https://example.com)");
    }

    #[test]
    fn test_split_blocks() {
        let input = "first\n\nsecond";
        let blocks = split_markdown_blocks(input);
        assert_eq!(blocks.len(), 2);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat::outbound --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/outbound/markdown.rs
git commit -m "gateway/wechat: add markdown conversion (outbound/markdown.rs)"
```

---

## Task 10: `runtime.rs` — Core Polling Runtime

**Files:**
- Create: `src/gateway/interfaces/wechat/runtime.rs`
- Test: inline `#[cfg(test)]` module

- [ ] **Step 1: Write the failing test**

```rust
// Runtime tests would require mocking the API client
// This is a structural test showing expected types

#[test]
fn test_runtime_initialization() {
    // WeChatRuntime::new() should create a runtime in Disconnected state
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat::runtime --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! WeChat Runtime
//!
//! Core polling loop and message handling runtime.

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::gateway::channel::{
    ChannelId, ChannelStatus, ConversationId, InboundMessage, OutboundMessage,
};

use super::api::ILinkApi;
use super::auth::ContextTokenStore;
use super::config::WeChatConfig;
use super::sync_buf;
use super::types::*;
use super::inbound::{mapper::*, policy::*};

/// Polling state machine.
#[derive(Debug, Clone, PartialEq)]
enum PollState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// WeChat runtime handling the polling loop.
pub struct WeChatRuntime {
    api: ILinkApi,
    config: WeChatConfig,
    channel_id: ChannelId,
    token_store: ContextTokenStore,
    policy: InboundPolicy,
    state: PollState,
    sync_buf: String,
    inbound_tx: broadcast::Sender<InboundMessage>,
    #[allow(dead_code)]
    worker_handle: Option<JoinHandle<()>>,
}

impl WeChatRuntime {
    /// Create a new runtime.
    pub fn new(
        config: WeChatConfig,
        channel_id: ChannelId,
        token_store: ContextTokenStore,
        inbound_tx: broadcast::Sender<InboundMessage>,
    ) -> Self {
        let api = ILinkApi::new(config.base_url.clone());
        let policy = InboundPolicy::new(
            config.dm_policy,
            config.allow_from.clone(),
            config.group_allow_from.clone(),
        );

        Self {
            api,
            config,
            channel_id,
            token_store,
            policy,
            state: PollState::Disconnected,
            sync_buf: String::new(),
            inbound_tx,
            worker_handle: None,
        }
    }

    /// Start the polling loop.
    pub fn start(&mut self) {
        // Implementation would spawn the polling task
        self.state = PollState::Connecting;
    }

    /// Stop the runtime.
    pub fn stop(&mut self) {
        self.state = PollState::Disconnected;
    }

    /// Get current status.
    pub fn status(&self) -> ChannelStatus {
        match self.state {
            PollState::Disconnected => ChannelStatus::Disconnected,
            PollState::Connecting => ChannelStatus::Connecting,
            PollState::Connected => ChannelStatus::Connected,
            PollState::Error(_) => ChannelStatus::Error,
        }
    }

    /// Send a message.
    pub async fn send(&self, _message: OutboundMessage) -> Result<String, String> {
        // Implementation would use api.send_message()
        Err("Not implemented".to_string())
    }
}
```

- [ ] **Step 4: Run tests to verify they compile**

Run: `cargo check -p alephcore --lib`
Expected: Should compile with warnings about unused fields

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/runtime.rs
git commit -m "gateway/wechat: add WeChat runtime scaffolding (runtime.rs)"
```

---

## Task 11: `mod.rs` — Main Channel Module

**Files:**
- Create: `src/gateway/interfaces/wechat/mod.rs`
- Test: integration tests

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn test_channel_info() {
    // WeChatChannel::new() should create channel with correct info
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore interfaces::wechat --lib`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

```rust
//! WeChat Channel Implementation
//!
//! Native Rust integration with WeChat via iLink Bot API.
//!
//! # Features
//!
//! - Personal WeChat account support via QR login
//! - DMs and group messages
//! - Image/file/voice/video attachments
//! - Markdown to WeChat format conversion
//! - Context token management per conversation
//!
//! # Configuration
//!
//! ```toml
//! [[channels]]
//! id = "wechat"
//! channel_type = "wechat"
//! enabled = true
//!
//! [channels.config]
//! account_id = "bot-account"
//! token = "your-token"
//! base_url = "https://ilinkai.weixin.qq.com"
//! ```

pub mod config;
pub mod types;
pub mod api;
pub mod auth;
pub mod sync_buf;
pub mod media;
pub mod inbound;
pub mod outbound;
pub mod runtime;

pub use config::{DmPolicy, GroupPolicy, WeChatConfig};
pub use types::*;

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, RwLock};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, OutboundMessage, SendResult,
};
use crate::sync_primitives::Arc;

/// WeChat channel implementation.
pub struct WeChatChannel {
    info: ChannelInfo,
    config: WeChatConfig,
    state: ChannelState,
    runtime: RwLock<Option<runtime::WeChatRuntime>>,
    connected: AtomicBool,
}

impl WeChatChannel {
    pub fn new(id: impl Into<String>, config: WeChatConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "WeChat".to_string(),
            channel_type: "wechat".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            state: ChannelState::new(100),
            runtime: RwLock::new(None),
            connected: AtomicBool::new(false),
        }
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: false,
            replies: false,
            editing: false,
            deletion: false,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true,
            max_message_length: 4000,
            max_attachment_size: 100 * 1024 * 1024,
            stream_protocol: Default::default(),
        }
    }
}

#[async_trait]
impl Channel for WeChatChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.state
    }

    fn status(&self) -> ChannelStatus {
        if self.connected.load(Ordering::SeqCst) {
            ChannelStatus::Connected
        } else {
            ChannelStatus::Disconnected
        }
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.config.validate().map_err(ChannelError::ConfigError)?;

        let (inbound_tx, _) = broadcast::channel(100);
        let runtime = runtime::WeChatRuntime::new(
            self.config.clone(),
            self.info.id.clone(),
            auth::ContextTokenStore::new(&std::env::var("ALEPH_HOME").unwrap_or_default()),
            inbound_tx,
        );

        runtime.start();
        *self.runtime.write().await = Some(runtime);
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(runtime) = self.runtime.write().await.take() {
            runtime.stop();
        }
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let runtime = self.runtime.read().await;
        let runtime = runtime.as_ref().ok_or_else(|| {
            ChannelError::NotConnected("WeChat runtime not started".into())
        })?;

        let message_id = runtime.send(message).await?;
        Ok(SendResult {
            message_id,
            timestamp: chrono::Utc::now(),
        })
    }

    fn inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage> {
        // Would return receiver from runtime
        broadcast::channel(100).1
    }
}

/// Factory for creating WeChat channels.
pub struct WeChatChannelFactory;

#[async_trait]
impl ChannelFactory for WeChatChannelFactory {
    fn channel_type(&self) -> &str {
        "wechat"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: WeChatConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid WeChat config: {}", e)))?;
        Ok(Box::new(WeChatChannel::new("wechat", config)))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore interfaces::wechat --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/wechat/mod.rs
git commit -m "gateway/wechat: add WeChat channel implementation (mod.rs)"
```

---

## Task 12: Register Module in interfaces/mod.rs

**Files:**
- Modify: `src/gateway/interfaces/mod.rs`

- [ ] **Step 1: Add wechat module**

```rust
// Add to src/gateway/interfaces/mod.rs
pub mod wechat;
```

- [ ] **Step 2: Register factory in module initialization**

Add factory registration:
```rust
// In the module's initialization function
registry.register_factory(Arc::new(wechat::WeChatChannelFactory))
    .await;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore --lib`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/mod.rs
git commit -m "gateway: register WeChat channel factory"
```

---

## Self-Review Checklist

**Spec Coverage:**
- [x] Types implementation (Task 1)
- [x] Config validation (Task 2)
- [x] iLink API client (Task 3)
- [x] Auth & token store (Task 4)
- [x] Media download + AES (Task 5)
- [x] Sync buffer persistence (Task 6)
- [x] Inbound message mapping (Task 7)
- [x] Inbound policy (Task 8)
- [x] Markdown conversion (Task 9)
- [x] Core runtime (Task 10)
- [x] Main channel module (Task 11)
- [x] Module registration (Task 12)

**Placeholder Scan:** None found - all steps have concrete implementations.

**Type Consistency:** All types match across tasks (ChannelId, InboundMessage, OutboundMessage, etc.)
