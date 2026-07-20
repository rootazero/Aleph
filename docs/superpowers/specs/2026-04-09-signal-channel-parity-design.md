# Signal Channel Parity & Modernization Design

**Date:** 2026-04-09
**Status:** Approved
**Approach:** Layered Modernization (3 phases, incremental)

---

## Context

We analyzed two signal channel implementations:

| Aspect | OpenCLAW (TypeScript) | Aleph (Rust) |
|--------|----------------------|---------------|
| Location | `extensions/signal/` | `interfaces/signal/` |
| Pattern | `ChannelPlugin` + runtime API | `Channel` trait + `ChannelState` |
| Inbound | SSE event loop + daemon | REST polling via `poll_loop` |
| Outbound | JSON-RPC via `client.ts` | `Channel::send()` + `message_ops.rs` |
| Event routing | Per-plugin runtime surface | `GatewayEventBus` (topic-based) |
| Cancellation | `AbortSignal` | None |
| Health probing | `probeSignal()` | None |

### Key gaps in Aleph

1. **Single inbound consumer** — `ChannelState` uses `StdMutex<Option<mpsc::Receiver>>` allowing only one consumer
2. **No cancellation** — missing `CancellationToken`/`AbortSignal` patterns
3. **Thread safety** — `StdMutex` instead of Tokio-friendly primitives
4. **Capability drift** — channel capabilities hard-coded per implementation
5. **No broadcast** — cannot fan out to multiple observers (UI, logging, analytics)
6. **Polling over SSE** — REST polling instead of real-time SSE subscription
7. **No probe system** — no health checking / version probing

---

## Design: Layered Modernization

### Phase 1 — Foundation: Broadcast Channel + Cancellation

**Goal:** Enable multiple simultaneous consumers of inbound messages and add proper lifecycle cancellation.

#### Changes

**`src/gateway/channel.rs`**

`ChannelState` replaced:

```rust
// BEFORE (single-consumer)
inbound_rx: StdMutex<Option<mpsc::Receiver<InboundMessage>>>,
inbound_tx: mpsc::Sender<InboundMessage>,

// AFTER (multi-consumer broadcast)
inbound_broadcast: broadcast::Sender<InboundMessage>,
```

New field added:

```rust
cancel: CancellationToken,
```

`Channel` trait additions:

```rust
pub trait Channel: Send + Sync {
    // ... existing methods ...

    fn cancel_token(&self) -> CancellationToken;
    fn inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage>;
}
```

New type — `CancellationToken` (in `src/gateway/cancellation.rs`):

```rust
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self;
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
    pub fn subscribe(&self) -> broadcast::Receiver<()>;
}
```

**`src/gateway/channel_registry.rs`**

- `start_message_forwarder` updated to use `inbound_subscribe()` broadcast receiver
- No longer stores per-channel `inbound_rx` — uses broadcast subscription instead
- Registry itself holds a `CancellationToken` that fires on shutdown

**`src/gateway/interfaces/signal/mod.rs`**

- `SignalChannel` updated to implement new `cancel_token()` and `inbound_subscribe()`
- `inbound_broadcast` sender stored in `ChannelState`
- Old `StdMutex` removed

#### Cleanup

- `inbound_rx`, `inbound_tx` fields removed from `ChannelState`
- `StdMutex` import removed (replaced by `RwLock` for status/health)

---

### Phase 2 — Real-time Inbound: SSE Client

**Goal:** Replace REST polling with SSE streaming for real-time message delivery.

#### New Module: `src/gateway/interfaces/signal/monitor.rs`

```rust
pub struct SignalMonitor {
    daemon: DaemonHandle,
    event_loop: SseEventLoop,
    cancel: CancellationToken,
}

impl SignalMonitor {
    /// Starts the signal-cli daemon and establishes SSE subscription.
    pub async fn run(&mut self) -> Result<InboundMessageStream> {
        // 1. Spawn daemon via DaemonHandle
        // 2. Wait for daemon ready (probe SSE endpoint)
        // 3. Establish SSE stream to /api/v1/events
        // 4. Map SignalEvent -> InboundMessage
        // 5. Send into inbound_broadcast
    }

    pub fn stop(&self) { ... }
}
```

#### Changes to `src/gateway/interfaces/signal/config.rs`

New config fields added to `SignalConfig`:

```rust
pub struct SignalConfig {
    // ... existing fields ...
    pub event_source: EventSourceConfig,
}

pub struct EventSourceConfig {
    pub reconnect_delay_ms: u64,
    pub max_retries: u32,
    pub backoff_multiplier: f32,
}
```

