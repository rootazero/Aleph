# WhatsApp Native Rust Redesign

**Status**: Design (pending implementation plan)  
**Date**: 2026-04-15  
**Scope**: Replace Go bridge with native Rust WhatsApp client, close feature gaps vs. openclaw, and remove legacy bridge code.  
**Decision**: Approved approach B — radical replacement with full cleanup.

---

## 1. Background

Aleph currently implements WhatsApp through an external Go binary (`whatsapp-bridge`) that communicates with the Rust core via JSON-RPC over a Unix domain socket. This architecture was chosen when no mature Rust WhatsApp Web library existed.

As of 2026, the Rust ecosystem has a production-ready alternative: [`wa-rs`](https://crates.io/crates/wa-rs) (a stable-Rust fork of `whatsapp-rust`), which implements the full WhatsApp Web protocol including Signal Protocol E2E encryption, QR/pair-code auth, groups, media, reactions, and read receipts. This makes the Go bridge obsolete.

Meanwhile, openclaw’s TypeScript WhatsApp implementation (`extensions/whatsapp/`) demonstrates a rich feature set that Aleph lacks: group-policy engines, media optimization, poll support, multi-account runtime, heartbeat monitoring, and sophisticated text chunking.

This design specifies how to:
1. Replace the Go bridge with a native Rust `wa-rs` integration.
2. Refactor `WhatsAppChannel` to align with Aleph’s `Channel` trait without bridge-specific adapters.
3. Close feature gaps by porting openclaw’s capabilities into idiomatic Rust.
4. **Delete** all legacy bridge code to prevent technical debt accumulation.

---

## 2. Goals & Non-Goals

### Goals
- Implement a native Rust WhatsApp client using `wa-rs` inside `alephcore`.
- Make `WhatsAppChannel` a first-class `Channel` trait implementation with no external process dependencies.
- Achieve feature parity with openclaw for messaging core (text, media, reactions, polls, read receipts, group policies, multi-account).
- Remove the entire Go bridge codebase (`interfaces/whatsapp-bridge/`) and its Rust-side glue (`BridgeManager`, `BridgeRpcClient`, `BridgeProtocol`, `BridgedChannel` adaptations).
- Leverage Rust’s type safety and `tokio` concurrency to make the connection state machine and event loop more robust than the TypeScript reference.

### Non-Goals
- We will **not** support the WhatsApp Business API (cloud/on-prem) in this redesign. Scope remains WhatsApp Web (Baileys protocol) only.
- We will **not** build a generic “plugin SDK” for channels; this work is scoped to WhatsApp only, though patterns may inform future channels.
- We will **not** change the `Channel` trait or `ChannelRegistry` abstractions in `src/gateway/channel.rs`; we will conform to them.

---

## 3. Architecture Overview

### 3.1 Before (Current)

```text
┌─────────────────────────────────────────────┐
│  Aleph Core (Rust)                          │
│  ┌───────────────────────────────────────┐  │
│  │  WhatsAppChannel                      │  │
│  │  ├─ BridgeManager  (spawn go binary)  │  │
│  │  ├─ BridgeRpcClient (unix socket)     │  │
│  │  └─ BridgeEvent loop                  │  │
│  └───────────────────────────────────────┘  │
└──────────────────┬──────────────────────────┘
                   │ JSON-RPC over unix socket
                   ▼
┌─────────────────────────────────────────────┐
│  whatsapp-bridge (Go binary)                │
│  ├─ socket server                           │
│  ├─ handler (connect, send, etc.)           │
│  └─ Baileys (JS via embedded Node)          │
└─────────────────────────────────────────────┘
```

Problems:
- Cross-language serialization overhead and error impedance mismatch.
- Process lifecycle complexity (spawn, restart, stale socket cleanup).
- `BridgeManager`/`BridgeRpcClient` are WhatsApp-specific bridge glue that cannot be reused.
- `native_baileys/` exists as an empty stub, signaling architectural confusion.
- Deployment requires shipping a Go binary alongside the Rust executable.

### 3.2 After (Redesigned)

```text
┌──────────────────────────────────────────────────────────┐
│  Aleph Core (Rust)                                       │
│  ┌────────────────────────────────────────────────────┐  │
│  │  ChannelRegistry                                   │  │
│  │  └─ WhatsAppChannel  (implements Channel trait)    │  │
│  │       ├─ WaRuntime   (wa-rs Bot + event loop)      │  │
│  │       ├─ WaOutbound  (send, react, poll, mark_read)│  │
│  │       ├─ WaInbound   (event mapping, policy gates) │  │
│  │       ├─ WaAuth      (Vault-backed session store)  │  │
│  │       ├─ WaPolicy    (DM/group/mention/allowlist)  │  │
│  │       ├─ WaMedia     (download, optimize, upload)  │  │
│  │       └─ WaChunking  (text split, caption rules)   │  │
│  └────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
                              │
                              ▼
                       WhatsApp Servers
```

All logic lives inside a single Rust process. `wa-rs` handles the WebSocket transport, Signal Protocol encryption, and protocol framing. Aleph layers its `Channel` trait, policy engine, media pipeline, and Vault storage on top.

---

## 4. Module Structure

```
src/gateway/interfaces/whatsapp/
├── mod.rs                    # WhatsAppChannel (Channel trait impl)
├── config.rs                 # WhatsAppConfig, WhatsAppAccountConfig
├── factory.rs                # ChannelFactory for registry registration
│
├── runtime/
│   ├── mod.rs                # WaRuntime: owns wa-rs Bot, connection loop
│   ├── client.rs             # Thin wrapper around wa-rs Bot/Client
│   ├── event_loop.rs         # Tokio task: stream events → inbound channel
│   └── state.rs              # ConnectionState machine (Disconnected → Connecting → Connected)
│
├── auth/
│   ├── mod.rs                # WaAuthManager trait + Vault impl
│   └── vault_store.rs        # Serialize wa-rs auth state into Aleph Vault
│
├── outbound/
│   ├── mod.rs                # WaOutbound: send_message, send_reaction, send_poll, mark_read
│   ├── chunking.rs           # Text chunking (length / newline modes)
│   └── media.rs              # Media preprocessing before wa-rs send
│
├── inbound/
│   ├── mod.rs                # WaInbound: event mapping + policy gates
│   ├── mapper.rs             # baileys Event → Aleph InboundMessage
│   ├── policy.rs             # DM policy, group policy, mention gating
│   └── history_buffer.rs     # Group chat history injection (moved from root)
│
├── policy/
│   ├── mod.rs                # Allowlist evaluation, pairing codes
│   ├── dm_policy.rs          # Pairing / allowlist / open / disabled
│   └── group_policy.rs       # Group allowlists, mention detection
│
├── media/
│   ├── mod.rs                # MediaProcessor: download, resize, format fixups
│   └── limits.rs             # Per-account mediaMaxMb enforcement
│
├── message/
│   ├── mod.rs                # Message type conversions (wa-rs ↔ Aleph)
│   └── formats.rs            # Markdown-to-WhatsApp text normalization
│
├── reactions.rs              # Reaction level, ack reactions
├── account.rs                # WhatsApp account data structures
├── account_registry.rs       # Multi-account resolution
├── types.rs                  # Shared WhatsApp-specific types (JID helpers, etc.)
└── tests/
    ├── mod.rs
    ├── outbound_test.rs
    ├── policy_test.rs
    └── media_test.rs

[DELETED]
- bridge_manager.rs
- rpc_client.rs
- bridge_protocol.rs
- bridge_fallback.rs
- native_baileys/ (the old empty stub)
- baileys_runtime.rs (absorbed into runtime/)
- interfaces/whatsapp-bridge/ (entire Go project)
```

---

## 5. Key Components

### 5.1 `WhatsAppChannel` (`mod.rs`)

Implements the existing `Channel` trait with no bridge-specific types.

```rust
pub struct WhatsAppChannel {
    info: ChannelInfo,
    state: ChannelState,
    config: WhatsAppConfig,
    runtime: Option<WaRuntimeHandle>,
    outbound: WaOutbound,
    policy: WaPolicyEngine,
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn info(&self) -> &ChannelInfo { &self.info }
    fn state(&self) -> &ChannelState { &self.state }

    async fn start(&mut self) -> ChannelResult<()> { ... }
    async fn stop(&mut self) -> ChannelResult<()> { ... }
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> { ... }
    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> { ... }
    async fn mark_read(&self, message_id: &MessageId) -> ChannelResult<()> { ... }
    async fn react(&self, conversation_id: &ConversationId, message_id: &MessageId, reaction: &str)
        -> ChannelResult<()> { ... }
    async fn get_pairing_data(&self) -> ChannelResult<PairingData> { ... }
}
```

### 5.2 `WaRuntime` (`runtime/`)

Wraps `wa-rs` and exposes a handle-based API to `WhatsAppChannel`.

Responsibilities:
- Build `wa-rs::Bot` with Aleph-customized storage (Vault-backed auth) and transport.
- Spawn a background `tokio` task that drives the `wa-rs` event stream.
- Emit mapped events into `ChannelState`’s inbound broadcast channel.
- Expose `send_message`, `send_reaction`, `send_poll`, `mark_read` calls to the outbound layer.

```rust
pub struct WaRuntime {
    bot: wa_rs::Bot,
    event_tx: mpsc::Sender<wa_rs::Event>,
    shutdown: CancellationToken,
}

impl WaRuntime {
    pub async fn start(config: &WhatsAppConfig, auth: WaAuthManager) -> Result<Self, WaError> { ... }
    pub async fn shutdown(self) { ... }
    pub fn connection_state(&self) -> ConnectionState { ... }
}
```

The event loop runs on its own `tokio::task` and translates `wa-rs::Event` into Aleph `InboundMessage` (or internal status updates) using the inbound mapper.

### 5.3 `WaAuthManager` (`auth/`)

Replaces file-based bridge session storage with Aleph Vault.

- Vault path: `whatsapp/auth/{account_id}`
- Stores `wa-rs` auth state (creds + keys + app state sync) as an encrypted blob.
- On first run, triggers QR/pair-code pairing flow and persists the resulting auth state.
- On subsequent runs, loads auth state from Vault and resumes the session without re-pairing.

This satisfies the existing design requirement to integrate with Aleph Vault and removes the file-system auth directory pattern inherited from the Go bridge.

### 5.4 `WaOutbound` (`outbound/`)

Handles all outgoing traffic, incorporating openclaw’s capabilities:

| Capability | Implementation |
|---|---|
| Text chunking | `chunking.rs` implements `ChunkMode::Length` and `ChunkMode::Newline`, default limit 4000 chars |
| Media sending | Pre-process media: auto-resize images, enforce `mediaMaxMb`, rewrite `audio/ogg` → `audio/ogg; codecs=opus`, set `gifPlayback` flag |
| Polls | Map Aleph poll structures to `wa-rs` poll sends |
| Reactions | Forward to `wa-rs` reaction API |
| Read receipts | Forward to `wa-rs` mark-read API |
| Typing indicators | Forward to `wa-rs` presence/composing API |

`WaOutbound` resolves the active account from `WhatsAppConfig` (supporting multi-account) before every send.

### 5.5 `WaPolicyEngine` (`policy/` + `inbound/policy.rs`)

Closes the biggest functional gap vs. openclaw. On every inbound message, the policy engine evaluates:

1. **DM Policy**
   - `Pairing` (default): unknown senders generate a pairing request; user must approve.
   - `Allowlist`: sender must match `allowFrom` list.
   - `Open`: accept all (requires explicit `allowFrom: ["*"]`).
   - `Disabled`: drop all DMs.

2. **Group Policy**
   - Group membership allowlist (`groups`): if present, only listed group JIDs are eligible.
   - Sender policy (`groupPolicy` + `groupAllowFrom`): `open`, `allowlist`, or `disabled`.
   - Group sender allowlist falls back to `allowFrom` when `groupAllowFrom` is unset.

3. **Mention Gating**
   - Explicit `@bot` mention in message text.
   - Configured mention regex patterns.
   - Implicit reply-to-bot detection.
   - Session-level override (`/activation mention` / `/activation always`).

4. **Pairing Requests**
   - Persist approved pairings (merged with configured `allowFrom`).
   - Expire after 1 hour, capped at 3 pending per channel.

Messages that fail policy checks are dropped (with tracing logs) and do not enter the gateway pipeline.

### 5.6 `WaMedia` (`media/`)

Media pipeline for both inbound and outbound:

- **Inbound**: download media from WhatsApp CDN via `wa-rs`, decode if needed, apply `mediaMaxMb` save cap.
- **Outbound**: load media from URL or local path, enforce send cap, auto-optimize images (resize/quality sweep), fix MIME types for WhatsApp compatibility.
- On media send failure, fallback to sending a text warning instead of silently dropping the response.

### 5.7 `WaChunking` (`outbound/chunking.rs`)

Text delivery optimization:

- `ChunkMode::Length`: split at `textChunkLimit` characters with word-safe boundaries.
- `ChunkMode::Newline`: prefer paragraph boundaries (blank lines), then fall back to length-safe chunking.
- Captions: when sending multi-media replies, the caption is applied to the first media item only (matching openclaw behavior).

---

## 6. Data Flow

### 6.1 Outbound

```text
Thinker / Gateway
       │
       ▼
WhatsAppChannel::send(OutboundMessage)
       │
       ▼
WaOutbound::send_message(account_id, conversation_id, text, attachments, options)
       │
       ├─ chunk text if needed
       ├─ preprocess media (resize, MIME fixup)
       └─ resolve target JID
       ▼
WaRuntime::send_message(wa_rs::OutboundMessage)
       │
       ▼
wa-rs Bot ──► WhatsApp Server
```

### 6.2 Inbound

```text
WhatsApp Server
       │
       ▼
wa-rs Bot (WebSocket + Signal Protocol)
       │
       ▼
WaRuntime event_loop task
       │
       ├─ ConnectionOpen  → update ChannelState status to Connected
       ├─ ConnectionClose → update status to Disconnected / Error
       ├─ MessagesUpsert  → map to InboundMessage
       │                       │
       │                       ▼
       │                  WaPolicyEngine::evaluate()
       │                       │
       │                       ├─ DM policy check
       │                       ├─ Group policy check
       │                       └─ Mention gating
       │                       │
       │                       ▼
       │                  Pass? → ChannelState::send_inbound()
       │                  Fail?  → log + drop
       │
       └─ Receipt / Presence / Reaction updates → internal handlers
       ▼
ChannelRegistry inbound router
       ▼
Thinker / ContextAggregator
```

### 6.3 Authentication / Pairing

```text
User initiates pairing (CLI / UI)
       │
       ▼
WhatsAppChannel::get_pairing_data()
       │
       ▼
WaRuntime::start_pairing_flow()
       │
       ├─ No Vault auth found → wa-rs generates QR code
       │                       QR code returned as PairingData::QrCode
       │
       └─ Vault auth found    → resume session directly
                               return PairingData::None
       │
       ▼
User scans QR
       │
       ▼
wa-rs completes handshake
       │
       ▼
WaAuthManager::save_auth(state) → Aleph Vault
       │
       ▼
ConnectionOpen event → ChannelStatus::Connected
```

---

## 7. Feature Parity with OpenClaw

| Feature | OpenClaw | Aleph (After Redesign) |
|---|---|---|
| Protocol | WhatsApp Web (Baileys) | WhatsApp Web (`wa-rs`) |
| QR / Pair-code login | Yes | Yes |
| Multi-account | Yes | Yes (per-account config + registry) |
| DM policy (pairing/allowlist/open/disabled) | Yes | Yes |
| Group allowlist | Yes | Yes |
| Group sender policy | Yes | Yes |
| Mention gating | Yes | Yes |
| Self-chat safeguards | Yes | Yes |
| Text chunking | Yes | Yes |
| Read receipts | Yes | Yes |
| Reactions (ack + agent) | Yes | Yes |
| Polls | Yes | Yes |
| Media optimization | Yes | Yes |
| Heartbeat / health | Yes | Yes (via ChannelHealth + custom heartbeat) |
| Inbound history injection | Yes | Yes (group history buffer) |
| Vault-backed auth | No (file-based) | Yes |
| Single-process deployment | No (Node runtime + gateway) | Yes (pure Rust) |

---

## 8. Cleanup Plan — Code to Delete

The following files and directories must be removed to prevent technical debt:

### Go Bridge (entire directory)
- `interfaces/whatsapp-bridge/` — complete Go project
  - `cmd/whatsapp-bridge/main.go`
  - `internal/handler/handler.go`
  - `internal/socket/server.go`
  - `go.mod`, `go.sum`, and all supporting Go files

### Rust-Side Bridge Glue
- `src/gateway/interfaces/whatsapp/bridge_manager.rs`
- `src/gateway/interfaces/whatsapp/rpc_client.rs`
- `src/gateway/interfaces/whatsapp/bridge_protocol.rs`
- `src/gateway/interfaces/whatsapp/bridge_fallback.rs` (planned but not yet real)
- `src/gateway/interfaces/whatsapp/baileys_runtime.rs` (absorbed into `runtime/`)
- `src/gateway/interfaces/whatsapp/native_baileys/` — the old empty stub directory

### Obsolete Config Fields
- `WhatsAppConfig.bridge_binary`
- `WhatsAppConfig.max_restarts`

### Build / CI References
- Any `Makefile`, `justfile`, or CI steps that compile or copy `whatsapp-bridge`
- References to `whatsapp-bridge` in packaging scripts

### Legacy Bridge Tests
- Tests inside `bridged_channel.rs` that specifically mock the WhatsApp bridge protocol (keep generic bridged-channel tests if they apply to other channels)

---

## 9. Migration Strategy

### Phase 1: Foundation (1–2 weeks)
- Add `wa-rs` and `wa-rs-tokio-transport` dependencies behind a temporary `whatsapp-native` feature flag.
- Implement `auth/vault_store.rs` and `runtime/client.rs` (thin wrapper around `wa-rs`).
- Implement basic `event_loop.rs` that can connect and emit `ConnectionOpen` / `ConnectionClose`.
- **Verification**: `cargo check --features whatsapp-native` passes; unit tests for auth store pass.

### Phase 2: Messaging Core (2 weeks)
- Implement inbound mapper (`MessagesUpsert` → `InboundMessage`).
- Implement outbound send (`text` + `media` basic).
- Wire `WhatsAppChannel` to use `WaRuntime` instead of `BridgeManager`.
- **Verification**: Integration test sends a text message and receives a reply through a mock `wa-rs` backend or a controlled test account.

### Phase 3: Feature Parity & Policy (2 weeks)
- Port chunking, media optimization, reactions, polls, read receipts.
- Implement `WaPolicyEngine` with DM/group/mention gating.
- Implement multi-account runtime resolution.
- **Verification**: Policy tests cover all gate combinations; media tests verify resize and fallback behavior.

### Phase 4: Cleanup & Removal (1 week)
- Delete Go bridge directory and Rust bridge glue files.
- Remove obsolete config fields and update `WhatsAppConfig::validate()`.
- Remove feature flag; make native implementation the only path.
- **Verification**: `cargo clippy -D warnings`, `cargo test --lib`, and full gateway smoke test pass.

---

## 10. Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| `wa-rs` protocol drift | High | Pin exact version; subscribe to upstream releases; maintain regression tests that verify connection handshake against live WhatsApp Web endpoints in CI (weekly smoke). |
| Vault auth migration (users with existing file-based sessions) | Medium | Implement one-time migration: on first startup, if Vault entry is missing but old bridge `data_dir` exists, import the session state into Vault and delete the old files. If import is impossible, prompt for re-pairing. |
| Group policy complexity | Medium | Start with DM policy + simple group allowlist; add mention gating and advanced rules in Phase 3. Write table-driven tests for every policy matrix. |
| Media handling edge cases | Medium | Re-use Aleph’s existing HTTP client pool (`reqwest`) for media downloads; implement strict size caps and fallbacks; test with real image/video/audio files. |
| Deleting bridge code too early | Low | Only delete in Phase 4 after Phase 2/3 messaging tests are green. Keep a backup branch until production validates stability. |

---

## 11. Open Questions

1. Do we need to support **pair code** (8-digit code) login in addition to QR code? (openclaw supports both; `wa-rs` supports both.)
2. Should we keep **per-account auth directories** as a transient fallback during migration, or migrate everything into Vault immediately?
3. Are there any existing **integration tests** in CI that depend on the `whatsapp-bridge` binary? If so, they must be rewritten to use `wa-rs` mocks or a test WhatsApp account.

---

## 12. References

- Aleph existing WhatsApp: `src/gateway/interfaces/whatsapp/`
- Aleph `Channel` trait: `src/gateway/channel.rs`
- openclaw WhatsApp extension: `/Volumes/TBU4/Github/openclaw/extensions/whatsapp/`
- openclaw WhatsApp docs: `/Volumes/TBU4/Github/openclaw/docs/channels/whatsapp.md`
- `wa-rs` crate: https://crates.io/crates/wa-rs
- `wa-rs` repository: https://github.com/homunbot/wa-rs
- Previous Aleph design (now superseded): `docs/superpowers/specs/2026-04-12-whatsapp-native-design.md`
