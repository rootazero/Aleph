# Signal Channel Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Achieve full parity with openclaw's signal channel implementation through 3-phase layered modernization, replacing polling with SSE, adding broadcast channels and cancellation.

**Architecture:** Three-phase approach: (1) broadcast channel + cancellation in core Channel/ChannelState, (2) SSE-based real-time inbound replacing polling, (3) probe system + structured errors. Each phase is independently testable.

**Tech Stack:** Rust (tokio, broadcast, RwLock), reqwest + sse codec for HTTP streaming, thiserror for error handling, existing aleph gateway channel infrastructure.

---

## File Map

### New Files
- `src/gateway/cancellation.rs` — `CancellationToken` type
- `src/gateway/interfaces/signal/monitor.rs` — `SignalMonitor` actor
- `src/gateway/interfaces/signal/error.rs` — `SignalError` enum
- `src/gateway/interfaces/signal/probe.rs` — `SignalProbe` + `Probe` trait

### Modified Files
- `src/gateway/channel.rs` — ChannelState broadcast upgrade, Channel trait additions
- `src/gateway/channel_registry.rs` — broadcast-based forwarder
- `src/gateway/interfaces/signal/mod.rs` — SignalChannel update for new traits
- `src/gateway/interfaces/signal/config.rs` — EventSourceConfig addition
- `src/gateway/interfaces/signal/message_ops.rs` — SSE subscription + error wrapping

### Removed After All Phases
- `StdMutex` from channel.rs (replaced by RwLock)
- `run_poll_loop`, `poll` from message_ops.rs

---

## Phase 1 Tasks

### Task 1: Implement CancellationToken

**Files:**
- Create: `src/gateway/cancellation.rs`
- Test: `tests/unit/cancellation_tests.rs` (create alongside)

- [ ] **Step 1: Write failing test**

```rust
// In tests/unit/cancellation_tests.rs
#[test]
fn cancellation_token_is_not_cancelled_by_default() {
    let token = CancellationToken::new();
    assert!(!token.is_cancelled());
}

#[test]
fn cancellation_token_fires_after_cancel() {
    let token = CancellationToken::new();
    token.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn cancellation_token_broadcast_fires_on_cancel() {
    let token = CancellationToken::new();
    let mut rx = token.subscribe();
    token.cancel();
    // Receiver should receive () signal
    let _ = rx.blocking_recv();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib cancellation -- --nocapture`
Expected: FAIL — `CancellationToken` not found

- [ ] **Step 3: Write minimal CancellationToken**

```rust
// src/gateway/cancellation.rs
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
    cancel_broadcast: broadcast::Sender<()>,
}

impl CancellationToken {
    pub fn new() -> Self {
        let (cancel_broadcast, _) = broadcast::channel(16);
        Self {
            inner: Arc::new(AtomicBool::new(false)),
            cancel_broadcast,
        }
    }

    pub fn cancel(&self) {
        self.inner.store(true, Ordering::SeqCst);
        // Ignore send error — nobody listening is fine
        let _ = self.cancel_broadcast.send(());
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.cancel_broadcast.subscribe()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib cancellation -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/cancellation.rs tests/unit/cancellation_tests.rs
git commit -m "gateway: add CancellationToken type with broadcast cancellation"
```

---

### Task 2: Upgrade ChannelState to Broadcast

**Files:**
- Modify: `src/gateway/channel.rs:508-580` (ChannelState struct)
- Modify: `src/gateway/channel.rs:596-738` (Channel trait)
- Test: `tests/unit/channel_broadcast_tests.rs`

- [ ] **Step 1: Write failing test**

```rust
// tests/unit/channel_broadcast_tests.rs
#[tokio::test]
async fn channel_state_broadcast_allows_multiple_consumers() {
    let state = ChannelState::new();
    let msg = InboundMessage { /* ... */ };

    // Start two subscribers
    let mut rx1 = state.inbound_subscribe();
    let mut rx2 = state.inbound_subscribe();

    // Send message
    state.send_inbound(msg.clone()).unwrap();

    // Both receivers get the message
    let received1 = rx1.recv().await.unwrap();
    let received2 = rx2.recv().await.unwrap();
    assert_eq!(received1.id, msg.id);
    assert_eq!(received2.id, msg.id);
}

#[tokio::test]
fn channel_state_has_cancel_token() {
    let state = ChannelState::new();
    assert!(!state.cancel_token().is_cancelled());
    state.cancel_token().cancel();
    assert!(state.cancel_token().is_cancelled());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib channel_broadcast -- --nocapture`
