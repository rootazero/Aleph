# Channel Infrastructure Fix Design

**Date:** 2026-03-03
**Status:** Approved
**Scope:** Basic Three — fix `status()`, `inbound_receiver()`, `send()` for all 14 channels

## Problem

During Telegram channel debugging, multiple systemic bugs were discovered in the Channel trait infrastructure:

1. **Stale `status()`**: Trait default reads `self.info().status` — a static snapshot set at construction time. 7+ channels never update this, so `ChannelRegistry::send()` rejects all messages ("not connected").

2. **Broken `inbound_receiver()`**: Discord, Slack, Email, and others return `None` — the message forwarder never starts, so inbound messages are silently dropped.

3. **Unsafe status mutation**: iMessage and CLI directly mutate `info.status` without any synchronization primitive, which is unsound under concurrent access.

4. **Overly strict send guard**: `ChannelRegistry::send()` rejects anything not `Connected`, but with the stale status bug this blocks all channels.

Only Telegram has all three correct (using `RwLock<ChannelStatus>` + `Mutex<Option<Receiver>>` + `try_read()`).

## Approach: Trait-Level Fix with ChannelState Helper

Extract the correct Telegram pattern into a reusable `ChannelState` struct, modify the Channel trait to use it as the source of truth, and mechanically migrate all 14 channels.

## Design

### 1. `ChannelState` Struct

New struct in `core/src/gateway/channel.rs`:

```rust
use std::sync::Mutex as StdMutex;
use tokio::sync::RwLock;
use std::sync::Arc;

pub struct ChannelState {
    status: Arc<RwLock<ChannelStatus>>,
    inbound_rx: StdMutex<Option<mpsc::Receiver<InboundMessage>>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl ChannelState {
    pub fn new(buffer_size: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer_size);
        Self {
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
            inbound_rx: StdMutex::new(Some(rx)),
            inbound_tx: tx,
        }
    }

    pub fn status(&self) -> ChannelStatus {
        self.status.try_read().map(|s| *s).unwrap_or(ChannelStatus::Connecting)
    }

    pub async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }

    pub fn take_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.inbound_rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }
}
```

**Key decisions:**
- `status`: `RwLock` + `try_read()` — non-blocking reads, async writes
- `inbound_rx`: `std::sync::Mutex` — because `inbound_receiver()` is sync in the trait
- `buffer_size` parameterized — default 1000, adjustable per channel

### 2. Channel Trait Changes

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    fn info(&self) -> &ChannelInfo;

    // NEW — required method
    fn state(&self) -> &ChannelState;

    fn status(&self) -> ChannelStatus {
        self.state().status()  // was: self.info().status
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.state().take_receiver()  // was: each channel's own impl
    }

    // start(), stop(), send() unchanged — each channel implements
    // Other default methods unchanged
}
```

### 3. Channel Migration Pattern

For each of the 14 channels:

1. Add `state: ChannelState` field
2. Implement `fn state(&self) -> &ChannelState { &self.state }`
3. In constructor: `state: ChannelState::new(1000)`
4. In `start()`: `self.state.set_status(ChannelStatus::Connected).await`
5. In `stop()`: `self.state.set_status(ChannelStatus::Disconnected).await`
6. Delete custom `status()` / `inbound_receiver()` overrides
7. In message receive loop: `self.state.sender().send(msg).await`

**Channels and migration complexity:**

| Channel | Current Bug | Effort |
|---------|------------|--------|
| Telegram | None (gold standard) | Small — replace own RwLock+Mutex with ChannelState |
| iMessage | Unsafe `info.status =` | Small |
| CLI | Unsafe `info.status =` | Small |
| Discord | receiver=None, stale status | Medium |
| Slack | receiver=None, stale status | Medium |
| Email | receiver=None, stale status | Medium |
| WhatsApp | Stale status | Small |
| Matrix | Stale status | Small |
| Signal | Stale status | Small |
| Mattermost | Stale status | Small |
| IRC | Stale status | Small |
| Webhook | Stale status | Small |
| XMPP | Stale status | Small |
| Nostr | Stale status | Small |

### 4. ChannelRegistry Changes

**`send()` — relax status guard:**

```rust
pub async fn send(&self, channel_id: &ChannelId, message: OutboundMessage) -> ChannelResult<SendResult> {
    let channel_arc = self.get(channel_id).await
        .ok_or_else(|| ChannelError::NotConnected(...))?;
    let channel = channel_arc.read().await;

    // Only reject explicitly Disabled channels
    if channel.status() == ChannelStatus::Disabled {
        return Err(ChannelError::NotConnected(...));
    }

    channel.send(message).await
}
```

**`list()` — use real-time status:**

```rust
pub async fn list(&self) -> Vec<ChannelInfo> {
    for channel_arc in channels.values() {
        let channel = channel_arc.read().await;
        let mut info = channel.info().clone();
        info.status = channel.status();  // override with live status
        infos.push(info);
    }
}
```

## Out of Scope

- Unifying PairingManager (security) and SqlitePairingStore (gateway) — separate effort
- Agent channel-awareness (sending images via channels) — separate effort
- Shutdown signaling improvements — functional as-is
- Channel capability negotiation with agent — separate effort
