# Feishu Channel Optimization Design

> Reference: OpenClaw Feishu/Lark implementation (~8000 lines, TypeScript, production-grade)
> Scope: Aleph Feishu channel restructure + feature completion + robustness hardening

## Motivation

Aleph's Feishu channel (1955 lines, 5 files) is a functional MVP but has significant gaps compared to production-grade implementations like OpenClaw's. Key issues:

1. **God object** — `client.rs` (639 lines) mixes token management, HTTP API, WebSocket, streaming cards, and media handling
2. **Missing MessageOperations** — Users cannot use cross-channel message tools (reply/react/send) on Feishu
3. **No sender name** — `sender_name` is always None, degrading agent context in group chats
4. **Primitive dedup** — In-memory VecDeque without TTL, no capacity management
5. **No card interaction** — Button clicks ignored entirely
6. **No config validation** — Bad credentials only caught at runtime during `start()`
7. **No group session scope** — All group messages share one session, no topic isolation
8. **Leaky abstraction** — `ws_reconnect_handle()` exposes client internals (P5 violation)

## Decisions Made During Brainstorming

| Topic | Decision | Rationale |
|-------|----------|-----------|
| Approach | Responsibility restructure (not incremental patch, not over-abstraction) | P2 high cohesion, P6 file splitting |
| Card interaction | Basic: button value → plain message → agent loop | R8 LLM sovereignty — no action dispatch engine |
| Webhook | Deferred to future | WebSocket sufficient for self-hosted personal assistant |
| Group session scope | `group` + `group_topic` (no `_sender` variants) | Personal assistant context; per-sender isolation unnatural |
| Dedup persistence | Not needed — increase capacity + add TTL | WS doesn't replay confirmed messages; simplicity (P6) |
| Sender name | LRU cache, 500 capacity, 1h TTL | Non-blocking; graceful degradation on permission error |

## File Structure (After)

```
core/src/gateway/interfaces/feishu/
├── mod.rs          # FeishuChannel: lifecycle + Channel trait (~150 lines)
├── config.rs       # FeishuConfig + validation + GroupSessionScope (~100 lines)  [NEW]
├── types.rs        # Pure data types: events, API request/response (~200 lines)  [SLIMMED]
├── events.rs       # WS frame parsing, text extraction, CardAction (~290 lines)  [EXTENDED]
├── auth.rs         # TokenManager: get/refresh/background renewal (~120 lines)   [FROM client.rs]
├── api.rs          # FeishuApi: all HTTP API calls (~350 lines)                  [FROM client.rs]
├── websocket.rs    # WS connect loop + reconnect + event dispatch (~180 lines)   [FROM mod.rs]
├── streaming.rs    # FeishuStreamingCard + FeishuEventEmitter (~270 lines)       [UNCHANGED]
├── message_ops.rs  # MessageOperations trait impl (~100 lines)                   [NEW]
├── user_cache.rs   # Sender name LRU cache (~80 lines)                          [NEW]
└── dedup.rs        # MessageDedup with TTL (~60 lines)                           [NEW]
```

**Deleted**: `client.rs` — split into `auth.rs` + `api.rs`

## Section 1: Responsibility Restructure

### 1.1 `client.rs` → `auth.rs` + `api.rs`

**`auth.rs` — TokenManager**

```rust
pub struct TokenManager {
    app_id: String,
    app_secret: String,
    base_url: String,
    http: reqwest::Client,
    token: Arc<RwLock<TokenState>>,
}

impl TokenManager {
    pub async fn get_token(&self) -> Result<String, String>  // lazy refresh
    pub async fn refresh_token(&self) -> Result<(), String>   // force refresh
    pub fn spawn_background_refresh(&self, shutdown: watch::Receiver<bool>)
}
```

Extracted from `client.rs:67-177`. Logic unchanged — just isolated into its own struct.

**`api.rs` — FeishuApi**

