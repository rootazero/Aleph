# Feishu Channel Architecture Alignment Design

> Date: 2026-04-16  
> Scope: `src/gateway/interfaces/feishu/`  
> Objective: Align Feishu channel architecture with the proven WhatsApp native pattern, fix functional gaps, and clean up obsolete code.

---

## 1. Background & Motivation

Aleph's Feishu channel currently lives as a flat collection of 11 files under `src/gateway/interfaces/feishu/`. While functional for basic text and image messaging, it suffers from:

- **Architecture drift**: No clear separation between runtime, inbound, outbound, and policy concerns—unlike WhatsApp's `wa_runtime` / `wa_inbound` / `wa_outbound` / `wa_policy` layers.
- **Feature gaps**: `MessageOperations` (reply/react/edit/delete) are unimplemented; `CardAction` events are parsed but never injected into the agent loop; webhook challenge verification is broken.
- **Code duplication**: Group/DM filtering logic is copy-pasted between `websocket.rs` and `webhook.rs`.
- **Technical debt**: Obsolete design documents and stale WhatsApp bridge references accumulate in `docs/superpowers/`.

This design adopts **Scheme B1 — Full Architectural Alignment**: rebuild Feishu with the same layered boundaries that have proven stable for WhatsApp, while exploiting Rust's module system for compile-time safety and testability.

---

## 2. Target Directory Structure

```
src/gateway/interfaces/feishu/
├── mod.rs                 # FeishuChannel + Channel trait impl (~120 lines)
├── config.rs              # FeishuConfig, account resolution, validation
├── types.rs               # Pure data types: events, API responses
├── auth.rs                # TokenManager
├── api.rs                 # FeishuApi HTTP client (trimmed of business logic)
├── message_ops.rs         # MessageOperations trait implementation (NEW)
│
├── feishu_runtime/        # Connection lifecycle & state machine
│   ├── mod.rs
│   ├── state.rs           # RuntimeState enum
│   └── ws_client.rs       # WebSocket connect/read/reconnect loop
│
├── feishu_inbound/        # Event ingestion, mapping, dedup, caching
│   ├── mod.rs
│   ├── events.rs          # WS / Webhook event parsing
│   ├── mapper.rs          # FeishuEvent → InboundMessage
│   ├── policy.rs          # Inbound evaluation (DM / group / mention)
│   ├── dedup.rs           # MessageDedup (migrated)
│   ├── user_cache.rs      # UserProfileCache with TTL & capacity (migrated)
│   └── webhook_server.rs  # Axum handler for Feishu webhooks (NEW)
│
├── feishu_outbound/       # Sending messages, cards, media
│   ├── mod.rs
│   ├── sender.rs          # Text / image / card dispatch logic
│   ├── media.rs           # Image upload & media download helpers
│   ├── streaming.rs       # FeishuEventEmitter + StreamingCard (migrated)
│   └── reactions.rs       # Typing indicator & reaction helpers (NEW)
│
└── feishu_policy/         # Access-control policies
    ├── mod.rs
    ├── group_policy.rs    # Group allowlist, require-mention rules
    └── dm_policy.rs       # DM toggle, allow-from list
```

### Boundary Rules

1. `api.rs` **must not** contain business decisions (e.g., "should this message be a card?"). It is a thin HTTP wrapper only.
2. `feishu_runtime` owns the transport connection (WebSocket) and shutdown signalling.
3. `feishu_inbound` owns the entire path from raw bytes → `InboundMessage`.
4. `feishu_outbound` owns the entire path from `OutboundMessage` / `MessageOperations` → HTTP request.
5. `feishu_policy` is called **once** by the inbound mapper, eliminating the current duplication between WS and webhook paths.

---

## 3. Data Flow

### 3.1 Startup (`FeishuChannel::start`)

```
FeishuChannel::start()
    ├─► config.validate()
    ├─► TokenManager::new() → refresh app_access_token
    ├─► FeishuApi::new()    → confirm bot_open_id
    ├─► FeishuRuntime::new(api, config)
    │   └─► runtime.start()
    │       ├─ WebSocket mode: spawn_ws_loop() in feishu_runtime/ws_client.rs
    │       └─ Webhook mode:  (managed by external caller / gateway server)
    ├─► FeishuMessageOps::new(api) → register in MessageOperationsRegistry
    └─► status = Connected
```

