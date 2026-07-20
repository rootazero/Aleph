# 2026-04-05-001 — OpenClaw Comparative Analysis & Gateway Optimization Plan

> Learn from OpenClaw, then surpass it — leveraging Rust's type system and concurrency advantages.

---

## Status: Draft

## 1. Problem Statement

Comparing Aleph's Rust gateway against OpenClaw's TypeScript implementation reveals systematic deficiencies in three layers:

| Layer | OpenClaw (TypeScript) | Aleph (Rust) | Verdict |
|-------|----------------------|--------------|---------|
| **Protocol boundary** | TypeBox + AJV runtime validation | Manual serde, no runtime validation | ❌ Aleph gap |
| **Event bus** | Typed `AgentEventPayload` with `seq` ordering | `broadcast::Sender<String>`, untyped | ❌ Aleph gap |
| **Event frames** | `EventFrame` typed envelope with schema | Ad-hoc `TopicEvent { topic: String, data: Value }` | ❌ Aleph gap |
| **Channel plugins** | ~15 runtime-discoverable plugins in `channels/plugins/` | Channels are hardcoded modules | ❌ Aleph gap |
| **Session events** | `SessionEventSubscriberRegistry` per-session typed | No per-session subscription | ❌ Aleph gap |
| **Exec approval** | Human-in-the-loop `exec-approval` system | Not implemented | ❌ Missing |
| **Multi-device ACP** | `NodeRegistry` + device pairing | Not implemented | ❌ Missing |

OpenClaw compensates for TypeScript's lack of compile-time guarantees with extensive runtime validation (AJV). Aleph should do better — Rust's type system can enforce at compile time what OpenClaw validates at runtime.

**Design principle**: Don't copy OpenClaw's patterns verbatim. Adapt them to idiomatic Rust using:
- Zero-cost abstractions (type-state patterns, sealed traits)
- Compile-time type safety (no `Value` leakage past the boundary)
- Structured concurrency (tokio channels, not async event emitters)

---

## 2. Gap Analysis

### Gap 1: Protocol Boundary — No Schema Validation

**OpenClaw**: Every incoming frame is validated against a TypeBox schema via AJV before processing. Invalid frames are rejected with a structured error.

**Aleph**: Frames are deserialized via `serde_json` directly into typed structs. No schema validation. Malformed or unexpected fields are silently ignored (serde defaults).

**Impact**: Security and robustness. Unexpected fields in config updates, session patches, or agent params are silently accepted.

**Fix**: Add a `SchemaValidated<T>` newtype that wraps a deserialized value after `schemars` JSON Schema validation. Use at all public RPC entry points.

### Gap 2: Event Bus — Type Safety Lost at Serialization Boundary

**OpenClaw**: `emitAgentEvent()` fires a typed `AgentEventPayload` to in-process listeners synchronously. Listeners receive a fully typed object.

**Aleph**: `GatewayEventBus::publish_json()` serializes any typed struct to a JSON string, then broadcasts it. Subscribers receive `String` and must re-deserialize. There is no type guarantee — a subscriber for `agent.run.*` gets `String` and hopes the JSON contains what they expect.

```rust
// Aleph today — type-safe publisher, type-unsafe subscriber
pub fn publish_json<T: serde::Serialize>(&self, event: &T) -> Result<usize, serde_json::Error>
pub fn subscribe(&self) -> broadcast::Receiver<String>  // String is all we know
```

**Impact**: Runtime panics from JSON parse errors, incorrect topic/data assumptions. No compile-time enforcement of topic/data consistency.

**Fix**: Replace `broadcast::Sender<String>` with `broadcast::Sender<TopicEvent>` where `TopicEvent` is an enum with typed variants. Subscribers pattern-match on the enum. No String serialization at the bus level.

### Gap 3: Event Envelope — No Typed `EventFrame` Equivalent

**OpenClaw** `EventFrame`:
```typescript
type EventFrame = {
  type: "event";
  topic: string;
  data: Record<string, unknown>;
  seq: number;
  sessionKey?: string;
}
```

**Aleph** `TopicEvent`:
```rust
pub struct TopicEvent {
    pub topic: String,      // arbitrary string, no compile-time topics
    pub data: Value,        // arbitrary JSON, no typed data
    pub timestamp: u64,
}
```

**Impact**: Topic names like `"agent.run.started"` are just strings — typos not caught at compile time. Data shape is unconstrained.

**Fix**: Create a `GatewayEventFrame` enum with concrete variants for each topic, each carrying a typed payload struct. Compile-time exhaustive matching.

### Gap 4: Channel Plugin Registry — Not Runtime Discoverable

**OpenClaw**: Channels are plugins in `src/channels/plugins/` registered via a plugin registry. Adding a new channel = drop a file in the directory.

**Aleph**: Channels are modules in `src/gateway/interfaces/` imported and registered imperatively. Adding a new channel = modify existing code.

