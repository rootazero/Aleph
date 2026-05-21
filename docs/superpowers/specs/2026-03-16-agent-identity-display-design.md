# Agent Identity Display in Channel Responses

## Problem

When a bot has multiple agents, users interacting via Telegram/Slack/Discord etc. cannot tell which agent is currently responding. Previously this required asking "which agent are you?" via natural language. We need a passive, always-visible agent identity indicator.

## Decision Summary

- Use **platform-native identity** where the channel already shows agent context (e.g. webchat sidebar)
- **Fallback to `[AgentName]` prefix** for channels that don't support native identity display
- Agent identity comes from `AgentDefinition.name` (the agent's display name); if `None`, no identity is shown
- Always display (even with a single agent)

## Design

### 1. OutboundMessage Extension

Add `agent_display_name: Option<String>` to `OutboundMessage`:

```rust
pub struct OutboundMessage {
    // ... existing fields
    pub agent_display_name: Option<String>,
}
```

- `Some("交易助手")` — ReplyEmitter decides whether to prefix based on channel capability
- `None` — no identity shown (agent has no display name)

### 2. Interaction Capability: NativeIdentity

Extend the existing `thinker::interaction::Capability` enum with a new variant:

```rust
pub enum Capability {
    // ... existing variants (RichText, InlineButtons, Streaming, etc.)
    /// Channel natively displays agent identity (e.g. webchat sidebar)
    NativeIdentity,
}
```

This reuses the existing capability system. Channels already declare capabilities via `detect_capabilities()` → `HashSet<Capability>`. Webchat adds `NativeIdentity` to its set.

### 3. ReplyEmitter: Agent Name Threading & Prefix Logic

**Problem identified:** `ReplyEmitter` currently has no access to agent information. The agent display name must be threaded to it.

**Solution:** Add `agent_display_name: Option<String>` as a field on `ReplyEmitter`:

```rust
pub struct ReplyEmitter {
    // ... existing fields
    agent_display_name: Option<String>,
}
```

Construction sites that create `ReplyEmitter` (in `inbound_router/executor.rs` and `session_scheduler.rs`) already have access to the agent instance — they pass the agent display name at construction time.

**Prefix application is centralized in `ReplyEmitter::send_to_channel()`**, not in each channel implementation. This avoids N channels each needing to remember to call a utility function (P2 High Cohesion).

Logic in `send_to_channel()`:

```rust
async fn send_to_channel(&self, content: &str) {
    if content.is_empty() { return; }

    let is_first_send = !self.has_sent.load(Ordering::SeqCst);
    self.has_sent.store(true, Ordering::SeqCst);

    // Apply agent prefix only on first send, if channel lacks native identity
    let content = if is_first_send && !self.channel_has_native_identity() {
        apply_agent_prefix(content, &self.agent_display_name)
    } else {
        content.to_string()
    };

    let chunks = Self::split_message(&content, Self::MAX_MESSAGE_LENGTH);
    // ... rest of send logic, OutboundMessage carries agent_display_name
    // for channels that DO support native identity (future use)
}
```

`channel_has_native_identity()` checks if the target channel's capabilities include `NativeIdentity`, queried via `ChannelRegistry`.

### 4. Prefix Utility Function

A simple function in `channel.rs` (or `reply_emitter.rs`):

```rust
fn apply_agent_prefix(text: &str, agent_name: &Option<String>) -> String {
    match agent_name {
        Some(name) if !name.is_empty() => format!("[{}] {}", name, text),
        _ => text.to_string(),
    }
}
```

### 5. Streaming & Chunking Behavior

Two distinct scenarios where messages are sent in parts:

1. **Streaming chunks** (`stream_enabled: true`): `buffer_text()` auto-flushes when buffer exceeds threshold, calling `send_to_channel()` multiple times. The existing `has_sent: AtomicBool` gates prefix to the first flush only.

2. **Long message splitting** (`split_message()`): A single flush produces multiple `OutboundMessage` chunks. The prefix is already in the text before splitting, so only the first chunk naturally contains it.

Both scenarios are correctly handled by applying the prefix once in `send_to_channel()` before splitting, gated by `has_sent`.

### 6. Agent Switching Mid-Conversation

If a user switches agents mid-conversation, the `ReplyEmitter` for an in-flight request was created with the original agent's name. This is correct behavior — the response belongs to the agent that started processing it. The next response will use the new agent's name.

### 7. Data Flow

```
LLM Response
  ↓
StreamCallback (run_loop.rs)
  ↓
ReplyEmitter.send_to_channel():
  ├─ First send + channel lacks NativeIdentity → prepend "[Agent名] "
  ├─ First send + channel has NativeIdentity → no prefix (pass agent_display_name in OutboundMessage metadata)
  └─ Subsequent sends → no prefix
  ↓
ChannelRegistry.send()
  ↓
Channel.send() — sends OutboundMessage as-is
```

### 8. Files to Modify

| File | Change |
|------|--------|
| `src/gateway/channel.rs` | Add `agent_display_name` to `OutboundMessage` |
| `src/thinker/interaction.rs` | Add `NativeIdentity` variant to `Capability` enum |
| `src/gateway/reply_emitter.rs` | Add `agent_display_name` field; centralize prefix logic in `send_to_channel()`; add `channel_has_native_identity()` check |
| `src/gateway/inbound_router/executor.rs` | Pass agent display name when constructing `ReplyEmitter` |
| `src/gateway/session_scheduler.rs` | Pass agent display name when constructing `ReplyEmitter` |
| `src/gateway/interfaces/webchat/` | Add `NativeIdentity` to `detect_capabilities()` |

### 9. Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Display format (non-native) | `[AgentName] message` | Clean, unobtrusive, works in plain text |
| When to display | Always (even single agent) | Consistent behavior, no conditional logic |
| No display_name | Skip prefix entirely | Don't expose internal agent_id to users |
| Webchat handling | Native identity (no prefix) | Already shows agent via sidebar UI |
| Capability declaration | `Capability::NativeIdentity` enum variant | Reuses existing capability system (P3 Extensibility) |
| Prefix injection point | Centralized in `ReplyEmitter` | Single point of logic, not N channel impls (P2 High Cohesion) |
| First-send gating | Reuse existing `has_sent: AtomicBool` | No new state needed; handles both streaming and chunking |
| Agent name source | `AgentDefinition.name: Option<String>` | The user-facing display name field |