### 3.2 Inbound Flow

```
WebSocket Frame or Webhook Payload
    │
    ▼
feishu_inbound::events::parse()
    │
    ▼
FeishuEvent::MessageReceive | CardAction | BotAdded | ...
    │
    ▼
feishu_inbound::mapper::map_event()
    ├─► dedup::is_duplicate()
    ├─► user_cache::resolve_name()
    ├─► extract_text_content()   // text / post / image / file / audio / video
    └─► build conversation_id    // respects group_session_scope
    │
    ▼
feishu_inbound::policy::evaluate()
    ├─► DM toggle
    ├─► Group allowlist
    └─► Require-mention check
    │
    ▼
InboundMessage ──► channel_state.sender() ──► Agent Loop
```

**CardAction Injection**
- `CardAction` events are mapped to an `InboundMessage` whose `text` field is the button `action_value`.
- This closes the gap where card interactions were previously logged and discarded.

### 3.3 Outbound Flow

```
Agent Loop ──► FeishuChannel::send(OutboundMessage)
    │
    ▼
feishu_outbound::sender::send_message()
    ├─► decide message type: text / image / card
    ├─► should_use_card() logic lives here
    ├─► invoke api.reply_message() when reply_to is present
    └─► invoke api.send_text() / api.send_image() / api.send_card()
    │
    ▼
SendResult
```

**MessageOperations Flow**
```
Tool call ──► FeishuMessageOps::{reply,react,edit,delete,send}
    ├─ reply  ──► api.reply_message()
    ├─ react  ──► api.add_reaction()
    ├─ edit   ──► Err(Unsupported)   // Feishu text messages are immutable
    ├─ delete ──► Err(Unsupported)   // same reason
    └─ send   ──► api.send_text()
```

### 3.4 Runtime Reconnect & Shutdown

```rust
// feishu_runtime/ws_client.rs
let mut backoff = 1;
loop {
    match connect_async(&url).await {
        Ok(ws) => {
            backoff = 1;
            state = Connected;
            read_messages_until_disconnect_or_shutdown(ws).await;
        }
        Err(e) => {
            state = Error;
            sleep_with_shutdown(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(60);
            url = api.refresh_ws_endpoint().await?; // old URLs expire
        }
    }
}
```

Shutdown is triggered by `FeishuChannel::stop()` via a `tokio::sync::oneshot::Sender/Receiver`—mirroring the WhatsApp pattern.

---

## 4. Functional Additions & Fixes

### 4.1 High Priority (Required)

| # | Feature | File(s) | Details |
|---|---------|---------|---------|
| 1 | **MessageOperations** | `message_ops.rs` | Implement `reply`, `react`, `edit`, `delete`, `send`. Register in `MessageOperationsRegistry`. |
| 2 | **CardAction in Agent Loop** | `feishu_inbound/mapper.rs` | Map `FeishuEvent::CardAction` to `InboundMessage` with `text = action_value`. |
| 3 | **Webhook Challenge Fix** | `feishu_inbound/webhook_server.rs` | Return `{"challenge": <body.challenge>}`. Currently returns empty string, causing Feishu URL verification to fail. |
| 4 | **Webhook Signature Fix** | `feishu_inbound/webhook_server.rs` | Compute HMAC-SHA256 over `timestamp + nonce + encrypt_key + body` per Feishu official spec. |
| 5 | **UserProfileCache TTL + Capacity** | `feishu_inbound/user_cache.rs` | 500 entry capacity, 1-hour TTL, LRU eviction. |
| 6 | **Unified Inbound Policy** | `feishu_policy/` + `feishu_inbound/policy.rs` | Single evaluation point for DM/group/mention rules. |

### 4.2 Medium Priority (Included)

| # | Feature | File(s) | Details |
|---|---------|---------|---------|
| 7 | **Extended Message Types** | `feishu_inbound/mapper.rs` | Handle `post`, `file`, `audio`, `video`, `sticker` by mapping to placeholder text (e.g., `[Post]`, `[File: name]`) instead of dropping them. |
| 8 | **Multi-Account Runtime** | `feishu_runtime/mod.rs` | Use `config.resolve_credentials(account_id)` so multiple Feishu accounts can coexist. |
| 9 | **Typing Indicator** | `feishu_outbound/reactions.rs` | Extract the existing "Typing" reaction logic from `streaming.rs` into a reusable helper. |

