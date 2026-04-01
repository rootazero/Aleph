# Feishu/Lark Channel Design

**Date**: 2026-03-22
**Status**: Draft
**Scope**: Add Feishu (飞书) / Lark channel support to Aleph

## Summary

Add a new channel implementation for Feishu/Lark, following Aleph's existing `Channel` trait + `ChannelFactory` pattern. Feishu and Lark are treated as a single unified channel (differentiated by `domain` config field), using WebSocket long-polling for event subscription and HTTP API for message sending.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Feishu vs Lark | Unified channel, `domain` field | Same API, different endpoints — one codebase |
| Connection mode | WebSocket only | Self-hosted, no public IP needed; Webhook deferred |
| SDK approach | Custom `FeishuClient` module | No official Rust SDK; wrap reqwest + tokio-tungstenite |
| Message types (MVP) | Text + Image | Covers 80% use cases; audio/video/card deferred |
| Group chat | Basic support | @mention required; single session per group |
| Multi-account | No | Single channel instance per config entry; Aleph supports multiple `[[channels]]` |
| Streaming output | No | Feishu text messages don't support edit; card streaming deferred |

## Module Structure

```
src/gateway/interfaces/feishu/
├── mod.rs          # FeishuChannel, FeishuChannelFactory, FeishuConfig
├── client.rs       # FeishuClient: token lifecycle, HTTP API, WebSocket connection
├── types.rs        # Feishu API request/response types, event enums
└── events.rs       # WebSocket event parsing, dispatch to InboundMessage
```

## Configuration

```toml
[[channels]]
id = "feishu"
channel_type = "feishu"
enabled = true
agent = "default"

[channels.config]
app_id = "cli_xxx"
app_secret = "secret"
domain = "feishu"           # "feishu" | "lark" | custom URL
bot_name = "Aleph"          # For group chat @mention detection
dm_allowed = true
groups_allowed = true
require_mention = true      # Group chat requires @bot to respond
```

### Domain Mapping

| `domain` value | Base URL |
|----------------|----------|
| `"feishu"` (default) | `https://open.feishu.cn` |
| `"lark"` | `https://open.larksuite.com` |
| Custom URL | Used as-is (private deployment) |

## Data Flow

```
Feishu Server
  ↓ WebSocket long connection
FeishuClient (receive events + heartbeat keepalive)
  ↓ Raw JSON event
events.rs (parse event type, extract message content)
  ↓ FeishuEvent
FeishuChannel (convert to InboundMessage)
  ↓ channel_state.sender()
ChannelRegistry → InboundMessageRouter → ExecutionEngine
  ↓ Agent processing complete
ReplyEmitter → FeishuChannel::send()
  ↓
FeishuClient::send_message() (HTTP API)
  ↓
Feishu Server → User
```

## Component Design

### FeishuConfig

```rust
#[derive(Debug, Deserialize)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    #[serde(default = "default_domain")]
    pub domain: String,              // "feishu" | "lark" | custom URL
    pub bot_name: Option<String>,
    #[serde(default = "default_true")]
    pub dm_allowed: bool,
    #[serde(default)]
    pub groups_allowed: bool,
    #[serde(default = "default_true")]
    pub require_mention: bool,
}
```

### FeishuClient (client.rs)

Encapsulates all Feishu API interaction:

```rust
pub struct FeishuClient {
    app_id: String,
    app_secret: String,
    base_url: String,
    http: reqwest::Client,
    token: Arc<RwLock<TokenState>>,
}

struct TokenState {
    access_token: String,
    expires_at: Instant,  // 2h validity, refresh 5min early
}
```

**Token management:**
- Startup: `POST /open-apis/auth/v3/app_access_token/internal` → get `app_access_token`
- Background task: refresh every ~115 minutes (5 min before 2h expiry)
- All API calls read from shared `token` state

**WebSocket connection:**
- `POST /open-apis/callback/ws/endpoint` → get WSS URL + ticket
- Connect to WSS URL via `tokio-tungstenite`
- Handle frames: Ping→Pong, Event→dispatch
- Auto-reconnect on disconnect (exponential backoff: 1s → 2s → 4s → ... → 60s cap)

**WebSocket frame envelope:**

Feishu WS frames use a JSON envelope structure:
```json
{
  "header": {
    "event_id": "unique-id",
    "event_type": "im.message.receive_v1",
    "token": "verification_token",
    "create_time": "1234567890"
  },
  "event": { ... }
}
```
The `POST /callback/ws/endpoint` response includes `url` (WSS endpoint) and `client_config` (reconnect settings). The `service_id` may need to be included as a query parameter on the WS URL.

**HTTP API methods:**
- `send_text(chat_id, text, reply_to)` → `POST /open-apis/im/v1/messages`
- `send_image(chat_id, image_key)` → `POST /open-apis/im/v1/messages`
- `reply_message(message_id, msg_type, content)` → `POST /open-apis/im/v1/messages/{id}/reply`
- `upload_image(data, filename)` → `POST /open-apis/im/v1/images` → returns `image_key`
- `download_media(message_id, file_key)` → `GET /open-apis/im/v1/messages/{id}/resources/{file_key}`
- `get_bot_info()` → `GET /open-apis/bot/v3/info` (health check)

### Event Types (types.rs)

```rust
/// WebSocket frame types
enum WsFrameType { Event, Ping, Pong }

/// Parsed Feishu events
enum FeishuEvent {
    MessageReceive {
        message_id: String,
        chat_id: String,
        chat_type: ChatType,       // P2p | Group
        sender_id: String,         // open_id
        sender_name: Option<String>,
        message_type: String,      // "text" | "image" | ...
        content: String,           // JSON string
        mentions: Vec<Mention>,
        parent_id: Option<String>,
    },
    Unknown(String),
}

enum ChatType { P2p, Group }

struct Mention {
    key: String,       // @_user_1 placeholder
    id: String,        // open_id
    name: String,
    is_bot: bool,
}
```

