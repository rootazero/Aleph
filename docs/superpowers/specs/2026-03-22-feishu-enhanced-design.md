# Feishu Channel Enhanced Features Design

**Date**: 2026-03-22
**Status**: Draft
**Scope**: Add streaming cards, markdown rendering, and typing indicators to the existing Feishu channel

## Summary

Enhance the Feishu channel from MVP (text+image, instant delivery) to a polished experience with three features: streaming card output (real-time response display), markdown rendering (code blocks, tables, lists), and typing indicators (emoji reaction feedback). All three use existing Feishu APIs and require no changes to Aleph's core Channel trait.

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

### Implementation

**New file**: `core/src/gateway/interfaces/feishu/streaming.rs`

```rust
pub struct FeishuStreamingCard {
    card_id: String,
    message_id: String,
    sequence: u32,
    accumulated_text: String,
    last_update: Instant,
    closed: bool,
}
```

Key methods:
- `create(client, chat_id, reply_to) → Self` — create card + send to chat
- `update(client, text_chunk)` — accumulate text, throttled PUT to card element
- `close(client, final_text)` — final update + disable streaming_mode

**Integration with FeishuChannel**:
- Implement `Channel::edit()` to update existing streaming card
- On first `send()` when streaming is enabled: create streaming card, store in channel state
- On subsequent `edit()` calls: update card content via streaming API
- On final `send()` with `is_final`: close streaming card

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
- `mod.rs`: in `send()`, check content before choosing send method:
  ```rust
  fn should_use_card(text: &str) -> bool {
      text.contains("```") || text.contains("|---|")
  }
  ```
- When streaming is active, all output goes through streaming card (Feature 1), so this check only applies to non-streaming mode

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
Inbound message received → add_reaction("Typing") → save reaction_id
  ↓
Agent processing...
  ↓
Response sent / error → remove_reaction(reaction_id)
```

### Implementation

- `client.rs`: new `add_reaction(message_id, emoji_type)` and `remove_reaction(message_id, reaction_id)` methods
- `types.rs`: new `ReactionResponse` type
- `mod.rs`: implement `Channel::send_typing()` — add typing reaction, store state for later removal
- `ChannelCapabilities.typing_indicator = true`
- Typing state stored per-conversation: `HashMap<ConversationId, TypingState>` with `{ message_id, reaction_id }`

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
| `core/src/gateway/interfaces/feishu/streaming.rs` | FeishuStreamingCard: create, update, close |

## Modified Files

| File | Changes |
|------|---------|
| `core/src/gateway/interfaces/feishu/types.rs` | Add config fields (streaming, render_mode, typing_indicator), ReactionResponse type |
| `core/src/gateway/interfaces/feishu/client.rs` | Add send_card(), add_reaction(), remove_reaction(), create_streaming_card(), update_streaming_card(), close_streaming_card() |
| `core/src/gateway/interfaces/feishu/mod.rs` | Implement edit(), send_typing(); update send() for card/streaming logic; typing state management |
| `interfaces/webchat/src/views/settings/channels/definitions.rs` | Add streaming, render_mode, typing_indicator fields to FEISHU_FIELDS |

## No Changes To

- Channel trait, ChannelRegistry, InboundMessageRouter, ReplyEmitter
- Other channel implementations
- Backend API handlers

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