---

## 5. Obsolete Code Cleanup

### 5.1 Documents to Delete

The following design/plan documents are superseded by this spec:

- `docs/superpowers/specs/2026-03-27-feishu-channel-optimization-design.md`
- `docs/superpowers/specs/2026-03-22-feishu-channel-design.md`
- `docs/superpowers/specs/2026-03-22-feishu-enhanced-design.md`
- `docs/superpowers/plans/2026-03-27-feishu-channel-optimization.md`
- `docs/superpowers/plans/2026-03-22-feishu-channel.md`
- `docs/superpowers/plans/2026-03-22-feishu-enhanced.md`
- `docs/superpowers/plans/2026-04-12-whatsapp-native-implementation.md`
- `docs/superpowers/specs/2026-04-12-whatsapp-native-design.md`

### 5.2 Source Files to Migrate & Remove

After their logic is moved into the new layered modules:

- `src/gateway/interfaces/feishu/websocket.rs` → `feishu_runtime/ws_client.rs` + `feishu_inbound/`
- `src/gateway/interfaces/feishu/webhook.rs` → `feishu_inbound/webhook_server.rs`
- `src/gateway/interfaces/feishu/streaming.rs` → `feishu_outbound/streaming.rs`
- `src/gateway/interfaces/feishu/events.rs` → `feishu_inbound/events.rs`
- `src/gateway/interfaces/feishu/dedup.rs` → `feishu_inbound/dedup.rs`
- `src/gateway/interfaces/feishu/user_cache.rs` → `feishu_inbound/user_cache.rs`

### 5.3 Stale References to Scrub

- `src/gateway/link/manager.rs` tests: remove `whatsapp-go` bridge YAML stubs (WhatsApp is now native Rust).
- Any `CHANNEL_SECRET_FIELDS` or channel handler comments that still claim WhatsApp uses a go-bridge.

**Note**: `src/gateway/bridge/` (the generic `BridgeSupervisor` infrastructure) is **retained** because it may still be used by Signal or future external bridges.

---

## 6. Testing Strategy

### 6.1 Migrate Existing Tests

| Original | Destination | Action |
|----------|-------------|--------|
| `events.rs` (30+ tests) | `feishu_inbound/events.rs` | Migrate verbatim |
| `dedup.rs` (4 tests) | `feishu_inbound/dedup.rs` | Migrate verbatim |
| `user_cache.rs` (1 test) | `feishu_inbound/user_cache.rs` | Migrate + add TTL/LRU tests |
| `config.rs` (existing) | `config.rs` | Keep as-is |

### 6.2 New Tests

| Module | Test Name | Purpose |
|--------|-----------|---------|
| `feishu_inbound/mapper.rs` | `test_card_action_to_inbound_message` | Verify CardAction injects into loop |
| `feishu_inbound/mapper.rs` | `test_post_message_placeholder` | Verify post type maps to text |
| `feishu_inbound/policy.rs` | `test_dm_blocked`, `test_group_allowed`, `test_mention_required` | Policy matrix |
| `feishu_inbound/webhook_server.rs` | `test_challenge_response`, `test_signature_verification` | Webhook correctness |
| `message_ops.rs` | `test_capabilities`, `test_reply_mock` | MessageOps wiring |
| `feishu_outbound/sender.rs` | `test_should_use_card_boundaries` | Card decision logic |

### 6.3 Quality Gates

- `cargo check -p alephcore` — must be clean.
- `cargo clippy -p alephcore -- -D warnings` — must be clean.
- `cargo test -p alephcore --lib feishu` — all tests must pass.

---

## 7. Design Principles Applied

1. **Symmetry with WhatsApp**: `feishu_runtime` / `feishu_inbound` / `feishu_outbound` / `feishu_policy` mirror `wa_runtime` / `wa_inbound` / `wa_outbound` / `wa_policy`. This reduces cognitive load for future maintainers.
2. **Fail Fast**: `config.validate()` runs before any network I/O.
3. **Single Source of Truth**: Inbound policy is evaluated in exactly one place.
4. **YAGNI**: No attempt to build a generic "card builder DSL"—card logic stays scoped to Feishu until another channel needs it.
5. **No Bridge Unless Needed**: Feishu talks HTTP directly; no external process bridge is introduced.

---

## 8. Approval

User approved this design on 2026-04-16 via conversational confirmation.