### Event Parsing (events.rs)

Parses raw WebSocket JSON frames into `FeishuEvent`:
- Identify frame type (event vs ping/pong)
- For `im.message.receive_v1` events: extract message fields, mentions, chat type
- Unknown event types → `FeishuEvent::Unknown`

### Message Conversion (mod.rs)

**Inbound (Feishu → Aleph):**
1. Group chat: if `require_mention=true` and bot not mentioned → skip
2. Text extraction:
   - `"text"` → parse JSON content, extract `.text` field, remove bot mention placeholders
   - `"image"` → download image via `download_media()` → `Attachment { type: Image }`
   - Other types → skip (outside MVP scope)
3. Build `InboundMessage` with channel_id, conversation_id=chat_id, sender_id, is_group, etc.

**Outbound (Aleph → Feishu):**
1. Check for image attachments → `upload_image()` → `send_image()`
2. Otherwise → `send_text()`
3. If `reply_to` is set → use `reply_message()` API instead
4. Return `SendResult { message_id }`

### FeishuChannel Lifecycle

```rust
pub struct FeishuChannel {
    info: ChannelInfo,
    config: FeishuConfig,
    channel_state: ChannelState,
    client: Option<FeishuClient>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}
```

**start():**
1. Create `FeishuClient`, acquire initial token
2. Call `get_bot_info()` to validate credentials
3. Connect WebSocket, spawn event loop task
4. Spawn token refresh background task
5. Set status → `Connected`

**stop():**
1. Send shutdown signal via `oneshot`
2. Await WebSocket task termination
3. Set status → `Disconnected`

**send():**
1. Image attachments → upload → send image message
2. Text only → send text message
3. Has `reply_to` → use reply API
4. Return `SendResult`

### Capabilities

```rust
ChannelCapabilities {
    attachments: false,          // File attachments deferred; platform supports them
    images: true,
    audio: false,
    video: false,
    reactions: false,
    replies: true,
    editing: false,           // Feishu text messages don't support edit
    deletion: false,
    typing_indicator: false,
    read_receipts: false,
    rich_text: false,
    max_message_length: 4096,
    max_attachment_size: 20 * 1024 * 1024, // 20MB
}
```

### ChannelProvider Implementation

`FeishuChannel` must also implement `ChannelProvider` (defined in `channel.rs`) to provide interaction metadata used by the context aggregator:

```rust
impl ChannelProvider for FeishuChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest {
            paradigm: InteractionParadigm::Chat,
            max_output_chars: 4096,
            supports_streaming: false,
            prefer_compact: false,
        }
    }
}
```

Note: The `agent` field in the channel config (e.g., `agent = "default"`) is resolved externally by `AgentResolver` (`src/config/agent_resolver.rs`), not by `FeishuConfig`.

## Security Notes

- `app_secret` is stored in plaintext in `config.toml`, consistent with how other channels (Telegram `bot_token`, Discord `bot_token`) handle secrets today
- Vault integration for channel secrets is a broader improvement deferred for all channels, not Feishu-specific

## Error Handling

| Scenario | Strategy |
|----------|----------|
| Token refresh failure | Retry 3x at 5s intervals; set status → `Error` on exhaustion |
| WebSocket disconnect | Exponential backoff reconnect (1s→2s→4s→...→60s cap), never panic |
| Message send failure | Return `ChannelError`, upper layer handles |
| Unknown message type | Warn log, skip message |
| Invalid credentials | `start()` returns error, channel stays `Disconnected` |
| API rate limited (429) | Return `ChannelError::RateLimited` with `retry_after_secs` from response header |
| Duplicate events on reconnect | Maintain bounded set of recent `message_id` values (~1000); skip duplicates |

## Integration Points

**New files:**
- `src/gateway/interfaces/feishu/mod.rs`
- `src/gateway/interfaces/feishu/client.rs`
- `src/gateway/interfaces/feishu/types.rs`
- `src/gateway/interfaces/feishu/events.rs`

**Modified files:**
- `src/gateway/interfaces/mod.rs` — add `pub mod feishu;`
- `src/gateway/handlers/channel.rs` — add `"feishu"` arm to `create_channel_from_config()` match block
- `src/bin/aleph-server/commands/start/builder/subsystems.rs` — add `"feishu"` case to startup channel creation

**No changes to:**
- `Channel` trait, `ChannelRegistry`, `InboundMessageRouter`, `ReplyEmitter`
- Any existing channel implementations

## Dependencies

- `tokio-tungstenite` — WebSocket client (already in `Cargo.toml`, used by Nostr/Slack/Mattermost)

## Future Extensions (Not In Scope)

- Webhook connection mode
- Card/rich text message rendering
- Streaming card output
- Audio/video/file message types
- Typing indicator (via reaction API)
- Message editing/deletion
- Multi-account within single channel
- Group session scoping modes (by sender, by topic)

## Test Plan

- Unit tests for event parsing (`events.rs`) — mock WebSocket JSON payloads
- Unit tests for message conversion — Feishu event ↔ InboundMessage/OutboundMessage
- Unit tests for @mention extraction and removal
- Unit tests for domain → base_url mapping
- Integration test: FeishuConfig deserialization from TOML
- Unit test for WebSocket reconnection state machine — verify backoff progression and cap at 60s
- Manual test: connect to real Feishu bot, send/receive messages