**Impact**: Extensibility. Third-party channel implementations require modifying Aleph core.

**Fix**: Implement a `ChannelPlugin` trait + `#[derive(ChannelPlugin)]` macro that auto-registers channels via a `CHANNEL_PLUGIN_REGISTRY` static. Aligns with the existing `ChannelFactory` pattern in `ChannelRegistry`.

### Gap 5: Session-Context Event Subscriptions

**OpenClaw**: `SessionEventSubscriberRegistry` allows subscribing to events scoped to a specific `sessionKey`. Events without matching `sessionKey` are not delivered.

**Aleph**: All `TopicEvent` broadcasts go to all subscribers. No session scoping.

**Impact**: Events like typing indicators, message delivery receipts, and agent progress are broadcast globally even when clients only care about their own session.

**Fix**: Add `session_id: Option<SessionId>` to `TopicEvent`. Subscribers filter by session context.

### Gap 6: Exec Approval — Missing Human-in-the-Loop

**OpenClaw**: `exec-approval` system requires human confirmation before executing shell commands.

**Impact**: Security. Any compromised channel can trigger dangerous tool executions.

**Fix**: Implement two-phase exec approval (already planned in `docs/plans/2026-04-04-006-feat-two-phase-exec-approval-plan.md` — not yet implemented). Priority should be high.

### Gap 7: Multi-Device ACP — Not Implemented

**OpenClaw**: `NodeRegistry` with device pairing, `NodePairRequestParams`, `NodeInvokeParams`, cross-device message routing.

**Impact**: Multi-device scenarios (phone + desktop) can't route agent events across devices.

**Fix**: Postpone for Phase 2. Requires ACP protocol design.

---

## 3. Optimization Plan

### Phase 1: Typed Event Bus (Highest Leverage, Lowest Risk)

**Goal**: Replace `broadcast::Sender<String>` with typed event distribution. Preserve `TopicFilter` glob matching at the subscription level.

**Approach**:
1. Create `gateway/events/frame.rs` — define `GatewayEventFrame` enum with concrete variants (not String-based topics):
   ```rust
   pub enum GatewayEventFrame {
       AgentRunStarted { run_id: RunId, session_id: SessionId, seq: u64 },
       AgentRunCompleted { run_id: RunId, session_id: SessionId, seq: u64 },
       AgentToolStart { run_id: RunId, tool_name: String, seq: u64 },
       AgentToolEnd { run_id: RunId, tool_name: String, seq: u64 },
       SessionCreated { session_id: SessionId },
       ConfigChanged { section: Option<String>, value: Value },
       // ... exhaustive list
   }
   ```
2. Migrate `GatewayEventBus` from `broadcast::Sender<String>` to `broadcast::Sender<GatewayEventFrame>`.
3. Add `#[serde(tag = "type")]` to `GatewayEventFrame` so it serializes to JSON identically to current `TopicEvent` for WebSocket clients.
4. Keep `TopicFilter` as a runtime filter on the enum's `type` field (for WebSocket subscription compatibility).
5. Update `event_emitter/mod.rs` to emit typed `GatewayEventFrame` variants instead of serializing to JSON first.
6. Add `session_id: Option<SessionId>` to relevant variants for session-scoped delivery.

**Risk**: Medium. All event emitters and subscribers must be updated. Test surface is large.

**Cleanup**: Remove `TopicEvent::to_notification()` (replaced by `#[serde(tag)]` on the enum). Remove `event_emitter/types.rs` StreamEvent boundary loss.

**Expected impact**:
- Compile-time exhaustive matching on event types
- Zero JSON re-deserialization errors in subscribers
- Session-scoped filtering at the bus level
- Rust's type system validates event data shapes at compile time

### Phase 2: Schema Validation at Protocol Boundary

**Goal**: Reject malformed incoming frames before they reach handler logic.

**Approach**:
1. Add `schemars` JSON Schema generation to Aleph's handler params types.
2. Create `gateway/protocol/validated.rs`:
   ```rust
   pub struct SchemaValidated<T> {
       value: T,
       schema: &'static schemars::Schema,
   }
   ```
3. Add a `Validate` trait:
   ```rust
   pub trait Validate: serde::de::DeserializeOwned {
       const SCHEMA: schemars::Schema = schemars::schema_for!(Self);
       fn validate(value: &serde_json::Value) -> Result<Self, SchemaError>;
   }
   ```
4. Add a `SchemaValidatedExt` trait to `serde_json::Value`:
   ```rust
   pub trait SchemaValidatedExt<T> {
       fn validated(self) -> Result<T, SchemaError>;
   }
   ```
5. At each `process_request` entry point, call `.validated()` before deserializing into the typed handler params.
6. Return structured `InvalidParams` errors matching JSON-RPC 2.0 spec.

**Risk**: Low. Validation is additive — handlers that skip it still work.

