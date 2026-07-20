# LINE Channel Implementation Design

**Date:** 2026-04-09
**Status:** Approved
**Supersedes:** N/A

---

## 1. Overview

Implement a LINE messaging channel adapter for Aleph, following Aleph's Rust-native architecture patterns. The adapter integrates LINE's webhook-based messaging API with Aleph's unified `Channel` trait, enabling LINE users to interact with Aleph agents through direct messages and group chats.

**Goals:**
- Full-featured LINE Bot: rich messages (Flex, Quick Reply, Template), group chats, DM
- Aleph-native Rust implementation — no TypeScript/JS patterns copied directly
- Leverage Aleph's existing channel infrastructure (Channel trait, ChannelRegistry, ChannelPolicy)
- Webhook security via HMAC-SHA256 signature (matching LINE's native security model)

**Non-Goals:**
- LINE Login OAuth (deferred to future phase)
- Audio/video streaming (LINE has native support; deferred)

---

## 2. Architecture

### 2.1 Directory Structure

```
src/gateway/interfaces/line/
├── mod.rs          # LineChannel impl Channel + LineChannelFactory + register_with_plugin()
├── config.rs       # LineConfig + LineChannelConfig + validate()
├── types.rs        # LINE event types (serde-deserializable from LINE webhook payload)
├── webhook.rs      # HMAC-verified webhook server (TCP socket + manual HTTP parsing)
└── message_ops.rs # Outbound message builders (text, Flex, Quick Reply, Template, media)
```

**Key principle:** Each file has a single focused responsibility. `mod.rs` orchestrates; `webhook.rs` handles inbound; `message_ops.rs` handles outbound.

### 2.2 Channel Adapter Pattern

The `LineChannel` struct implements Aleph's `Channel` trait:

```rust
pub struct LineChannel {
    info: ChannelInfo,
    config: LineConfig,
    channel_state: ChannelState,
    api: Option<Arc<LineMessagingApi>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}
```

**Lifecycle:**
1. `start()` → validates config, creates API client, spawns webhook server task
2. `stop()` → signals shutdown, cleans up webhook listener
3. Inbound messages flow: LINE server → webhook server → `channel_state.sender()` → ChannelRegistry forwarder
4. Outbound: `send()` → `LineMessagingApi` → LINE Messaging API

### 2.3 Webhook Server

**Pattern:** Raw TCP socket with manual HTTP parsing — matching Feishu's approach in `feishu/webhook.rs`.

**Flow:**
```
TCP listener on webhook_host:webhook_port
    → accept connection
    → parse HTTP request (method, path, headers, body)
    → verify HMAC-SHA256 signature (X-Line-Signature header)
    → deserialize JSON body to LineEvent
    → construct InboundMessage
    → send to channel_state.sender()
```

**HMAC verification:**
- Signature: `X-Line-Signature` header (base64-encoded HMAC-SHA256)
- Payload: HTTP body bytes
- Key: `channel_secret`
- Uses `hmac` + `sha2` crates (already available in Aleph)

**Webhook path:** `/{webhook_path}` (configurable, default `/line/webhook`)

### 2.4 LINE Event Types

Deserialized directly from LINE's webhook payload (`types.rs`):

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LineEvent {
    Message { replyToken: String, source: LineSource, message: LineMessage },
    Follow { replyToken: String, source: LineSource },
    Unfollow { source: LineSource },
    Join { replyToken: String, source: LineSource },
    Leave { source: LineSource },
    Postback { replyToken: String, source: LineSource, postback: PostbackData },
    Beacon { replyToken: String, source: LineSource, beacon: BeaconData },
}

#[derive(Debug, Clone, Deserialize)]
pub struct LineSource {
    #[serde(rename = "type")]
    pub source_type: LineSourceType, // "user" | "group" | "room"
    pub userId: Option<String>,
    pub groupId: Option<String>,
    pub roomId: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LineSourceType {
    User,
    Group,
    Room,
}
```

---

## 3. Configuration

### 3.1 LineConfig

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineConfig {
    /// Long-lived channel access token from LINE Console
    pub channel_access_token: String,

    /// Channel secret from LINE Console
    pub channel_secret: String,

    /// Webhook path (default: "line/webhook")
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,

    /// Webhook server host (default: "0.0.0.0")
    #[serde(default = "default_webhook_host")]
    pub webhook_host: String,

    /// Webhook server port (default: 8080)
    #[serde(default = "default_webhook_port")]
    pub webhook_port: u16,

    /// DM access policy
    #[serde(default)]
    pub dm_policy: LineDmPolicy,

    /// Group access policy
    #[serde(default)]
    pub group_policy: LineGroupPolicy,

    /// Allowed user IDs for DMs (empty = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Allowed group IDs (empty = allow all groups)
    #[serde(default)]
    pub allowed_groups: Vec<String>,

    /// Send typing indicator while processing
    #[serde(default = "default_true")]
    pub send_typing: bool,

    /// Message coalescing configuration
    #[serde(default)]
    pub coalescing: Option<CoalescingConfig>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LineDmPolicy {
    #[default]
    Pairing,
    Allowlist,
    Open,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LineGroupPolicy {
    #[default]
    Allowlist,
    Open,
    Disabled,
}
```

**Validation:** `validate()` checks `channel_access_token` and `channel_secret` are non-empty.

### 3.2 Registration

```rust
// In interfaces/mod.rs:
pub mod line;
pub use line::{LineChannel, LineChannelFactory, LineConfig};

pub fn register_channel_plugins() {
    telegram::register_with_plugin();
    line::register_with_plugin();
}
```

```rust
// In interfaces/line/mod.rs:
pub fn register_with_plugin() {
    crate::gateway::CHANNEL_FACTORY.register("line", Arc::new(LineChannelFactory::new()));
}
```

---

## 4. Capabilities

```rust
fn capabilities() -> ChannelCapabilities {
    ChannelCapabilities {
        attachments: true,
        images: true,
        audio: true,
        video: true,
        reactions: true,       // LINE supports emoji reactions
        replies: true,         // LINE reply token for threading
        editing: false,         // LINE doesn't support edit
        deletion: true,        // LINE supports message deletion
        typing_indicator: true,
        read_receipts: false,
        rich_text: true,       // Flex, Quick Reply, Template
        max_message_length: 5000, // LINE text limit
        max_attachment_size: 50 * 1024 * 1024, // 50MB
        stream_protocol: StreamProtocol::EditBased, // LEP-style progressive delivery
    }
}
```

---

## 5. Message Handling

### 5.1 Inbound

**ConversationId mapping:**
- DM: `LineSource.userId` → `ConversationId(user_id)`
- Group: `LineSource.groupId` → `ConversationId(group_id)`
- Room: `LineSource.roomId` → `ConversationId(room_id)`

**UserId mapping:** `LineSource.userId` → `UserId(user_id)`

**Group detection:** `source.source_type != LineSourceType::User` → `is_group: true`

**Reply token handling:** LINE's `replyToken` is stored in `InboundMessage.raw` as JSON for the agent layer to use when composing replies.

### 5.2 Outbound (message_ops.rs)

All send operations use `reqwest` to call LINE Messaging API v2.

```rust
// Core send function
async fn push_message(&self, token: &str, payload: LinePushPayload) -> ChannelResult<SendResult>

// Convenience wrappers
async fn push_text(&self, to: &str, text: &str) -> ChannelResult<SendResult>
async fn push_flex(&self, to: &str, alt: &str, flex: FlexMessage) -> ChannelResult<SendResult>
async fn push_quick_reply(&self, to: &str, text: &str, actions: Vec<QuickReplyAction>) -> ChannelResult<SendResult>
async fn push_template(&self, to: &str, alt: &str, template: TemplateMessage) -> ChannelResult<SendResult>
```

**Reply token usage:** When replying to an inbound message, use `push_reply()` which sends to `/v2/bot/message/reply` with `replyToken`. For proactive messages (no reply token), use `/v2/bot/message/push`.

**Chunking:** Long text (>5000 chars) is chunked into multiple messages, matching Telegram's chunking behavior.

---

## 6. Security

### 6.1 Webhook HMAC

- **Header:** `X-Line-Signature`
- **Algorithm:** HMAC-SHA256
- **Payload:** Raw HTTP body bytes (NOT the parsed JSON string)
- **Secret:** `channel_secret`

```rust
fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    let sig_bytes = BASE64.decode(signature).unwrap();
    constant_time_eq(&expected[..], &sig_bytes)
}
```

### 6.2 ChannelPolicy

Reuse `WhatsAppPolicy` from `channel_policy.rs` for DM/group evaluation. Line's user IDs are opaque strings (not phone numbers), so `E164Number` normalization is not applicable — `WhatsAppPolicy::matches_allowlist` is string-comparison-based and works directly.

---

## 7. Key Differences from openclaw

| Aspect | openclaw (TypeScript) | Aleph (Rust) |
|--------|----------------------|--------------|
| Webhook | Express middleware | Raw TCP socket + manual HTTP parse |
| Type safety | Runtime JS | Compile-time Rust structs/enums |
| Runtime facade | JS dynamic module loading | Direct trait impl |
| Config validation | Zod schema | Rust struct + `validate()` |
| Rich messages | JS helper functions | Rust `message_ops.rs` functions |
| HMAC | Node crypto | `hmac` + `sha2` crates |
| Platform integration | Plugin-based with facade | Direct `Channel` trait impl |

---

## 8. Dependencies

**New crates:**
- `hmac`, `sha2` — already in Aleph's dependency tree
- `base64` — already in Aleph's dependency tree
- `reqwest` — already in Aleph's dependency tree (used by other interfaces)

**No new heavy dependencies.** The webhook server uses `tokio::net::TcpListener` (already available).

---

## 9. File Inventory

| File | Purpose |
|------|---------|
| `interfaces/line/mod.rs` | `LineChannel` impl `Channel`, `LineChannelFactory` impl `ChannelFactory`, `register_with_plugin()` |
| `interfaces/line/config.rs` | `LineConfig`, `LineDmPolicy`, `LineGroupPolicy`, `validate()` |
| `interfaces/line/types.rs` | `LineEvent`, `LineSource`, `LineMessage`, `LineReplyToken`, serde Deserialize impls |
| `interfaces/line/webhook.rs` | `run_webhook_server()`, `handle_connection()`, HMAC verification, event dispatch |
| `interfaces/line/message_ops.rs` | `LineMessagingApi`, `push_text()`, `push_flex()`, `push_quick_reply()`, `push_template()` |

---

## 10. Implementation Order

1. `types.rs` — LINE event types (no dependencies, pure serde)
2. `config.rs` — configuration struct (no dependencies)
3. `webhook.rs` — webhook server with HMAC (uses `types.rs`)
4. `message_ops.rs` — outbound API calls (uses `types.rs`)
5. `mod.rs` — `LineChannel` impl `Channel` (assembles all above)
6. Integration: add to `interfaces/mod.rs`, `register_with_plugin()`
7. Tests: unit tests for HMAC verification, event parsing, config validation
