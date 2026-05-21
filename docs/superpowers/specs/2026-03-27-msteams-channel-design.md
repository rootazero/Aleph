# Microsoft Teams Channel Design

## Summary

Add Microsoft Teams as a channel in Aleph's gateway, implementing the full Channel trait with native streaming (streaminfo protocol), AI UX best practices (welcome cards, AI labeling, status updates), and message operations (edit/delete). Pure HTTP implementation against Bot Framework REST API — no Microsoft SDK dependency.

## Context

Aleph supports 15+ messaging channels but lacks Microsoft Teams. OpenClaw's Teams integration (~17,355 lines TypeScript) provides a mature reference. This design adapts the best ideas while staying true to Aleph's Rust architecture and Channel abstraction.

Key decisions already made:
- **Pure HTTP** — direct Bot Framework REST API, consistent with Telegram/Discord implementations
- **WebhookReceiver** — reuse existing shared webhook server for inbound messages
- **Native streaming** — Teams streaminfo protocol (not edit-based typewriter)
- **Vault credentials** — app_id/app_password/tenant_id stored in Aleph Vault
- **Scope**: A (messaging) + B (streaming) + C (AI UX) + D (edit/delete). Graph API advanced features and feedback/reflection deferred.

## File Structure

```
src/gateway/interfaces/msteams/
├── mod.rs          # MsTeamsChannel: Channel + WebhookHandler implementation
├── config.rs       # MsTeamsConfig, MsTeamsChannelFactory
├── message_ops.rs  # MessageOperations trait implementation
├── auth.rs         # JWT validation, Bot token acquisition/refresh
├── api.rs          # Bot Framework REST API client
├── streaming.rs    # Teams streaminfo protocol handler
└── types.rs        # Bot Framework Activity/Entity types
```

## Part A: Core Messaging

### A1. Configuration

```rust
// config.rs
pub struct MsTeamsConfig {
    /// Azure Bot registration App ID
    pub app_id: String,
    /// Azure Bot registration App Password (client secret)
    pub app_password: String,
    /// Azure AD Tenant ID (optional, defaults to "common" for multi-tenant)
    pub tenant_id: Option<String>,
    /// Allowed user AAD IDs (empty = allow all)
    pub allowed_users: Vec<String>,
    /// Allow group/team messages
    pub groups_allowed: bool,
    /// Webhook path (default: "/msteams/messages")
    pub webhook_path: String,
    /// Send typing indicator while processing
    pub send_typing: bool,
    /// Maximum retries for failed messages
    pub max_retries: u32,
}
```

Factory creates `MsTeamsChannel` and registers its `WebhookHandler` with the shared `WebhookReceiver`.

### A2. Authentication (auth.rs)

**Inbound JWT validation:**
1. Fetch Microsoft OpenID metadata from `https://login.botframework.com/v1/.well-known/openidconfiguration`
2. Cache JWKS signing keys (refresh every 24h or on validation failure)
3. Validate JWT: issuer, audience (must match app_id), expiry, signature
4. Use `jsonwebtoken` crate for RS256 verification

**Outbound Bot token:**
1. POST to `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token` with client_credentials grant
2. Scope: `https://api.botframework.com/.default`
3. Cache token with TTL (typically 3600s, refresh at 80% lifetime)
4. Thread-safe: `tokio::sync::RwLock<CachedToken>`

```rust
pub struct TokenCache {
    token: RwLock<Option<CachedToken>>,
    app_id: String,
    app_password: String,
    tenant_id: String,
}

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

impl TokenCache {
    /// Get valid token, refreshing if needed
    pub async fn get_token(&self) -> Result<String, ChannelError>;
}
```

### A3. Bot Framework REST API (api.rs)

Thin HTTP client wrapping Bot Framework v3 endpoints:

```rust
pub struct BotFrameworkClient {
    http: reqwest::Client,
    token_cache: Arc<TokenCache>,
}

impl BotFrameworkClient {
    /// Send activity to conversation
    /// POST {serviceUrl}/v3/conversations/{conversationId}/activities
    pub async fn send_activity(&self, service_url: &str, conversation_id: &str,
                                activity: &Activity) -> Result<ActivityResponse, ChannelError>;

    /// Reply to activity
    /// POST {serviceUrl}/v3/conversations/{conversationId}/activities/{activityId}
    pub async fn reply_to_activity(&self, service_url: &str, conversation_id: &str,
                                    activity_id: &str, activity: &Activity) -> Result<ActivityResponse, ChannelError>;

    /// Update activity (edit message)
    /// PUT {serviceUrl}/v3/conversations/{conversationId}/activities/{activityId}
    pub async fn update_activity(&self, service_url: &str, conversation_id: &str,
                                  activity_id: &str, activity: &Activity) -> Result<(), ChannelError>;

    /// Delete activity
    /// DELETE {serviceUrl}/v3/conversations/{conversationId}/activities/{activityId}
    pub async fn delete_activity(&self, service_url: &str, conversation_id: &str,
                                  activity_id: &str) -> Result<(), ChannelError>;

    /// Send typing indicator
    /// POST {serviceUrl}/v3/conversations/{conversationId}/activities
    /// with type = "typing"
    pub async fn send_typing(&self, service_url: &str, conversation_id: &str) -> Result<(), ChannelError>;
}
```

### A4. Activity Types (types.rs)

Minimal structural types for Bot Framework protocol — no SDK dependency:

```rust
/// Bot Framework Activity (inbound and outbound)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(rename = "type")]
    pub activity_type: String,           // "message", "typing", "conversationUpdate", etc.
    pub id: Option<String>,
    pub timestamp: Option<String>,
    pub service_url: Option<String>,      // Per-message service URL (varies by region)
    pub channel_id: Option<String>,       // "msteams"
    pub from: Option<ChannelAccount>,
    pub conversation: Option<ConversationAccount>,
    pub recipient: Option<ChannelAccount>,
    pub text: Option<String>,
    pub text_format: Option<String>,      // "plain", "markdown", "xml"
    pub attachments: Option<Vec<ActivityAttachment>>,
    pub entities: Option<Vec<serde_json::Value>>,  // Flexible for streaminfo, AI entity, mentions
    pub reply_to_id: Option<String>,
    pub value: Option<serde_json::Value>, // Adaptive Card action payloads
    pub members_added: Option<Vec<ChannelAccount>>,
    pub members_removed: Option<Vec<ChannelAccount>>,
    pub channel_data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelAccount {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "aadObjectId")]
    pub aad_object_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationAccount {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "conversationType")]
    pub conversation_type: Option<String>,  // "personal", "groupChat", "channel"
    #[serde(rename = "tenantId")]
    pub tenant_id: Option<String>,
    #[serde(rename = "isGroup")]
    pub is_group: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityAttachment {
    pub content_type: String,
    pub content_url: Option<String>,
    pub content: Option<serde_json::Value>,
    pub name: Option<String>,
}

/// Response from sending an activity
#[derive(Debug, Clone, Deserialize)]
pub struct ActivityResponse {
    pub id: String,
}
```

### A5. Channel Implementation (mod.rs)

```rust
pub struct MsTeamsChannel {
    info: ChannelInfo,
    state: ChannelState,
    config: MsTeamsConfig,
    client: Arc<BotFrameworkClient>,
    /// serviceUrl → ConversationReference cache for proactive messaging
    conversation_refs: RwLock<HashMap<String, ConversationReference>>,
}

/// Cached conversation context for proactive messaging and edit/delete
struct ConversationReference {
    service_url: String,
    conversation_id: String,
    bot_id: String,
}
```

**Capabilities:**
```rust
ChannelCapabilities {
    attachments: true,
    images: true,
    audio: true,
    video: true,
    reactions: false,      // Teams reaction API limited, defer to Phase 2
    replies: true,
    editing: true,
    deletion: true,
    typing_indicator: true,
    read_receipts: false,
    rich_text: true,       // Teams supports HTML subset
    max_message_length: 28_000,  // Teams limit ~28KB
    max_attachment_size: 4_194_304,  // 4MB inline, larger needs OneDrive (Phase 2)
}
```