```rust
pub struct FeishuApi {
    auth: Arc<TokenManager>,
    http: reqwest::Client,
    base_url: String,
    bot_open_id: Arc<RwLock<Option<String>>>,
}

impl FeishuApi {
    // Bot info
    pub async fn get_bot_info(&self) -> Result<BotInfo, String>
    pub async fn bot_open_id(&self) -> Option<String>

    // WebSocket
    pub async fn get_ws_endpoint(&self) -> Result<String, String>
    pub async fn refresh_ws_endpoint(&self) -> Result<String, String>  // replaces ws_reconnect_handle()

    // Send messages
    pub async fn send_text(&self, chat_id, text, reply_to) -> Result<String, FeishuSendError>
    pub async fn send_image(&self, chat_id, image_key, reply_to) -> Result<String, FeishuSendError>
    pub async fn send_card(&self, chat_id, markdown, reply_to) -> Result<String, FeishuSendError>
    pub async fn reply_message(&self, message_id, msg_type, content) -> Result<String, FeishuSendError>

    // Streaming cards
    pub async fn create_streaming_card(&self, initial_text) -> Result<String, String>
    pub async fn send_card_message(&self, chat_id, card_id, reply_to) -> Result<String, FeishuSendError>
    pub async fn update_streaming_card(&self, card_id, content, seq) -> Result<(), String>
    pub async fn close_streaming_card(&self, card_id, summary, seq) -> Result<(), String>

    // Media
    pub async fn upload_image(&self, data, filename) -> Result<String, String>
    pub async fn download_media(&self, message_id, file_key) -> Result<Vec<u8>, String>

    // Reactions
    pub async fn add_reaction(&self, message_id, emoji_type) -> Result<String, String>
    pub async fn remove_reaction(&self, message_id, reaction_id) -> Result<(), String>

    // User profile (NEW)
    pub async fn get_user_info(&self, open_id) -> Result<Option<String>, String>  // returns display name
}
```

**Key change**: `ws_reconnect_handle()` replaced by `refresh_ws_endpoint()`. The WS loop calls this method instead of reaching into client internals. This fixes the P5 (Least Knowledge) violation.

### 1.2 `mod.rs` — Slimmed FeishuChannel

WebSocket loop (lines 135-296) moves to `websocket.rs`. What remains:

```rust
pub struct FeishuChannel {
    info: ChannelInfo,
    config: FeishuConfig,
    channel_state: ChannelState,
    api: Option<Arc<FeishuApi>>,
    user_cache: Option<Arc<UserProfileCache>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl Channel for FeishuChannel {
    async fn start(&mut self) -> ChannelResult<()>  // init api, validate, spawn WS, register MessageOps
    async fn stop(&mut self) -> ChannelResult<()>
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult>
}
```

### 1.3 `websocket.rs` — Connection Loop

Extracted from `mod.rs:135-296`. Takes ownership of:
- WebSocket connect/reconnect with exponential backoff
- Frame dispatch to event parser
- Dedup check
- Group/mention/DM filtering
- InboundMessage construction (with sender name from UserProfileCache)
- Shutdown signal handling

```rust
pub async fn run_ws_loop(
    api: Arc<FeishuApi>,
    initial_url: String,
    config: FeishuConfig,
    sender: mpsc::Sender<InboundMessage>,
    channel_id: ChannelId,
    bot_open_id: String,
    user_cache: Arc<UserProfileCache>,
    status_handle: StatusHandle,
    mut shutdown_rx: watch::Receiver<bool>,
)
```

Single top-level function, not a struct. Called via `tokio::spawn` from `FeishuChannel::start()`.

## Section 2: MessageOperations

New file `message_ops.rs`:

```rust
pub struct FeishuMessageOps {
    api: Arc<FeishuApi>,
}

impl MessageOperations for FeishuMessageOps {
    fn channel_id(&self) -> &str { "feishu" }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            reply: true,
            edit: false,    // Feishu text messages are immutable
            react: true,
            delete: false,  // Feishu Bot API doesn't support deletion
            send: true,
        }
    }

    async fn reply(&self, params) -> Result<MessageResult>   // POST /im/v1/messages/{id}/reply
    async fn edit(&self, _params) -> Result<MessageResult>    // return unsupported
    async fn react(&self, params) -> Result<MessageResult>    // POST /im/v1/messages/{id}/reactions
    async fn delete(&self, _params) -> Result<MessageResult>  // return unsupported
    async fn send(&self, params) -> Result<MessageResult>     // POST /im/v1/messages
}
```

**Registration**: In `FeishuChannel::start()`, after successful auth, register `FeishuMessageOps` into the `MessageOpsRegistry` (same pattern as Telegram/Discord).

## Section 3: Sender Name Cache

New file `user_cache.rs`:

```rust
pub struct UserProfileCache {
    api: Arc<FeishuApi>,
    cache: StdMutex<HashMap<String, CachedEntry>>,
    capacity: usize,    // 500
    ttl: Duration,      // 1 hour
}

enum CachedEntry {
    Found { name: String, fetched_at: Instant },
    NotAvailable { fetched_at: Instant },  // permission error or user not found
}

impl UserProfileCache {
    pub async fn resolve_name(&self, open_id: &str) -> Option<String>
}
```

**Behavior**:
- Cache hit + not expired → return immediately
- Cache miss → call `GET /open-apis/contact/v3/users/{open_id}?user_id_type=open_id`
- API failure → cache `NotAvailable` to avoid repeated failing requests for same user
- Capacity full → evict oldest entry (iterate to find min `fetched_at`)
- All operations are non-blocking for message processing (failure returns None)
- Required permission: `contact:user.base:readonly`

