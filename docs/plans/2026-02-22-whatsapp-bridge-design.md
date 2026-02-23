# WhatsApp Bridge Design: Thin Sidecar + Rich Adapter

> Date: 2026-02-22
> Status: Approved
> Scope: WhatsApp real protocol integration via whatsmeow Go Sidecar

---

## 1. Overview

Replace the current WhatsApp stub implementation with a real protocol adapter using the **Thin Sidecar** architecture: a Go binary wrapping [whatsmeow](https://github.com/tulir/whatsmeow) (WhatsApp Multi-Device protocol), managed as a child process by Aleph Server, communicating via JSON-RPC over Unix Socket.

### Design Principles

- **Thin Go, Rich Rust**: Go bridge < 800 LOC, all business logic in Rust
- **Process isolation**: Bridge crash does not affect AI Server
- **Zero configuration**: Bridge binary auto-spawned on `channel.start()`
- **Full lifecycle transparency**: 9-state pairing state machine with real-time Dashboard updates

### Architecture Context

This design addresses Phase 8 (Multi-Channel) by upgrading WhatsApp from stub to production-ready, while establishing patterns reusable for future QR-based channels (WeChat, LINE).

---

## 2. System Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                      aleph-server (Rust)                         │
│                                                                  │
│  ┌──────────────┐  ┌──────────────────────────────────────────┐  │
│  │ Gateway      │  │ WhatsAppChannel (impl Channel trait)     │  │
│  │              │  │                                          │  │
│  │ RPC Handlers ├──┤  ┌─────────────────┐                    │  │
│  │   channels.* │  │  │ BridgeManager   │ spawn/restart/kill │  │
│  │              │  │  └────────┬────────┘                    │  │
│  │ EventBus  ◄──┤  │          │                              │  │
│  │ (TopicEvent) │  │  ┌───────┴────────┐                    │  │
│  └──────────────┘  │  │ BridgeRpcClient│ JSON-RPC client    │  │
│                    │  └───────┬────────┘                    │  │
│                    │          │                              │  │
│                    │  ┌───────┴──────────────┐               │  │
│                    │  │ PairingStateMachine  │               │  │
│                    │  │ Idle → Initializing  │               │  │
│                    │  │ → WaitingQr → ...    │               │  │
│                    │  │ → Connected          │               │  │
│                    │  └─────────────────────┘               │  │
│                    └──────────────────────────────────────────┘  │
│                               │                                  │
│                    Unix Socket │ /tmp/aleph-wa-{id}.sock          │
└───────────────────────────────┼──────────────────────────────────┘
                                │
┌───────────────────────────────┼──────────────────────────────────┐
│               whatsapp-bridge (Go binary)                        │
│                               │                                  │
│  ┌────────────────┐  ┌───────┴────────┐  ┌──────────────────┐   │
│  │ JSON-RPC Server│  │   whatsmeow    │  │ SQLite Store     │   │
│  │ (Unix Socket)  │──│   (MD client)  │──│ (session/keys)   │   │
│  └────────────────┘  └────────────────┘  └──────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Location | Responsibility |
|-----------|----------|---------------|
| **BridgeManager** | `core/src/gateway/interfaces/whatsapp/bridge_manager.rs` | Go child process lifecycle: spawn, health check, auto-restart, graceful shutdown |
| **BridgeRpcClient** | `core/src/gateway/interfaces/whatsapp/rpc_client.rs` | JSON-RPC client: connect Unix Socket, send commands, receive event stream |
| **PairingStateMachine** | `core/src/gateway/interfaces/whatsapp/pairing.rs` | Fine-grained pairing lifecycle: 9 states, triggers EventBus broadcasts |
| **WhatsAppChannel** | `core/src/gateway/interfaces/whatsapp/mod.rs` | Upgraded Channel trait impl, integrates above three components |
| **whatsapp-bridge** | `bridges/whatsapp/` | Go binary, thin whatsmeow wrapper, exposes JSON-RPC interface |

---

## 3. Pairing State Machine

### State Definition

```rust
pub enum PairingState {
    /// Initial: bridge not started
    Idle,
    /// Bridge process is starting
    Initializing,
    /// QR code generated, waiting for user scan
    WaitingQr {
        qr_data: String,
        generated_at: Instant,
        expires_at: Instant,    // typically 60s
    },
    /// QR code expired, waiting for bridge to push new one
    QrExpired,
    /// User scanned, waiting for phone confirmation
    Scanned,
    /// Handshake done, syncing encryption keys and contacts
    Syncing { progress: f32 },  // 0.0 ~ 1.0
    /// Fully connected, can send/receive
    Connected {
        device_name: String,
        phone_number: String,
    },
    /// Disconnected (reconnectable)
    Disconnected { reason: String },
    /// Unrecoverable error
    Failed { error: String },
}
```

### State Transitions

```
                    ┌──────────┐
                    │   Idle   │
                    └────┬─────┘
                         │ channel.start()
                    ┌────▼──────────┐
                    │ Initializing  │
                    └────┬──────────┘
                         │ bridge spawned + ready
                    ┌────▼──────────┐
              ┌────→│  WaitingQr   │←────────┐
              │     └────┬──────────┘         │
              │          │                    │
              │    ┌─────▼───────┐    ┌──────┴─────┐
              │    │   Scanned   │    │ QrExpired   │
              │    └─────┬───────┘    └────────────┘
              │          │ confirmed          ↑
              │    ┌─────▼───────┐            │
              │    │   Syncing   │     timeout (60s)
              │    └─────┬───────┘            │
              │          │ 100%          auto-refresh
              │    ┌─────▼───────┐            │
              │    │  Connected  │────────────┘
              │    └─────┬───────┘    (session expired)
              │          │
              │    ┌─────▼──────────┐
              └────│ Disconnected   │
                   └────────────────┘
```

### Mapping to ChannelStatus

| PairingState | ChannelStatus |
|---|---|
| Idle | Disconnected |
| Initializing | Connecting |
| WaitingQr / QrExpired / Scanned / Syncing | Connecting |
| Connected | Connected |
| Disconnected | Disconnected |
| Failed | Error |

### Event Broadcasting

Each state transition broadcasts a `TopicEvent` on `channels.whatsapp.pairing`:

```rust
pub struct PairingEvent {
    pub channel_id: String,
    pub state: PairingState,
    pub timestamp: DateTime<Utc>,
}
```

Dashboard subscribes via existing WebSocket mechanism for millisecond-level UI updates.

---

## 4. Bridge RPC Protocol

### Rust → Go (Commands)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `bridge.connect` | `{}` | `{ok: true}` | Initialize whatsmeow, begin pairing |
| `bridge.disconnect` | `{}` | `{ok: true}` | Disconnect and cleanup session |
| `bridge.send` | `{to, text, media?}` | `{id: "msg_id"}` | Send message |
| `bridge.status` | `{}` | `{connected, device?}` | Query connection state |
| `bridge.ping` | `{}` | `{pong: true, rtt_ms}` | Health check |

### Go → Rust (Event Push)

| Event | Data | Description |
|-------|------|-------------|
| `event.qr` | `{qr_data, expires_in_secs}` | New QR code generated |
| `event.qr_expired` | `{}` | QR code expired |
| `event.scanned` | `{}` | User scanned QR |
| `event.syncing` | `{progress: 0.0~1.0}` | Sync progress |
| `event.connected` | `{device_name, phone}` | Connection established |
| `event.disconnected` | `{reason}` | Connection lost |
| `event.message` | `{from, text, media?, timestamp}` | Inbound message |
| `event.receipt` | `{msg_id, type}` | Read/delivered receipt |

### Communication Flow

```
Rust (BridgeRpcClient)              Go (JSON-RPC Server)
  │                                    │
  │── {"method":"bridge.connect"} ────→│
  │                                    │  (whatsmeow init)
  │←── {"method":"event.qr"} ─────────│  (push QR)
  │                                    │
  │        (user scans on phone)       │
  │                                    │
  │←── {"method":"event.scanned"} ────│
  │←── {"method":"event.syncing"} ────│
  │←── {"method":"event.connected"} ──│
  │                                    │
  │── {"method":"bridge.send"} ───────→│
  │←── {"result": {"id":"msg_123"}} ──│
  │                                    │
  │←── {"method":"event.message"} ────│  (inbound msg)
```

### Go Bridge Project Structure

```
bridges/whatsapp/
├── go.mod
├── go.sum
├── main.go              # Entry, Unix Socket listener
├── rpc_server.go        # JSON-RPC handler
├── wa_client.go         # whatsmeow wrapper
├── message_converter.go # Message format conversion (minimal)
└── store/
    └── sqlite.go        # Session persistence
```

---

## 5. Dashboard UI Upgrade

### WhatsApp Panel Interaction Flow

```
┌─────────────────────────────────────────────────────────┐
│  Social Connections > WhatsApp                          │
│─────────────────────────────────────────────────────────│
│                                                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │ Status: ● Initializing...                         │  │
│  │ ═══════════════════════════░░░░░░░░░░░░░░░░░░░░░  │  │
│  └───────────────────────────────────────────────────┘  │
│                        ↓ (bridge ready)                  │
│  ┌───────────────────────────────────────────────────┐  │
│  │           ┌─────────────┐                         │  │
│  │           │  ██ QR ██   │   Scan to connect       │  │
│  │           │  ██ Code██  │                         │  │
│  │           │  ██████████ │   Expires in: 47s ━━░░  │  │
│  │           └─────────────┘                         │  │
│  │                                                   │  │
│  │   Or use pairing code: [Phone number] [Get Code]  │  │
│  └───────────────────────────────────────────────────┘  │
│                        ↓ (scanned)                      │
│  ┌───────────────────────────────────────────────────┐  │
│  │  ✓ Scanned! Syncing encryption keys...            │  │
│  │  ═══════════════════════════════════░░░░░░░░ 72%  │  │
│  └───────────────────────────────────────────────────┘  │
│                        ↓ (connected)                    │
│  ┌───────────────────────────────────────────────────┐  │
│  │  ● Connected                                      │  │
│  │  Device: iPhone 15 Pro                            │  │
│  │  Phone: +86 138****1234                           │  │
│  │  Uptime: 2h 15m                                   │  │
│  │                                                   │  │
│  │  [Disconnect]  [Re-pair]                          │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### Implementation Points

1. **Event subscription**: Dashboard subscribes to `channels.whatsapp.pairing` topic via existing WebSocket
2. **QR auto-refresh**: On `event.qr_expired`, automatically request new code — no user action needed
3. **Countdown animation**: Display remaining validity based on `expires_at`, computed locally
4. **Pairing code alternative**: Support whatsmeow's Pairing Code mode (phone number → 8-digit code)
5. **Serialization fix**: Unify `PairingData::QrCode` format between frontend and backend

### Leptos Component Structure

```
views/social_connections.rs
  ├── SocialConnections      (Tab container)
  ├── WhatsAppPanel          (upgraded)
  │    ├── PairingView       (QR + pairing code + status)
  │    ├── ConnectedView     (device info + action buttons)
  │    └── StatusIndicator   (status light + text)
  ├── TelegramPanel          (token input + validation)
  └── DiscordPanel           (token input + validation)
```

---

## 6. Data Flow

### Inbound Messages

```
WhatsApp user → whatsmeow → event.message → Unix Socket
→ BridgeRpcClient → InboundMessage conversion → inbound_tx
→ ChannelRegistry → Agent Loop
```

### Outbound Messages

```
Agent → ReplyEmitter → OutboundMessage → ChannelRegistry.send()
→ WhatsAppChannel.send() → bridge.send RPC → Unix Socket
→ Go Bridge → whatsmeow → WhatsApp user
```

---

## 7. Error Handling

| Scenario | Strategy |
|----------|----------|
| Go Bridge process crash | BridgeManager detects exit code, auto-restart after 3s, max 5 retries. Enters `Failed` state after exhaustion |
| Unix Socket disconnected | BridgeRpcClient retries 3 times (1s/2s/4s exponential backoff), triggers Bridge restart on failure |
| QR scan timeout | Bridge auto-requests new QR, Rust receives `event.qr`, Dashboard refreshes seamlessly |
| WhatsApp Session expired | Bridge pushes `event.disconnected`, PairingStateMachine returns to `Idle`, Dashboard prompts re-scan |
| Message send failure | `bridge.send` returns error, WhatsAppChannel returns `Err`, Dispatcher handles retry |
| Go Bridge binary missing | `channel.start()` returns explicit error, Dashboard shows installation guide |

---

## 8. Security

- **Session data**: Stored in `~/.aleph/whatsapp/`, file permissions 600
- **Process isolation**: Go Bridge runs as same user, no elevated privileges needed
- **Socket permissions**: Unix Socket file permissions 600, local-only access
- **No third-party relay**: All decryption happens locally, E2EE principle maintained

---

## 9. Testing Strategy

| Level | Method |
|-------|--------|
| BridgeRpcClient unit tests | Mock Unix Socket, verify JSON-RPC serialization |
| PairingStateMachine unit tests | Verify all state transition paths, ensure no illegal transitions |
| WhatsAppChannel integration tests | Start real Go Bridge (test mode), verify end-to-end pairing flow |
| Dashboard UI tests | Mock WebSocket events, verify UI state rendering |

---

## 10. File Structure

```
core/src/gateway/interfaces/whatsapp/
├── mod.rs                 # WhatsAppChannel (upgraded)
├── config.rs              # WhatsAppConfig (existing)
├── bridge_manager.rs      # Child process lifecycle (new)
├── rpc_client.rs          # JSON-RPC over Unix Socket (new)
├── pairing.rs             # PairingStateMachine (new)
├── message.rs             # Message format conversion (new)
└── factory.rs             # WhatsAppChannelFactory (existing, upgraded)

bridges/whatsapp/
├── go.mod
├── go.sum
├── main.go
├── rpc_server.go
├── wa_client.go
├── message_converter.go
└── store/
    └── sqlite.go

apps/dashboard/src/views/
└── social_connections.rs  # Upgraded WhatsAppPanel
```

---

## 11. Future Extensions

This Thin Sidecar pattern establishes a reusable template for future QR-based channels:

- **WeChat**: Same pattern, different Go bridge library
- **LINE**: Same pattern, different protocol stack
- **Multi-account**: BridgeManager supports spawning multiple Bridge instances with different IDs
- **Message interception**: PairingStateMachine can be extended with approval workflow states