**WebhookHandler implementation:**
- `verify()`: JWT validation of `Authorization: Bearer {token}` header. Log specific JWT failure reasons via `tracing::warn!` before returning `false` (expired token, wrong audience, key rotation needed, etc.)
- `handle()`: Parse Activity JSON, route by activity_type:
  - `"message"` → convert to InboundMessage. **Strip `<at>...</at>` mention tags** from text in group chats before passing to the agent (otherwise every group message starts with `<at>BotName</at>`)
  - `"conversationUpdate"` + membersAdded → welcome card (Part C)
  - `"messageReaction"` → defer to Phase 2
  - Other → log and ignore
- `path()`: returns configured webhook_path (default `/msteams/messages`)
- **Access control**: Reject messages when `!groups_allowed` and `conversation.conversation_type` is `"groupChat"` or `"channel"`. Check `allowed_users` against sender's `aadObjectId`.

**Key difference from generic WebhookHandler**: Teams uses JWT Bearer token in Authorization header, not HMAC signature in a custom header. The `verify()` method delegates to `auth.rs` JWT validation instead of HMAC.

**Factory registration**: `MsTeamsChannelFactory` implements `ChannelFactory` trait and is registered in the server startup code alongside existing factories (Telegram, Discord, etc.). The factory reads `MsTeamsConfig` from the channel config JSON, validates credentials, creates `TokenCache` + `BotFrameworkClient`, and returns a `MsTeamsChannel` instance.

## Part B: Native Streaming (streaminfo protocol)

### B1. StreamStrategy Extension

Add a streaming protocol abstraction to ReplyEmitter:

```rust
// In channel.rs or a new streaming.rs at gateway level
pub enum StreamProtocol {
    /// No streaming — buffer and send on completion (default)
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
pub struct ChannelCapabilities {
    // ... existing fields ...
    /// Streaming protocol supported by this channel
    #[serde(default)]
    pub stream_protocol: StreamProtocol,
}
```

**Default is `None`**, not `EditBased`. Channels that support edit-based streaming (Telegram, Discord) explicitly set `EditBased` in their capabilities. This is honest about what each channel actually supports.

### B2. NativeStreamHandler Trait

```rust
// In channel.rs
#[async_trait]
pub trait NativeStreamHandler: Send + Sync {
    /// Start streaming — send initial status/typing indicator
    /// Returns a stream_id for subsequent updates
    async fn stream_start(&self, conversation_id: &ConversationId,
                           status_text: &str) -> ChannelResult<String>;

    /// Send a streaming chunk (accumulated text, must be prefix of final)
    async fn stream_update(&self, conversation_id: &ConversationId,
                            stream_id: &str, text: &str, sequence: u32) -> ChannelResult<()>;

    /// Finalize the stream — send the complete message
    async fn stream_finalize(&self, conversation_id: &ConversationId,
                              stream_id: &str, message: OutboundMessage) -> ChannelResult<SendResult>;
}
```

### B3. Teams streaminfo Implementation (streaming.rs)

```rust
impl NativeStreamHandler for MsTeamsChannel {
    async fn stream_start(&self, conversation_id: &ConversationId,
                           status_text: &str) -> ChannelResult<String> {
        // Send typing activity with streaminfo entity (streamType: "informative")
        // Returns: activity ID as stream_id
        let activity = Activity {
            activity_type: "typing".into(),
            text: Some(status_text.into()),
            entities: Some(vec![build_stream_info_entity(None, "informative", 0)]),
            ..Default::default()
        };
        let resp = self.send_to_conversation(conversation_id, &activity).await?;
        Ok(resp.id)
    }

    async fn stream_update(&self, conversation_id: &ConversationId,
                            stream_id: &str, text: &str, sequence: u32) -> ChannelResult<()> {
        // Send typing activity with streaminfo entity (streamType: "streaming")
        // Throttle: >=1500ms between updates (Teams rate limit)
        let activity = Activity {
            activity_type: "typing".into(),
            text: Some(text.into()),
            entities: Some(vec![build_stream_info_entity(Some(stream_id), "streaming", sequence)]),
            ..Default::default()
        };
        self.send_to_conversation(conversation_id, &activity).await?;
        Ok(())
    }

    async fn stream_finalize(&self, conversation_id: &ConversationId,
                              stream_id: &str, message: OutboundMessage) -> ChannelResult<SendResult> {
        // Send message activity with streaminfo entity (streamType: "final")
        // + AI-generated entity (Part C)
        let mut activity = self.build_outbound_activity(&message);
        let mut entities = activity.entities.unwrap_or_default();
        entities.push(build_stream_info_entity(Some(stream_id), "final", 0));
        entities.push(build_ai_generated_entity());
        activity.entities = Some(entities);

        let resp = self.send_to_conversation(conversation_id, &activity).await?;
        Ok(SendResult {
            message_id: MessageId::new(&resp.id),
            timestamp: Utc::now(),
        })
    }
}

fn build_stream_info_entity(stream_id: Option<&str>, stream_type: &str, sequence: u32) -> serde_json::Value {
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
```