**Cleanup**: No legacy code removal in this phase.

**Expected impact**:
- Early rejection of malformed frames before any processing
- Clear error messages for API consumers
- Aligns with OpenClaw's AJV validation

### Phase 3: Channel Plugin Registry

**Goal**: Runtime-discoverable channel implementations via `#[derive(ChannelPlugin)]`.

**Approach**:
1. Define `ChannelPlugin` sealed trait in `gateway/interfaces/plugin.rs`:
   ```rust
   pub trait ChannelPlugin: Send + Sync {
       fn metadata(&self) -> ChannelMetadata;
       fn factory() -> Arc<dyn ChannelFactory>
       where Self: Sized;
   }
   ```
2. Create `channel_plugin_derive` proc macro that:
   - Generates `ChannelMetadata` from struct name and doc comments
   - Registers the plugin in a static `CHANNEL_PLUGIN_REGISTRY`
   - Implements the `ChannelFactory` trait for the channel struct
3. Add `#[derive(ChannelPlugin)]` to existing channel implementations:
   - `TelegramChannel`, `DiscordChannel`, `SlackChannel`, `WhatsAppChannel`, etc.
4. Add `ChannelRegistry::register_plugin()` that reads from the static registry.
5. Auto-register all compiled channels at startup via `inventory::submit!`.

**Risk**: Low. Uses existing `ChannelFactory` + `Channel` traits. Adds derive macro.

**Cleanup**: Remove hardcoded `mod telegram; mod discord;` imports from `interfaces/mod.rs` — replaced by plugin auto-discovery.

**Expected impact**:
- Adding a new channel = adding one file with `#[derive(ChannelPlugin)]`
- Third-party channels can be added without modifying Aleph core
- OpenClaw-style plugin architecture but type-safe

### Phase 4: Exec Approval (High Priority, Existing Plan)

**Already planned** in `docs/plans/2026-04-04-006-feat-two-phase-exec-approval-plan.md`. Not yet implemented.

**Action**: Implement per the existing plan after Phases 1-3.

### Phase 5: Multi-Device ACP (Postpone)

**Requires**: ACP protocol design document. Not started.

---

## 4. Cleanup Manifesto

> "优化和重构后要清理旧代码，避免屎山堆积"

Every phase includes explicit cleanup:

| Phase | Old Code to Remove |
|-------|-------------------|
| Phase 1 | `TopicEvent::to_notification()` (replaced by `#[serde(tag)]`), `event_emitter/types.rs` StreamEvent boundary types |
| Phase 2 | Any `unwrap()` or `expect()` on JSON deserialization in handler entry points |
| Phase 3 | Hardcoded channel module imports in `interfaces/mod.rs` (replaced by plugin registry) |
| All phases | No `as any` / `unwrap()` / `expect()` — validate or propagate |

**Anti-pattern to never reintroduce**:
```rust
// BAD — type-unsafe Value after serialization boundary
pub data: Value,

// GOOD — typed payload per event variant
AgentToolStart { tool_name: String, ... }
```

---

## 5. Open Questions

| # | Question | Decision |
|---|----------|----------|
| OQ1 | Should WebSocket clients receive typed JSON (same as today) or a binary protocol (MessagePack)? | Keep JSON for Phase 1. `#[serde(tag)]` on enum produces identical JSON shape. Binary can be Phase 2. |
| OQ2 | Should `GatewayEventFrame` variants carry `session_id: Option<SessionId>` or should sessions get a separate bus? | Add `session_id: Option<SessionId>` to relevant variants. Simpler than separate bus. |
| OQ3 | How to handle channel plugins that need async initialization? | Use `ChannelFactory::create()` which is already async. |
| OQ4 | Should we use `schemars` for JSON Schema generation or generate TypeBox-compatible schemas? | Use `schemars` (Rust-native). OpenClaw uses TypeBox for TypeScript interop — we have no such requirement. |

---

## 6. Success Criteria

- [ ] Phase 1: `GatewayEventBus` uses `broadcast::Sender<GatewayEventFrame>` — no `String` serialization at bus level
- [ ] Phase 1: All event emitters updated to emit typed enum variants
- [ ] Phase 1: `TopicFilter` still works with glob patterns on enum variant names
- [ ] Phase 2: All public RPC handlers validate incoming JSON against schema before processing
- [ ] Phase 2: Invalid frames return `InvalidParams` error with descriptive message
- [ ] Phase 3: `#[derive(ChannelPlugin)]` works on all 15 existing channels
- [ ] Phase 3: Adding a new channel requires zero changes to `ChannelRegistry` or `interfaces/mod.rs`
- [ ] All phases: `cargo clippy -p alephcore -- -D warnings` passes
- [ ] All phases: 8681 existing tests still pass
- [ ] All phases: No `unwrap()` or `expect()` in production event bus or protocol code