**Integration**: Called in `websocket.rs` when building `InboundMessage`, populating `sender_name` field.

## Section 4: Enhanced Dedup

New file `dedup.rs`:

```rust
pub struct MessageDedup {
    seen: VecDeque<(String, Instant)>,
    capacity: usize,  // 5000
    ttl: Duration,    // 24 hours
}

impl MessageDedup {
    pub fn new(capacity: usize, ttl: Duration) -> Self
    pub fn is_duplicate(&mut self, message_id: &str) -> bool
}
```

**`is_duplicate()` logic**:
1. Drain expired entries from front (lazy cleanup)
2. Linear scan remaining entries for match
3. If not found: push_back; if at capacity, pop_front
4. Return whether found

Replaces inline VecDeque logic in current `mod.rs:132, 167-175`.

## Section 5: Group Session Scope

In `config.rs`:

```rust
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupSessionScope {
    #[default]
    Group,
    GroupTopic,
}
```

`FeishuConfig` gains: `group_session_scope: GroupSessionScope` (default `Group`).

**Session key generation** (in `websocket.rs`):

```rust
let conversation_id = match config.group_session_scope {
    GroupSessionScope::Group => chat_id.clone(),
    GroupSessionScope::GroupTopic => {
        // root_id is the topic root; parent_id is fallback
        if let Some(ref topic_id) = root_id.or(parent_id.clone()) {
            format!("{}:topic:{}", chat_id, topic_id)
        } else {
            chat_id.clone()  // not in a topic thread
        }
    }
};
```

**Event extension**: `MessageBody` in `types.rs` gains `root_id: Option<String>`. Parsed in `events.rs` and propagated through `FeishuEvent::MessageReceive`.

## Section 6: Basic Card Interaction

**Event parsing** — `events.rs` handles `card.action.trigger`:

```rust
FeishuEvent::CardAction {
    chat_id: String,
    sender_id: String,
    action_value: String,  // button's value field becomes user message text
    message_id: String,
}
```

**Routing** — `websocket.rs` converts `CardAction` into a standard `InboundMessage`:

```rust
FeishuEvent::CardAction { chat_id, sender_id, action_value, message_id } => {
    let inbound = InboundMessage {
        text: action_value,
        conversation_id: ConversationId::new(&chat_id),
        sender_id: UserId::new(&sender_id),
        // ... same construction as regular messages
    };
    sender.send(inbound).await;
}
```

No versioned envelope protocol. No action validation. Button click = plain message. LLM handles intent.

## Section 7: Config Validation

In `config.rs`:

```rust
impl FeishuConfig {
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

Called in `FeishuChannel::new()`. Fail fast, before any network calls.

## Section 8: Cleanup Checklist

| Item | Action |
|------|--------|
| `client.rs` | **DELETE** — all code migrated to `auth.rs` + `api.rs` |
| `mod.rs` WS loop (lines 135-296) | **MOVE** to `websocket.rs` |
| `mod.rs` inline dedup (VecDeque, DEDUP_CAPACITY) | **REPLACE** with `dedup.rs` |
| `types.rs` FeishuConfig + defaults | **MOVE** to `config.rs` |
| `FeishuClient` struct | **REPLACED** by `TokenManager` + `FeishuApi` |
| `ws_reconnect_handle()` | **REPLACED** by `FeishuApi::refresh_ws_endpoint()` |
| All `FeishuClient` references in `streaming.rs` | **UPDATE** to `FeishuApi` |
| All `FeishuClient` references in `executor.rs` | **UPDATE** to `FeishuApi` |

## Testing Strategy

- **Existing tests preserved**: All unit tests in `events.rs`, `types.rs`, `mod.rs` migrate to their new homes
- **New tests**:
  - `config.rs`: validation edge cases (empty strings, invalid render_mode)
  - `dedup.rs`: capacity eviction, TTL expiry, duplicate detection
  - `user_cache.rs`: cache hit/miss/eviction/TTL
  - `message_ops.rs`: capabilities check, channel_id

## Config Example (After)

```json
{
  "type": "feishu",
  "app_id": "cli_xxx",
  "app_secret": "secret123",
  "domain": "feishu",
  "bot_name": "Aleph",
  "dm_allowed": true,
  "groups_allowed": true,
  "require_mention": true,
  "streaming": true,
  "render_mode": "auto",
  "typing_indicator": true,
  "group_session_scope": "group_topic"
}
```

## Not In Scope

- Webhook receiver (deferred — WebSocket sufficient for self-hosted)
- Multi-account support (use multiple `[[channels]]` entries)
- File attachment upload/download (future phase)
- Audio/video message handling (future phase)
- Read receipts (future phase)
- Complex card interaction protocol (R8 — LLM handles intent)