### B4. ReplyEmitter Integration

The actual ReplyEmitter flow (from `reply_emitter.rs`):

1. **`StreamEvent::ResponseChunk`** — non-intermediate chunks are buffered via `self.buffer.lock().await.push_str(&content)`
   - In instant mode (`!stream_enabled`): flushes buffer on `is_final`
   - In typewriter mode (`stream_enabled`): does nothing, waits for `RunComplete`
2. **`StreamEvent::RunComplete`** — flushes buffer, sends via `send_typewriter()` (typewriter) or `send_to_channel()` (instant)

For native streaming, the changes are:

**New field on ReplyEmitter:**
```rust
/// Native stream handler from the channel (if StreamProtocol::Native)
native_handler: Option<Arc<dyn NativeStreamHandler>>,
/// Active stream state (stream_id, sequence counter, last_update_time)
native_stream_state: Mutex<Option<NativeStreamState>>,
```

`ReplyEmitter::with_config()` resolves the handler via `ChannelRegistry::get_native_stream_handler(channel_id)`.

**In `StreamEvent::ResponseChunk` handler** (non-intermediate path, after buffering):
```rust
// After: self.buffer.lock().await.push_str(&content);
if let Some(ref handler) = self.native_handler {
    let buffer = self.buffer.lock().await;
    let accumulated = buffer.clone();
    drop(buffer);

    let mut state = self.native_stream_state.lock().await;
    if state.is_none() && accumulated.chars().count() >= 20 {
        // First chunk: send informative status, then first streaming update
        let status = pick_status_text();
        match handler.stream_start(&self.route.conversation_id, &status).await {
            Ok(stream_id) => {
                *state = Some(NativeStreamState {
                    stream_id,
                    sequence: 0,
                    last_update: Instant::now(),
                });
            }
            Err(e) => {
                warn!("Native stream_start failed, falling back: {}", e);
                self.native_handler = None; // Disable, fall through to normal path
            }
        }
    } else if let Some(ref mut s) = *state {
        // Subsequent chunks: throttle at 1500ms
        if s.last_update.elapsed() >= Duration::from_millis(1500) {
            s.sequence += 1;
            let _ = handler.stream_update(
                &self.route.conversation_id, &s.stream_id, &accumulated, s.sequence
            ).await;
            s.last_update = Instant::now();
        }
    }
}
```

**In `StreamEvent::RunComplete` handler** (before the existing flush logic):
```rust
// Finalize native stream if active
if let Some(ref handler) = self.native_handler {
    let state = self.native_stream_state.lock().await.take();
    if let Some(s) = state {
        let text = {
            let mut buffer = self.buffer.lock().await;
            std::mem::take(&mut *buffer)
        };
        if !text.is_empty() {
            let message = OutboundMessage::text(
                self.route.conversation_id.as_str(), &text
            );
            match handler.stream_finalize(
                &self.route.conversation_id, &s.stream_id, message
            ).await {
                Ok(_) => {
                    self.has_sent.store(true, Ordering::SeqCst);
                    return Ok(()); // Skip normal send path
                }
                Err(e) => {
                    warn!("Native stream_finalize failed, falling back: {}", e);
                    // Re-fill buffer so normal path can send it
                    *self.buffer.lock().await = text;
                }
            }
        }
    }
}
// ... existing RunComplete flush logic unchanged ...
```

**Key design decisions:**
- Native streaming happens **during `ResponseChunk`** events (real-time), unlike typewriter which replays **after `RunComplete`**
- `stream_finalize` in `RunComplete` sends the final message with AI entity
- Any failure in native streaming falls back gracefully to the existing instant/typewriter path
- The `native_handler` field is set to `None` on failure to prevent further attempts
- 1500ms throttle is enforced via `Instant::now()` comparison
- Min 20 chars before first `stream_start` to avoid flicker

