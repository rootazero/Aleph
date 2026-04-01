# Feishu Channel Enhanced Features Design

**Date**: 2026-03-22
**Status**: Draft
**Scope**: Add streaming cards, markdown rendering, and typing indicators to the existing Feishu channel

## Summary

Enhance the Feishu channel from MVP (text+image, instant delivery) to a polished experience with three features: streaming card output (real-time response display), markdown rendering (code blocks, tables, lists), and typing indicators (emoji reaction feedback). All three use existing Feishu APIs. The streaming feature requires a Feishu-specific `EventEmitter` implementation to handle real-time chunk delivery, since the standard `ReplyEmitter`'s typewriter mode is a post-hoc animation that buffers all content first.

## Feature 1: Streaming Card Output

### Mechanism

Use Feishu Card Kit v1 API (schema 2.0) to create a card with `streaming_mode: true`, then incrementally update its content as the agent produces text chunks.

### Flow

```
Agent starts processing
  ↓
1. POST /open-apis/cardkit/v1/cards
   → Create card with streaming_mode=true, initial content "⏳ Thinking..."
   → Returns card_id
  ↓
2. Send card message to chat
   → POST /open-apis/im/v1/messages (msg_type="interactive", content={type:"card", data:{card_id}})
   → Returns message_id
  ↓
3. Agent produces ResponseChunk events
   → PUT /open-apis/cardkit/v1/cards/{cardId}/elements/content/content
   → Body: { content: "full accumulated text", sequence: N, uuid: "s_{cardId}_{N}" }
   → Throttled: min 100ms between updates
  ↓
4. Agent completes (RunComplete)
   → Final content update with complete text
   → PATCH /open-apis/cardkit/v1/cards/{cardId}/settings
   → Body: { settings: {config:{streaming_mode:false, summary:{content:"first 50 chars..."}}}, sequence: N+1, uuid: "c_{cardId}_{N+1}" }
```

### Card Creation Payload

```json
{
  "type": "card_json",
  "data": "{\"schema\":\"2.0\",\"config\":{\"streaming_mode\":true,\"summary\":{\"content\":\"[Generating...]\"},\"streaming_config\":{\"print_frequency_ms\":{\"default\":50},\"print_step\":{\"default\":1}}},\"body\":{\"elements\":[{\"tag\":\"markdown\",\"content\":\"⏳ Thinking...\",\"element_id\":\"content\"}]}}"
}
```

### Architecture: FeishuEventEmitter

The standard `ReplyEmitter` buffers all `ResponseChunk` events and only sends/edits after `RunComplete` (post-hoc typewriter animation). For real-time streaming cards, we need a Feishu-specific `EventEmitter` that processes chunks as they arrive.

**New file**: `src/gateway/interfaces/feishu/streaming.rs`

Two structs:

```rust
/// Manages a single streaming card lifecycle
pub struct FeishuStreamingCard {
    card_id: String,
    message_id: String,
    sequence: AtomicU32,
    accumulated_text: Mutex<String>,
    last_update: Mutex<Instant>,
    closed: AtomicBool,
}

/// EventEmitter that streams to Feishu cards in real-time,
/// falling back to standard ReplyEmitter behavior for non-streaming.
pub struct FeishuEventEmitter {
    /// Standard reply emitter (used for instant mode and non-text events)
    inner: ReplyEmitter,
    /// Feishu client for Card Kit API calls
    client: Arc<FeishuClient>,
    /// Active streaming card (None if streaming disabled or not yet created)
    card: Arc<Mutex<Option<FeishuStreamingCard>>>,
    /// Route info for sending the initial card message
    route: ReplyRoute,
    /// Whether streaming is enabled
    streaming_enabled: bool,
    /// Typing indicator state
    typing_state: Arc<StdMutex<Option<TypingState>>>,
}
```

**FeishuEventEmitter::emit() flow**:

```
ResponseChunk arrives:
  ├─ streaming disabled → delegate to inner ReplyEmitter
  └─ streaming enabled:
      ├─ card not yet created → create card, send to chat, store card_id
      ├─ card exists → accumulate text, throttled PUT to card element (100ms min)
      └─ is_final → close card (disable streaming_mode)

RunComplete arrives:
  ├─ streaming card active → ensure card is closed with final text
  └─ no card → delegate to inner ReplyEmitter (instant send)
  └─ remove typing indicator if active
```

**Integration point**: The `FeishuEventEmitter` is constructed in `ExecutionEngine` (or wherever `ReplyEmitter` is created for a run) when the channel is of type "feishu" and streaming is enabled. It wraps the standard `ReplyEmitter`, forwarding non-streaming events while intercepting `ResponseChunk` for real-time card updates.

**Why not Channel::edit()**: The `edit()` signature takes `MessageId`, but streaming card updates need `card_id` + `sequence` + `uuid`. The card state is ephemeral per-response, not per-message. A dedicated emitter is cleaner than overloading `edit()` with Feishu-specific semantics.

### Throttling

- Minimum 100ms between updates (max 10 updates/sec)
- Pending text accumulated; only latest state sent on next tick
- Uses `tokio::time::Instant` to track last update time

### Configuration

`FeishuConfig` new field:
```rust
#[serde(default = "default_true")]
pub streaming: bool,  // default: true
```

## Feature 2: Markdown Rendering

### Output Mode Decision

AI responses are naturally markdown. The channel decides how to render based on content:

| Condition | Output Method |
|-----------|---------------|
| streaming=true (Feature 1 active) | Streaming card — markdown rendered automatically |
| Text contains `` ``` `` or `\|---\|` table syntax | Static card (msg_type="interactive") |
| Plain short text | Text message (current behavior) |

### Static Card Payload

For non-streaming responses that contain code/tables:

```json
{
  "schema": "2.0",
  "config": { "wide_screen_mode": true },
  "body": {
    "elements": [
      { "tag": "markdown", "content": "AI response with ```code``` blocks" }
    ]
  }
}
```

No header or footer — Aleph is a personal assistant, no need for agent name/model metadata.

### Implementation

- `client.rs`: new `send_card(chat_id, markdown_text, reply_to)` method
- `mod.rs`: in `send()`, choose send method based on render_mode:
  ```rust
  fn should_use_card(text: &str, render_mode: &str) -> bool {
      match render_mode {
          "card" => true,
          "raw" => false,
          _ => text.len() > 200 || text.contains("```") || text.contains("|---|") || text.contains("|:--"),
      }
  }
  ```
- When streaming is active, all output goes through streaming card (Feature 1), so this check only applies to non-streaming (instant) mode
- Default render_mode is "auto" — uses card for longer/richer text, plain text for short replies

### Configuration

`FeishuConfig` new field:
```rust
#[serde(default = "default_render_mode")]
pub render_mode: String,  // "auto" | "card" | "raw", default: "auto"
```

- `"auto"`: Use card when content has code/tables, text otherwise
- `"card"`: Always use card (all responses get markdown rendering)
- `"raw"`: Always use plain text (current MVP behavior)

## Feature 3: Typing Indicator

### Mechanism

Add a "Typing" emoji reaction to the user's message when the agent starts processing. Remove it when the response is sent or an error occurs.

### API Calls

**Add reaction**:
```
POST /open-apis/im/v1/messages/{message_id}/reactions
Authorization: Bearer {token}
Body: { "reaction_type": { "emoji_type": "Typing" } }
Response: { "code": 0, "data": { "reaction_id": "r_xxxxx" } }
```

**Remove reaction**:
```
DELETE /open-apis/im/v1/messages/{message_id}/reactions/{reaction_id}
Authorization: Bearer {token}
```

### Lifecycle

```
FeishuEventEmitter receives first ResponseChunk:
  → add_reaction("Typing") on inbound message_id (from ReplyRoute)
  → save reaction_id in typing_state

FeishuEventEmitter receives RunComplete / RunError:
  → remove_reaction(reaction_id)
  → clear typing_state
```

The typing indicator is managed by `FeishuEventEmitter`, not `Channel::send_typing()`. The emitter handles both the response streaming and typing feedback in one place.

### Implementation

- `client.rs`: new `add_reaction(message_id, emoji_type)` and `remove_reaction(message_id, reaction_id)` methods
- `types.rs`: new `ReactionResponse` type
- `streaming.rs`: `FeishuEventEmitter` manages typing via `Arc<StdMutex<Option<TypingState>>>` where `TypingState { message_id: String, reaction_id: String }`
- `ChannelCapabilities`: set `typing_indicator = true`, `reactions = true`, `editing = true` (for streaming), `rich_text = true`
- `ChannelProvider::interaction_manifest()`: set `supports_streaming` conditional on `self.config.streaming`

### Error Handling

- All reaction API failures silently ignored (non-critical)
- Message already withdrawn → API returns error → caught and ignored
- Rate limited → skip typing indicator, don't block main flow

### Configuration

`FeishuConfig` new field:
```rust
#[serde(default = "default_true")]
pub typing_indicator: bool,  // default: true
```

### Required Permission

`im:message.reaction:write_as_bot` — must be added in Feishu Open Platform app configuration.

## New Files

| File | Purpose |
|------|---------|
| `src/gateway/interfaces/feishu/streaming.rs` | FeishuStreamingCard + FeishuEventEmitter |

## Modified Files

| File | Changes |
|------|---------|
| `src/gateway/interfaces/feishu/types.rs` | Add config fields (streaming, render_mode, typing_indicator), ReactionResponse type, card payload types |
| `src/gateway/interfaces/feishu/client.rs` | Add send_card(), add_reaction(), remove_reaction(), create_streaming_card(), update_streaming_card(), close_streaming_card() |
| `src/gateway/interfaces/feishu/mod.rs` | Update send() for card/markdown logic; update capabilities (editing, typing_indicator, rich_text); update InteractionManifest (supports_streaming conditional on config.streaming) |
| `interfaces/webchat/src/views/settings/channels/definitions.rs` | Add streaming, render_mode, typing_indicator fields to FEISHU_FIELDS |
| Integration site where ReplyEmitter is constructed | Use FeishuEventEmitter instead of ReplyEmitter when channel is feishu + streaming enabled |

## No Changes To

- Channel trait, ChannelRegistry, InboundMessageRouter, ReplyEmitter
- Other channel implementations

## Error Handling Summary

| Scenario | Strategy |
|----------|----------|
| Card creation fails | Fall back to instant text message |
| Streaming update fails | Log warning, retry on next chunk |
| Card close fails | Log warning, card remains in streaming state (auto-closes client-side) |
| Reaction add fails | Silently ignore |
| Reaction remove fails | Silently ignore |
| Rate limited on card API | Back off, skip updates until next chunk |

## Test Plan

- Unit tests for `should_use_card()` detection (code blocks, tables, plain text)
- Unit tests for streaming card sequence/uuid generation
- Unit tests for text accumulation and throttle logic
- Unit tests for new config field deserialization (streaming, render_mode, typing_indicator)
- Integration test: card creation payload serialization
- Manual test: streaming output in real Feishu chat
- Manual test: markdown rendering (code block, table, list)
- Manual test: typing indicator appears and disappears
