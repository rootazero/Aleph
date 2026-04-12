---
date: 2026-04-05
topic: unified-typed-channel-event-bus
status: in-progress
---

# Unified Typed Channel Event Bus

## Problem Frame

### Current State

`GatewayEventBus` uses `broadcast::Sender<String>` — a **fire-and-forget, untyped channel**. All events are serialized to JSON strings before broadcasting. Subscribers receive `String` and must re-deserialize with no compile-time guarantee that the topic name matches the data shape.

```rust
// Current API — type-safe publish, type-unsafe subscribe
pub fn publish_json<T: serde::Serialize>(&self, event: &T) -> Result<usize, serde_json::Error>
pub fn subscribe(&self) -> broadcast::Receiver<String>  // String — no type info
```

`TopicEvent` uses `topic: String` and `data: Value` — arbitrary strings and arbitrary JSON:
```rust
pub struct TopicEvent {
    pub topic: String,
    pub data: Value,   // no guaranteed shape
    pub timestamp: u64,
}
```

`StreamEvent` in `event_emitter/types.rs` (~18 variants) is typed, but loses that type safety at the serialization boundary.

### Channel Events Are Second-Class

Channel interfaces (Telegram, Discord, etc.) produce `InboundMessage` that flows through `InboundMessageRouter`. Channel-level events (connection status, errors, typing indicators) are **not published to the event bus** — they exist only as internal state transitions.

OpenClaw's channel plugin system integrates channel events into the global event bus via `emitAgentEvent()`. Aleph should do the same.

### No Schema Validation

Incoming RPC frames are deserialized directly into typed structs via `serde_json`. Malformed frames are silently accepted (unknown fields ignored). No schema validation exists at the protocol boundary.

### Problems This Causes

1. **Runtime panics**: Subscriber deserializes JSON with wrong expected type → panic
2. **Silent data loss**: Unknown fields silently dropped by serde
3. **Channel opacity**: Channel status changes invisible to external event bus consumers
4. **No compile-time topic/data contract**: Topic strings like `"agent.run.started"` are untyped
5. **Security gap**: Malformed config patches or agent params not rejected at boundary

---

## Requirements

### Type-Safe Event Envelope

- **R1**: Replace `broadcast::Sender<String>` with `broadcast::Sender<GatewayEventFrame>` where `GatewayEventFrame` is an **enum with typed variants**

- **R2**: `GatewayEventFrame` variants are exhaustive — at minimum:
  ```rust
  pub enum GatewayEventFrame {
      // Agent events
      AgentRunStarted { run_id: RunId, session_id: SessionId, seq: u64 },
      AgentRunCompleted { run_id: RunId, session_id: SessionId, seq: u64, outcome: RunOutcome },
      AgentReasoning { run_id: RunId, chunk: String, seq: u64 },
      AgentToolStart { run_id: RunId, tool_name: ToolName, seq: u64 },
      AgentToolEnd { run_id: RunId, tool_name: ToolName, seq: u64, success: bool },
      AgentError { run_id: RunId, error: String, seq: u64 },

      // Channel events
      ChannelMessage { channel_id: ChannelId, conversation_id: ConversationId, message: InboundMessage },
      ChannelTyping { channel_id: ChannelId, conversation_id: ConversationId },
      ChannelStatusChanged { channel_id: ChannelId, status: ChannelStatus },
      ChannelError { channel_id: ChannelId, error: String },

      // Session events
      SessionCreated { session_id: SessionId },
      SessionUpdated { session_id: SessionId },
      SessionDeleted { session_id: SessionId },

      // Protocol events
      ConfigChanged { section: Option<String>, value: Value },
      PairingRequested { device_name: String },
      PairingCompleted { device_id: DeviceId },
  }
  ```

- **R3**: `#[serde(tag = "type")]` on `GatewayEventFrame` so it serializes to JSON with `"type": "AgentRunStarted"` field — **identical to current WebSocket wire format**

- **R4**: Subscribers use `broadcast::Receiver<GatewayEventFrame>` and pattern-match on enum variants — no re-deserialization from String

- **R5**: `session_id: Option<SessionId>` on agent and session event variants for session-scoped delivery

### Channel Event Integration

- **R6**: Channel interfaces emit typed `GatewayEventFrame` variants for:
  - Inbound messages (channel → agent)
  - Typing indicators
  - Connection status changes
  - Errors

- **R7**: `InboundMessageRouter` dispatches to `GatewayEventBus::publish()` with typed `GatewayEventFrame::ChannelMessage { ... }` — no String serialization at router level

- **R8**: Channel health events flow through the same typed bus as agent events — unified event model

### Schema Validation Layer

- **R9**: All incoming JSON-RPC params validated against JSON Schema before handler processing using `schemars`

- **R10**: Validation failure returns JSON-RPC `InvalidParams` error:
  ```json
  {
    "jsonrpc": "2.0",
    "error": {
      "code": -32602,
      "message": "Invalid params: property 'run_id' is required",
      "data": { "path": "params.run_id", "reason": "required property missing" }
    },
    "id": null
  }
  ```

- **R11**: Handler params types use `schemars::JsonSchema` derive to auto-generate schemas — no manual schema authoring

- **R12**: Config patches (`ConfigPatchParams`), session patches (`SessionsPatchParams`), and agent params (`AgentParams`) are validated

### Backward Compatibility

- **R13**: `GatewayEventBus` still implements `Clone` and `publish_json()` for legacy emitters during transition period
- **R14**: WebSocket wire format unchanged — `#[serde(tag = "type")]` produces identical JSON shape
- **R15**: Existing `TopicFilter::matches()` works by matching enum variant names (as strings)

### Topic/Event Naming Convention

- **R16**: All topic strings follow `category.subcategory.action` pattern:
  - `agent.run.started` → `GatewayEventFrame::AgentRunStarted`
  - `channel.message.received` → `GatewayEventFrame::ChannelMessage`
  - `session.created` → `GatewayEventFrame::SessionCreated`

- **R17**: Topic string is derivable from enum variant name: `AgentRunStarted` → `"agent.run.started"`

---

## Scope Boundaries

### In Scope
- `GatewayEventBus` type safety
- Channel event integration
- Schema validation at protocol boundary
- WebSocket backward compatibility

### Out of Scope (for this document)
- Multi-device ACP / node registry (separate design doc)
- Exec approval system (separate design doc)
- Binary protocol (MessagePack) — keep JSON for Phase 1
- Changes to `InboundMessageRouter` routing logic (only event emission changes)

---

## Success Criteria

- [ ] `GatewayEventBus::publish()` accepts `&GatewayEventFrame` (typed enum) — no String
- [ ] `GatewayEventBus::subscribe()` returns `broadcast::Receiver<GatewayEventFrame>` — no String
- [ ] All channel interfaces emit typed `GatewayEventFrame::Channel*` variants
- [ ] All agent loop events emit typed `GatewayEventFrame::Agent*` variants
- [ ] `#[serde(tag = "type")]` produces identical JSON wire format
- [ ] JSON-RPC params validated via `schemars` at all public entry points
- [ ] Invalid frames return structured `InvalidParams` error (no silent drops)
- [ ] `TopicFilter` glob patterns work on enum variant names (not raw strings)
- [ ] `cargo clippy -p alephcore -- -D warnings` passes
- [ ] 8681 existing tests pass
- [ ] No `unwrap()` or `expect()` in event bus or protocol validation code