## Part C: AI UX Enhancements

### C1. Welcome Card

On `conversationUpdate` activity with bot in `membersAdded`:

```rust
fn build_welcome_card(bot_name: &str, prompt_starters: &[&str]) -> serde_json::Value {
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
```

Prompt starters are derived from the active agent's configured greeting prompts, falling back to sensible defaults.

### C2. AI-Generated Content Label

All outbound bot messages include:

```rust
fn build_ai_generated_entity() -> serde_json::Value {
    serde_json::json!({
        "type": "https://schema.org/Message",
        "@type": "Message",
        "@id": "",
        "additionalType": ["AIGeneratedContent"]
    })
}
```

This causes Teams to display an "AI" badge on the message — zero effort, native UX.

### C3. Informative Status Updates

Before streaming starts, send an "informative" streaminfo update with randomized status text:

```rust
const STATUS_TEXTS: &[&str] = &[
    "Thinking...",
    "Working on that...",
    "Checking the details...",
    "Putting an answer together...",
];
```

Rendered as a blue progress bar in Teams client.

## Part D: Message Edit & Delete

### D1. Conversation Reference Cache

Every inbound Activity carries a `serviceUrl` (varies by Azure region and conversation). Cache this for proactive messaging:

```rust
struct ConversationReference {
    service_url: String,
    conversation_id: String,
    bot_id: String,
    last_seen: Instant,  // For LRU eviction
}

// On every inbound message:
self.conversation_refs.write().await.insert(
    conversation_id.clone(),
    ConversationReference {
        service_url: activity.service_url.clone(),
        conversation_id: activity.conversation.id.clone(),
        bot_id: activity.recipient.id.clone(),
        last_seen: Instant::now(),
    },
);
```

**Eviction**: Capped at 10,000 entries. On insert, if at capacity, evict the entry with the oldest `last_seen`. This is sufficient for single-instance self-hosted deployment.

### D2. MessageOperations Implementation (message_ops.rs)

Uses the `MessageOperations` trait from `builtin_tools/message/types.rs`. Note: this trait has its own `ChannelCapabilities` struct (separate from `gateway::channel::ChannelCapabilities`) with boolean fields for `reply`, `edit`, `react`, `delete`, `send`.

```rust
use crate::builtin_tools::message::types::{
    MessageOperations, ChannelCapabilities as MsgCapabilities,
    ReplyParams, EditParams, ReactParams, DeleteParams, SendParams, MessageResult,
};

pub struct MsTeamsMessageOps {
    client: Arc<BotFrameworkClient>,
    conversation_refs: Arc<RwLock<HashMap<String, ConversationReference>>>,
}

#[async_trait]
impl MessageOperations for MsTeamsMessageOps {
    fn channel_id(&self) -> &str { "msteams" }

    fn capabilities(&self) -> MsgCapabilities {
        MsgCapabilities {
            reply: true,
            edit: true,
            react: false,  // Defer to Phase 2
            delete: true,
            send: true,
        }
    }

    async fn reply(&self, params: ReplyParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.conversation_id)?;
        let mut activity = Activity::text_message(&params.text);
        activity.reply_to_id = Some(params.message_id.clone());
        inject_ai_entity(&mut activity);

        let resp = self.client.reply_to_activity(
            &conv_ref.service_url,
            &conv_ref.conversation_id,
            &params.message_id,
            &activity,
        ).await?;
        Ok(MessageResult::success_with_id(&resp.id))
    }

    async fn edit(&self, params: EditParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.conversation_id)?;
        self.client.update_activity(
            &conv_ref.service_url,
            &conv_ref.conversation_id,
            &params.message_id,
            &Activity {
                activity_type: "message".into(),
                text: Some(params.text.clone()),
                ..Default::default()
            },
        ).await?;
        Ok(MessageResult::success_with_id(&params.message_id))
    }

    async fn react(&self, _params: ReactParams) -> Result<MessageResult> {
        Ok(MessageResult::failed("Reactions not supported for Teams in Phase 1"))
    }

    async fn delete(&self, params: DeleteParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.conversation_id)?;
        self.client.delete_activity(
            &conv_ref.service_url,
            &conv_ref.conversation_id,
            &params.message_id,
        ).await?;
        Ok(MessageResult::success())
    }

    async fn send(&self, params: SendParams) -> Result<MessageResult> {
        let conv_ref = self.get_conversation_ref(&params.target)?;
        let mut activity = Activity::text_message(&params.text);
        inject_ai_entity(&mut activity);

        let resp = self.client.send_activity(
            &conv_ref.service_url,
            &conv_ref.conversation_id,
            &activity,
        ).await?;
        Ok(MessageResult::success_with_id(&resp.id))
    }
}
```