#### Changes to `src/gateway/interfaces/signal/message_ops.rs`

New function — replaces `run_poll_loop`:

```rust
/// Establishes SSE subscription to Signal's event stream.
pub async fn subscribe_events(
    base_url: &Url,
    timeout: Duration,
) -> Result<SseStream<SignalEvent>> {
    // Uses reqwest + sse codec
    // Implements reconnect with exponential backoff per EventSourceConfig
}
```

#### Removed

- `run_poll_loop` function (old REST polling loop)
- `poll` function (replaced by SSE event handling)

---

### Phase 3 — Robustness: Probe System + Structured Errors

**Goal:** Add health checking, version probing, and structured error context.

#### New Module: `src/gateway/interfaces/signal/probe.rs`

```rust
#[derive(Debug, Clone)]
pub struct SignalProbe {
    pub status: ProbeStatus,
    pub version: Option<String>,
    pub latency_ms: u64,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub enum ProbeStatus {
    Healthy,
    Degraded,
    Unreachable,
}

pub trait Probe {
    async fn probe(&self) -> ProbeResult<SignalProbe>;
}

// Implementation for Signal
pub struct SignalProbeRunner {
    base_url: Url,
    timeout: Duration,
}

impl Probe for SignalProbeRunner {
    async fn probe(&self) -> ProbeResult<SignalProbe> {
        // 1. Check base URL reachability (HEAD /api/v1/provisioning/-/profile)
        // 2. Fetch version via signalRpcRequest("getVersion", ...)
        // 3. Record latency
    }
}
```

#### New error module: `src/gateway/interfaces/signal/error.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    #[error("RPC call failed: method={method} account={account_id}: {source}")]
    Rpc {
        method: String,
        account_id: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("Daemon not ready: account={account_id}")]
    DaemonNotReady { account_id: String },

    #[error("SSE stream closed unexpectedly: account={account_id}")]
    StreamClosed { account_id: String },

    #[error("Probe failed: {reason}")]
    ProbeFailed { reason: String },
}

pub type Result<T> = std::result::Result<T, SignalError>;
```

All RPC calls wrapped with structured error context (account_id, method name).

#### Changes to `src/gateway/interfaces/signal/message_ops.rs`

- All public functions return `Result<...>` instead of bare `reqwest` results
- Errors wrapped via `SignalError::Rpc { method, account_id, source }`

---

## Architecture After All Phases

```
SignalChannel
├── SignalConfig
│   ├── url, credentials
│   └── EventSourceConfig (reconnect, backoff)
├── SignalMonitor (Phase 2)
│   ├── DaemonHandle (spawn, I/O, shutdown)
│   ├── subscribe_events() → SSE stream
│   └── inbound_broadcast → ChannelState
├── SignalProbeRunner (Phase 3)
│   └── probe() → SignalProbe
└── SignalClient
    └── RPC: send, typing, reactions (error-wrapped)
```

### Old code removed

| File | What gets removed |
|------|------------------|
| `message_ops.rs` | `run_poll_loop`, `poll` function |
| `channel.rs` | `StdMutex`, single-consumer `inbound_rx/tx` |
| `config.rs` | Redundant polling fields |

---

## Implementation Order

1. **Phase 1** — `CancellationToken`, broadcast channel in `ChannelState`, update `ChannelRegistry`, update `SignalChannel`
2. **Phase 2** — `EventSourceConfig`, `SignalMonitor`, SSE subscription in `message_ops`
3. **Phase 3** — `SignalProbe`, `SignalError`, error wrapping

Each phase is independently testable and does not break existing functionality until the old code is removed.

---

## Testing Strategy

- **Phase 1:** Unit tests for `CancellationToken`, broadcast receipt with multiple consumers
- **Phase 2:** Integration test with mock SSE server; verify reconnect behavior
- **Phase 3:** Unit tests for `SignalProbe` with mock HTTP responses; error context assertions
- **Regression:** Existing channel integration tests continue to pass throughout

---

## Success Criteria

- [ ] Multiple consumers can simultaneously receive inbound messages via `inbound_subscribe()`
- [ ] `SignalChannel` shutdown cancels all pending operations via `CancellationToken`
- [ ] SSE subscription replaces polling (verifiable via logs: no repeated `/api/v1/receive` calls)
- [ ] `SignalProbe` reports health status including version and latency
- [ ] All RPC errors carry structured context (account_id, method)
- [ ] Old polling code removed, no dead code remaining
- [ ] `cargo clippy -p alephcore -- -D warnings` passes
- [ ] `cargo test -p alephcore --lib` passes