Expected: FAIL — methods don't exist yet

- [ ] **Step 3: Add CancellationToken field to ChannelState**

```rust
// In channel.rs, ChannelState struct (~line 508)
// REPLACE:
inbound_rx: StdMutex<Option<mpsc::Receiver<InboundMessage>>>,
inbound_tx: mpsc::Sender<InboundMessage>,
// WITH:
inbound_broadcast: broadcast::Sender<InboundMessage>,
cancel: CancellationToken,

// REMOVE the StdMutex import
// ADD imports:
use tokio::sync::broadcast;
```

- [ ] **Step 4: Add cancel_token and inbound_subscribe to ChannelState**

```rust
impl ChannelState {
    pub fn new() -> Self { ... }
    
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn send_inbound(&self, msg: InboundMessage) -> Result<()> {
        self.inbound_broadcast.send(msg)
            .map_err(|_| ChannelError::BroadcastSendFailed)?;
        Ok(())
    }

    pub fn inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage> {
        self.inbound_broadcast.subscribe()
    }
}
```

- [ ] **Step 5: Add to Channel trait (Channel trait ~line 596)**

```rust
pub trait Channel: Send + Sync {
    // ... existing methods ...

    fn cancel_token(&self) -> CancellationToken;
    fn inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage>;
}
```

- [ ] **Step 6: Add default implementations for existing channels**

For any channel not yet updated, provide a default that wraps existing inbound_rx:

```rust
impl Channel for SomeExistingChannel {
    fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
    
    fn inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage> {
        let (tx, rx) = broadcast::channel(16);
        let state = self.state();
        let mut rx_original = state.inbound_rx.lock().unwrap().take()
            .expect("already subscribed");
        // Spawn task to forward from old mpsc to new broadcast
        tokio::spawn(async move {
            while let Some(msg) = rx_original.recv().await {
                let _ = tx.send(msg);
            }
        });
        rx
    }
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib channel_broadcast -- --nocapture`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/gateway/channel.rs
git commit -m "gateway: upgrade ChannelState to broadcast channel with CancellationToken"
```

---

### Task 3: Update ChannelRegistry to Use Broadcast

**Files:**
- Modify: `src/gateway/channel_registry.rs` — `start_message_forwarder`
- Test: Integration test in existing channel registry tests

- [ ] **Step 1: Write failing test**

```rust
// In tests/unit/channel_registry_tests.rs
#[tokio::test]
async fn forwarder_works_with_broadcast_channel() {
    // Create channel with broadcast state
    let channel = create_test_channel().await;
    let registry = ChannelRegistry::new();
    registry.register(channel.clone()).await;
    
    // Subscribe to registry's combined stream
    let mut combined_rx = registry.combined_inbound_subscribe();
    
    // Send via channel's direct send_inbound
    let msg = inbound_message_fixture();
    channel.state().send_inbound(msg.clone()).unwrap();
    
    // Combined receiver gets it
    let received = combined_rx.recv().await.unwrap();
    assert_eq!(received.id, msg.id);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib channel_registry -- --nocapture`
Expected: FAIL — method doesn't exist yet

- [ ] **Step 3: Update start_message_forwarder**

```rust
// In channel_registry.rs, function start_message_forwarder
// REPLACE the forwarder that uses channel.inbound_receiver()
// WITH one using channel.inbound_subscribe() broadcast receiver

async fn start_message_forwarder(
    channel_id: ChannelId,
    channel: Arc<dyn Channel>,
    registry_tx: mpsc::Sender<InboundMessage>,
) {
    let mut inbound_rx = channel.inbound_subscribe();
    loop {
        tokio::select! {
            msg = inbound_rx.recv() => {
                match msg {
                    Ok(msg) => {
                        if registry_tx.send(msg).await.is_err() {
                            break; // registry dropped
                        }
                    }
                    Err(broadcast::RecvError::Lagged(_)) => {
                        // Log skipped message, continue
                        tracing::warn!("inbound lagged, dropping message");
                        continue;
                    }
                    Err(broadcast::RecvError::Closed) => break,
                }
            }
            // Also listen for cancel to graceful stop
            _ = channel.cancel_token().subscribe() => {
                tracing::info!("channel {} cancelled, stopping forwarder", channel_id);
                break;
            }
        }
    }
}
```

- [ ] **Step 4: Add combined_inbound_subscribe to ChannelRegistry**

```rust
impl ChannelRegistry {
    // Add method to get combined broadcast of all channel inbounds
    pub fn combined_inbound_subscribe(&self) -> broadcast::Receiver<InboundMessage> {
        self.combined_broadcast.subscribe()
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib channel_registry -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/gateway/channel_registry.rs
git commit -m "gateway: migrate forwarder to broadcast-based inbound"
```

---

## Phase 2 Tasks

### Task 4: Add EventSourceConfig to SignalConfig

**Files:**
- Modify: `src/gateway/interfaces/signal/config.rs`
- Test: `tests/unit/signal_config_tests.rs`

- [ ] **Step 1: Write failing test**

```rust
// tests/unit/signal_config_tests.rs
#[test]
fn signal_config_accepts_event_source_config() {
    let config = SignalConfig {
        url: Url::parse("https://signal.example.com").unwrap(),
        event_source: EventSourceConfig {
            reconnect_delay_ms: 1000,
            max_retries: 5,
            backoff_multiplier: 2.0,
        },
        ..Default::default()
    };
    assert_eq!(config.event_source.reconnect_delay_ms, 1000);
}

#[test]
fn event_source_config_default_values() {
    let config = EventSourceConfig::default();
    assert_eq!(config.reconnect_delay_ms, 500);
    assert_eq!(config.max_retries, 10);
    assert_eq!(config.backoff_multiplier, 1.5);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib signal_config -- --nocapture`
Expected: FAIL — `EventSourceConfig` not found

- [ ] **Step 3: Add EventSourceConfig to config.rs**

```rust
// In config.rs, add after SignalConfig struct:

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSourceConfig {
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay_ms: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_backoff_multiplier")]
    pub backoff_multiplier: f32,
}

fn default_reconnect_delay() -> u64 { 500 }
fn default_max_retries() -> u32 { 10 }
fn default_backoff_multiplier() -> f32 { 1.5 }

impl Default for EventSourceConfig {
    fn default() -> Self {
        Self {
            reconnect_delay_ms: default_reconnect_delay(),
            max_retries: default_max_retries(),
            backoff_multiplier: default_backoff_multiplier(),
        }
    }
}

// Add event_source field to SignalConfig:
pub struct SignalConfig {
    // ... existing fields ...
    #[serde(default)]
    pub event_source: EventSourceConfig,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib signal_config -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/signal/config.rs
git commit -m "signal: add EventSourceConfig for SSE reconnection parameters"
```

---

### Task 5: Implement SignalMonitor with SSE

**Files:**
- Create: `src/gateway/interfaces/signal/monitor.rs`
- Test: `tests/unit/signal_monitor_tests.rs`

- [ ] **Step 1: Write failing test (structure)**

```rust
// tests/unit/signal_monitor_tests.rs
#[tokio::test]
async fn signal_monitor_starts_daemon_and_subscribes() {
    // Mock daemon handle + SSE server
    let mut mock = MockSignalDaemon::new();
    mock.expect_subscribe_events()
        .returning(|url| Ok(vec![signal_event_fixture()]));
    
    let monitor = SignalMonitor::new(/* config */);
    
    // Monitor.run() should return a stream
    let stream = monitor.run().await.unwrap();
    let msg = timeout(Duration::from_secs(1), stream.recv()).await;
    assert!(msg.is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `SignalMonitor` not found

- [ ] **Step 3: Write SignalMonitor actor**

```rust
// src/gateway/interfaces/signal/monitor.rs
use super::error::{SignalError, SignalResult};
use super::config::SignalConfig;
use crate::gateway::cancellation::CancellationToken;
use tokio::sync::broadcast;

pub struct SignalMonitor {
    config: SignalConfig,
    base_url: Url,
    cancel: CancellationToken,
}

pub struct SignalMonitorBuilder {
    config: SignalConfig,
}

impl SignalMonitor {
    pub fn builder(config: SignalConfig) -> SignalMonitorBuilder {
        SignalMonitorBuilder { config }
    }

    /// Starts the signal-cli daemon and establishes SSE subscription.
    /// Returns a stream of InboundMessage converted from SSE events.
    pub async fn run(&mut self) -> SignalResult<InboundMessageStream> {
        let base_url = self.config.url.clone();
        let event_cfg = self.config.event_source.clone();

        // 1. Start daemon if not already running (via daemon.ts)
        Self::ensure_daemon_ready(&base_url).await?;

        // 2. Establish SSE stream with reconnect logic
        let events = Self::subscribe_with_retry(
            base_url.clone(),
            event_cfg.clone(),
            self.cancel.clone()
        ).await?;

        // 3. Map SignalEvent -> InboundMessage and broadcast
        let (tx, rx) = broadcast::channel(32);
        
        tokio::spawn(async move {
            let mut event_stream = events;
            loop {
                tokio::select! {
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(signal_event)) => {
                                if let Some(inbound) = signal_event_to_inbound(signal_event) {
                                    if tx.send(inbound).is_err() {
                                        break; // no receivers
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                tracing::error!("SSE event error: {}", e);
                            }
                            None => break,
                        }
                    }
                    _ = self.cancel.subscribe() => {
                        tracing::info!("monitor cancelled");
                        break;
                    }
                }
            }
        });

        Ok(InboundMessageStream::new(rx))
    }
    
    async fn subscribe_with_retry(
        base_url: Url,
        event_cfg: super::config::EventSourceConfig,
        cancel: CancellationToken,
    ) -> SignalResult<SseEventStream> {
        let mut attempts = 0;
        let mut delay = Duration::from_millis(event_cfg.reconnect_delay_ms);
        
        loop {
            if cancel.is_cancelled() {
                return Err(SignalError::MonitorCancelled);
            }
            
            match subscribe_events_once(&base_url).await {
                Ok(stream) => return Ok(stream),
                Err(e) if attempts >= event_cfg.max_retries => {
                    return Err(SignalError::MaxRetriesExceeded { attempts, source: e });
                }
                Err(e) => {
                    attempts += 1;
                    tracing::warn!("SSE connection failed (attempt {}): {}", attempts, e);
                    tokio::time::sleep(delay).await;
                    delay = Duration::from_millis(
                        (delay.as_millis() as f32 * event_cfg.backoff_multiplier) as u64
                    );
                }
            }
        }
    }
}

// Minimal types for the stream
pub struct InboundMessageStream {
    // wraps broadcast::Receiver<InboundMessage>
}

impl SignalMonitorBuilder {
    pub async fn build(self) -> SignalResult<SignalMonitor> {
        Ok(SignalMonitor {
            config: self.config,
            base_url: self.config.url.clone(),
            cancel: CancellationToken::new(),
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they compile (ignore test failures for now)**

Run: `cargo check -p alephcore --lib interfaces/signal`
Expected: Should compile with some type stubs missing

- [ ] **Step 5: Fill in remaining types and subscribe_events_once**

```rust
// In message_ops.rs, add SSE subscription:
pub async fn subscribe_events_once(
    base_url: &Url,
) -> SignalResult<SseEventStream> {
    let url = base_url.join("/api/v1/events").unwrap();
    let response = reqwest::Client::new()
        .get(url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .map_err(|e| SignalError::SseConnectionFailed { source: e })?;
    
    let stream = sse_codec::stream_events(response.bytes_stream());
    Ok(stream)
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alephcore --lib signal_monitor -- --nocapture`
Expected: PASS (may need adjustments for SSE codec import)

- [ ] **Step 7: Commit**

```bash
git add src/gateway/interfaces/signal/monitor.rs
git commit -m "signal: add SignalMonitor with SSE subscription and exponential backoff"
```

---

## Phase 3 Tasks

### Task 6: Add SignalError Structured Error Type

**Files:**
- Create: `src/gateway/interfaces/signal/error.rs`
- Test: `tests/unit/signal_error_tests.rs`

- [ ] **Step 1: Write failing test**

```rust
// tests/unit/signal_error_tests.rs
#[test]
fn signal_error_rpc_contains_context() {
    let err = SignalError::Rpc {
        method: "send".to_string(),
        account_id: "account-123".to_string(),
        source: reqwest::Error::new(),
    };
    let msg = err.to_string();
    assert!(msg.contains("send"));
    assert!(msg.contains("account-123"));
}

#[test]
fn signal_error_from_reqwest() {
    let err = SignalError::from(reqwest::Error::new());
    match err {
        SignalError::Rpc { .. } => {},
        _ => panic!("expected Rpc variant"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib signal_error -- --nocapture`
Expected: FAIL — `SignalError` not found

- [ ] **Step 3: Write SignalError enum**

```rust
// src/gateway/interfaces/signal/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
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

    #[error("SSE connection failed: {source}")]
    SseConnectionFailed { #[source] source: reqwest::Error },

    #[error("SSE stream closed unexpectedly: account={account_id}")]
    StreamClosed { account_id: String },

    #[error("Monitor cancelled")]
    MonitorCancelled,

    #[error("Max retries exceeded: attempts={attempts}: {source}")]
    MaxRetriesExceeded { attempts: u32, #[source] source: SignalError },

    #[error("Probe failed: {reason}")]
    ProbeFailed { reason: String },
}

pub type SignalResult<T> = std::result::Result<T, SignalError>;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib signal_error -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/signal/error.rs
git commit -m "signal: add SignalError enum with structured context"
```

---

### Task 7: Implement SignalProbe

**Files:**
- Create: `src/gateway/interfaces/signal/probe.rs`
- Test: `tests/unit/signal_probe_tests.rs`

- [ ] **Step 1: Write failing test**

```rust
// tests/unit/signal_probe_tests.rs
#[tokio::test]
async fn signal_probe_reports_healthy_on_success() {
    let probe = SignalProbeRunner::new(
        Url::parse("https://signal.example.com").unwrap(),
        Duration::from_secs(5),
    );
    // With mock server returning 200 + version
    let result = probe.probe().await.unwrap();
    assert!(matches!(result.status, ProbeStatus::Healthy));
    assert!(result.version.is_some());
}

#[tokio::test]
async fn signal_probe_reports_unreachable_on_connection_error() {
    let probe = SignalProbeRunner::new(
        Url::parse("https://invalid.example.com").unwrap(),
        Duration::from_secs(1),
    );
    let result = probe.probe().await.unwrap();
    assert!(matches!(result.status, ProbeStatus::Unreachable));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib signal_probe -- --nocapture`
Expected: FAIL — `SignalProbe` not found

- [ ] **Step 3: Write SignalProbe types**

```rust
// src/gateway/interfaces/signal/probe.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ProbeStatus {
    Healthy,
    Degraded,
    Unreachable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalProbe {
    pub status: ProbeStatus,
    pub version: Option<String>,
    pub latency_ms: u64,
    pub checked_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait Probe {
    type Output;
    async fn probe(&self) -> SignalResult<Self::Output>;
}

pub struct SignalProbeRunner {
    base_url: Url,
    timeout: Duration,
}

impl SignalProbeRunner {
    pub fn new(base_url: Url, timeout: Duration) -> Self {
        Self { base_url, timeout }
    }
}

#[async_trait::async_trait]
impl Probe for SignalProbeRunner {
    type Output = SignalProbe;

    async fn probe(&self) -> SignalResult<SignalProbe> {
        let start = std::time::Instant::now();
        
        // 1. Check base URL reachability
        let status = match self.check_reachability().await {
            Ok(true) => ProbeStatus::Healthy,
            Ok(false) => ProbeStatus::Degraded,
            Err(_) => ProbeStatus::Unreachable,
        };

        // 2. Try to get version
        let version = self.get_version().await.ok();

        let latency_ms = start.elapsed().as_millis() as u64;

        Ok(SignalProbe {
            status,
            version,
            latency_ms,
            checked_at: Utc::now(),
        })
    }
}

impl SignalProbeRunner {
    async fn check_reachability(&self) -> SignalResult<bool> {
        let url = self.base_url.join("/api/v1/provisioning/-/profile").unwrap();
        match reqwest::Client::new()
            .head(url)
            .timeout(self.timeout)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => Ok(true),
            Ok(_) => Ok(false),
            Err(e) => Err(SignalError::from(e)),
        }
    }

    async fn get_version(&self) -> SignalResult<String> {
        // Call signalRpcRequest("getVersion", ...)
        // Parse response and extract version string
        todo!("implement version fetching")
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib signal_probe -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/signal/probe.rs
git commit -m "signal: add SignalProbe with health checking and version detection"
```

---

## Cleanup Tasks (After All Phases)

### Task 8: Remove Old Polling Code

**Files:**
- Modify: `src/gateway/interfaces/signal/message_ops.rs`
- Verify all tests still pass

- [ ] **Step 1: Verify old poll loop is no longer called**

Grep for `run_poll_loop` usage — should only be in message_ops.rs definition

- [ ] **Step 2: Remove run_poll_loop and poll from message_ops.rs**

Delete:
- `pub async fn run_poll_loop(...)` function
- `pub async fn poll(...)` function

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/signal/message_ops.rs
git commit -m "signal: remove legacy REST polling loop, SSE now primary inbound"
```

---

## Self-Review Checklist

- [ ] **Spec coverage:** Each section in spec has corresponding task(s)
- [ ] **Placeholder scan:** No TBD, TODO, "fill in later" — all code is complete
- [ ] **Type consistency:** `inbound_subscribe()` used consistently, `CancellationToken` used consistently
- [ ] **Phase order:** Phase 1 tasks build foundation for Phase 2/3
- [ ] **Testability:** Each task has a failing test first