## Changes to Existing Code

### ChannelCapabilities (channel.rs)

Add one field:
```rust
pub struct ChannelCapabilities {
    // ... existing fields unchanged ...
    /// Streaming protocol: EditBased (default), Native, or None
    #[serde(default)]
    pub stream_protocol: StreamProtocol,
}
```

`StreamProtocol` defaults to `None`. Channels that support edit-based streaming (Telegram, Discord) must explicitly set `EditBased` in their capabilities. This is a minor change to Telegram/Discord config files.

### ReplyEmitter (reply_emitter.rs)

Add `native_handler: Option<Arc<dyn NativeStreamHandler>>` and `native_stream_state` fields. Native streaming path activates during `ResponseChunk` events when handler is present. Existing typewriter and instant paths are untouched. See B4 for detailed integration.

### Channel trait (channel.rs)

Add optional `NativeStreamHandler` accessor:
```rust
/// Get native stream handler (if channel supports StreamProtocol::Native)
fn native_stream_handler(&self) -> Option<&dyn NativeStreamHandler> {
    None
}
```

Default returns `None`. Only MsTeamsChannel overrides this.

### WebhookReceiver

No changes needed. Teams channel registers its `WebhookHandler` implementation which uses JWT validation in `verify()` instead of HMAC. The `WebhookHandler` trait is flexible enough — `verify()` receives full headers and body.

## Security Considerations

1. **JWT validation**: Every inbound webhook verified against Microsoft's signing keys. Keys cached with TTL refresh.
2. **Credential storage**: app_id/app_password in Vault, never logged or exposed.
3. **serviceUrl validation**: Only accept serviceUrls matching known Bot Framework domains (`*.botframework.com`, `smba.trafficmanager.net`).
4. **Rate limiting**: Respect Teams' 1 req/sec for streaming updates. Built-in throttle in stream handler.
5. **Conversation ID safety**: Teams uses colons in conversation IDs — ensure file-system operations (if any) sanitize these.

## Testing Strategy

1. **Unit tests**: JWT validation with test keys, Activity parsing, streaminfo entity construction, welcome card generation
2. **Integration tests**: WebhookHandler with mock HTTP requests (reuse existing webhook_receiver test pattern)
3. **Manual testing**: Azure Bot registration + ngrok tunnel for end-to-end verification

## What We Learn from OpenClaw, What We Do Better

| OpenClaw Pattern | Aleph Adaptation |
|-----------------|-----------------|
| 60+ files, 17K lines TypeScript | ~7 files, ~2K lines Rust — same functionality, type safety at compile time |
| SDK abstraction layer (structural types) | No SDK needed — direct REST API is simpler and equally decoupled |
| Lazy SDK loading | N/A — no SDK to load |
| Separate streaming class | Integrated into Channel trait via NativeStreamHandler — reusable for future platforms (Slack, etc.) |
| File-backed conversation store | In-memory HashMap with RwLock — sufficient for single-instance self-hosted |
| Separate feedback-reflection system | Deferred to Phase 2 — Aleph's existing session memory handles learning |
| Graph API integration | Deferred to Phase 2 — core messaging doesn't need it |
| Complex target resolution (user:, conversation:, raw ID) | ConversationId is already resolved by Channel abstraction |

## Out of Scope (Phase 2)

- Graph API operations (search, pin, file upload to OneDrive/SharePoint)
- Feedback/reflection system (thumbs-down → LLM reflection)
- Reactions (Teams reaction API has limited semantics)
- Adaptive Card action handling beyond welcome card prompt starters
- Multi-tenant deployment (single bot registration per Aleph instance)
- File consent flow for large attachments (>4MB)
