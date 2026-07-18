# QQ Channel Design for Aleph

> Date: 2026-04-16  
> Scope: MVP for QQ Bot Official API integration  
> Status: Approved

---

## Overview

This document describes the design for adding a **QQ Channel** to Aleph, enabling users to interact with Aleph through QQ's official Bot API. The design is inspired by [OpenClaw](https://github.com/AIChatClaw/OpenClaw)'s QQ implementation but fully adapted to Aleph's Rust-native architecture and existing channel patterns.

### Goals

- Provide a native Rust QQ channel that aligns with Aleph's existing `Channel` trait architecture.
- Support C2C (private) and Group (@-mention) messaging via QQ's official Bot API.
- Reuse Aleph's proven patterns from Telegram (multi-account, access control, error cooldown, chunking).
- Avoid external protocol dependencies (e.g., OneBot/Go-CQHTTP) for the initial release.

### Non-Goals (MVP)

- Guild/channel messaging.
- Audio/video file support.
- Real-time streaming output (block-streaming).
- Advanced pairing flows beyond simple allowlists.

---

## Protocol Choice

**Selected: QQ Official Bot API**

- **API Base**: `https://api.sgroup.qq.com`
- **Token Endpoint**: `https://bots.qq.com/app/getAppAccessToken`
- **Gateway**: WebSocket-based event streaming.
- **Authentication**: `appId` + `clientSecret` → `access_token` (expires in ~7200s).

**Rationale**: This protocol requires zero external daemon processes, fitting Aleph's self-hosted, minimal-dependency philosophy. It also allows us to fully leverage Rust's async/await and type safety.

---

## Architecture

### Module Structure

```text
src/gateway/interfaces/qq/
├── mod.rs           # QQChannel: implements Channel trait
├── api.rs           # HTTP API client (token refresh, send, upload)
├── gateway.rs       # WebSocket Gateway (connect, heartbeat, dispatch)
├── config.rs        # QQConfig, QQAccountConfig
├── handlers.rs      # Event parser (C2C, Group @-message)
├── delivery.rs      # Outbound logic, chunking, passive→proactive fallback
├── access.rs        # Access control (allowlist, simplified pairing)
├── error.rs         # QQ-specific errors and retry classification
└── types.rs         # QQ platform types (Payload, Event, Media, etc.)
```

### Design Principles

1. **Follow Existing Patterns**: The structure mirrors `telegram/`, `discord/`, and other mature channels in Aleph. `QQChannel` implements the `Channel` trait and registers with `ChannelRegistry`.
2. **Gateway/REST Separation**: `gateway.rs` handles the WebSocket lifecycle; `api.rs` handles REST calls. They communicate through typed events.
3. **Multi-Account Ready**: Configuration follows `TelegramConfigV2`'s `accounts: Vec<AccountConfig>` pattern.
4. **Graceful Degradation**: When passive reply limits are exceeded, messages automatically fall back to proactive sends.

---

## Data Flow

### Inbound Flow

```text
QQ WS Gateway
    │
    ▼
gateway.rs: receive raw payload
    │
    ▼
handlers.rs: dispatch by payload.t
    ├── C2C_MESSAGE_CREATE     → C2CMessageEvent
    └── GROUP_AT_MESSAGE_CREATE → GroupMessageEvent
    │
    ▼
Build InboundMessage
    • channel_id      = "qq"
    • conversation_id = "c2c:{openid}" | "group:{group_openid}"
    • sender_id       = UserId(openid)
    • text            = plain text with @bot mention removed
    • attachments     = image/file metadata
    • reply_to        = referenced message id (if any)
    • is_group        = true | false
    │
    ▼
ChannelState::send_inbound() → broadcast
    │
    ▼
Gateway EventBus → AgentLoop
```

### Outbound Flow

```text
AgentLoop → OutboundMessage
    │
    ▼
QQChannel::send()
    │
    ▼
delivery.rs
    ├── Check length (chunk if > 4000 chars)
    ├── Upload image attachments via api.rs → file_info
    └── Build OutboundPayload
    │
    ▼
api.rs: POST /v2/{users|groups}/{id}/messages
    ├── Passive reply: include msg_id
    └── Proactive: omit msg_id
    │
    ▼
Return SendResult (QQ message_id)
```

---

## Key Components

### 1. QQChannel

```rust
pub struct QQChannel {
    info: ChannelInfo,
    channel_state: ChannelState,
    config: QQConfig,
    api_client: Arc<QQApiClient>,
    gateway_handle: Option<JoinHandle<()>>,
    reply_tracker: Arc<ReplyTracker>,
}

#[async_trait]
impl Channel for QQChannel {
    fn info(&self) -> &ChannelInfo;
    fn state(&self) -> &ChannelState;
    async fn start(&mut self) -> ChannelResult<()>;
    async fn stop(&mut self) -> ChannelResult<()>;
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult>;
    // ...other trait methods
}
```

**Capabilities** (MVP):

| Capability        | Value |
|-------------------|-------|
| attachments       | true  |
| images            | true  |
| audio             | false |
| video             | false |
| reactions         | false |
| replies           | true  |
| editing           | false |
| deletion          | false |
| typing_indicator  | false |
| read_receipts     | false |
| rich_text         | true  |
| max_message_length| 4000  |
| max_attachment_size| 8 MB |
| stream_protocol   | None  |

### 2. QQApiClient

Encapsulates all HTTP interactions:

- `get_access_token(app_id, client_secret) -> Result<String>`
- `send_c2c_message(openid, payload) -> Result<SendResult>`
- `send_group_message(group_openid, payload) -> Result<SendResult>`
- `upload_media(...) -> Result<String>` (MVP stub if not immediately needed)

**Token Management**:
- A `TokenManager` (protected by `tokio::sync::Mutex`) caches tokens per `app_id`.
- Auto-refresh triggers before expiry with singleflight semantics (only one refresh in flight per app).

### 3. QQGateway

Manages the WebSocket connection to QQ's Gateway URL (`wss://api.sgroup.qq.com/websocket`).

Lifecycle:
1. Connect.
2. Receive `Hello` (includes `heartbeat_interval`).
3. Send `Identify` with `token`, `intents`, and `shard` info.
4. Start heartbeat task.
5. Dispatch events to `handlers.rs` via `mpsc::Sender<QQEvent>`.
6. On `Reconnect` or disconnect, back off and reconnect.

**Intents** (MVP):
- `1 << 25` (C2C messages)
- `1 << 26` (Group @-messages)

### 4. Handlers

Parses raw WebSocket payloads into typed events:

```rust
pub enum QQEvent {
    C2CMessage(C2CMessageEvent),
    GroupAtMessage(GroupMessageEvent),
    HeartbeatAck,
    Reconnect { url: String },
    Unknown { t: String, d: serde_json::Value },
}
```

For each message event:
- Strip `@bot` mention from `content`.
- Map `attachments` to Aleph `Attachment` structs.
- Build `InboundMessage` and broadcast it.

### 5. Delivery & Passive Reply Handling

QQ's passive reply API has strict limits:
- **Max 4 replies per `msg_id`**.
- **Must occur within 1 hour**.

`delivery.rs` uses a `ReplyTracker` (`DashMap<String, ReplyRecord>`):

```rust
pub struct ReplyRecord {
    count: u8,
    first_reply_at: Instant,
}
```

**Logic**:
1. If `msg_id` exists in tracker and `count < 4` and `elapsed < 1h` → send passive reply (include `msg_id`).
2. Else → send proactive message (omit `msg_id`).
3. On successful passive reply, increment tracker count.

If QQ returns rate-limit errors (`304023`, `304024`), the channel enters an **error cooldown** (similar to `telegram::error_cooldown`), pausing sends to that conversation for a short duration.

### 6. Configuration

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQConfig {
    pub accounts: Vec<QQAccountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQAccountConfig {
    pub id: String,
    pub app_id: String,
    pub client_secret: String,
    pub enabled: bool,
    pub allowed_users: Vec<String>,      // openid allowlist
    pub allowed_groups: Vec<String>,    // group_openid allowlist
    pub dm_policy: QQDmPolicy,
    pub group_policy: QQGroupPolicy,
}
```

MVP access control is allowlist-based. Pairing code flow is reserved for a future iteration.

---

## Error Handling

| Error Class          | Trigger                        | Strategy                                      |
|----------------------|--------------------------------|-----------------------------------------------|
| `AuthFailed`         | Invalid token                  | Refresh once, fail if still invalid           |
| `RateLimited`        | HTTP 429 or QQ limit codes     | Read `retry_after`, enter cooldown            |
| `MessageTooLarge`    | Exceeds max length             | Chunk in `delivery.rs` and retry              |
| `SendFailed`         | Network/API failure            | Exponential backoff, up to `max_retries`      |
| `PassiveReplyExpired`| Window closed or count exceeded| Fall back to proactive send automatically     |

---

## Testing Strategy

1. **Unit Tests**
   - Token refresh logic in `api.rs`.
   - Payload parsing in `handlers.rs`.
   - Message chunking in `delivery.rs`.
   - Passive reply tracker state transitions.

2. **Mock Gateway Tests**
   - Spin up a local `tokio::net::TcpListener` WebSocket server.
   - Simulate `Hello`, `Heartbeat`, `C2C_MESSAGE_CREATE`, and `Reconnect`.
   - Assert `QQGateway` handles reconnection correctly.

3. **Integration Tests**
   - Use `wiremock` or `mockito` to mock QQ REST API.
   - End-to-end: mock inbound WS event → Aleph processing → mock outbound REST call.

---

## Decisions Log

| Decision                                  | Choice                                  | Rationale                                           |
|-------------------------------------------|-----------------------------------------|-----------------------------------------------------|
| Protocol                                  | QQ Official Bot API                     | Zero external daemons, native Rust, self-hosted     |
| Architecture pattern                      | Full Channel impl (like Telegram)       | Aligns with Aleph conventions, proven scalability   |
| MVP messaging scopes                      | C2C + Group only                        | Covers 90% of use cases, keeps complexity low       |
| MVP media support                         | Text + images only                      | Sufficient for MVP; audio/video deferred            |
| Streaming                                 | Disabled in MVP                         | QQ lacks edit-based streaming; block-stream later   |
| Passive reply overflow                    | Auto-fallback to proactive              | Matches OpenClaw behavior, required by API limits   |
| Multi-account                             | Supported from day one                  | Follows `TelegramConfigV2`, avoids rework           |
| Pairing flow                              | Simplified allowlist (MVP)              | Pairing codes reserved for post-MVP enhancement     |

---

## References

- Aleph `Channel` trait: `src/gateway/channel.rs`
- Telegram channel (reference impl): `src/gateway/interfaces/telegram/`
- OpenClaw QQ extension: `/Volumes/TBU4/Github/openclaw/extensions/qqbot/`
- QQ Bot Official Docs: https://bot.q.qq.com/wiki/
