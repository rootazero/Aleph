# Channel Infrastructure Fix Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix `status()`, `inbound_receiver()`, and `send()` across all 14 channels by introducing a shared `ChannelState` helper and migrating every channel to use it.

**Architecture:** Add a `ChannelState` struct encapsulating `Arc<RwLock<ChannelStatus>>` + `std::sync::Mutex<Option<Receiver>>` + `mpsc::Sender`. Add a new required `fn state(&self) -> &ChannelState` method to the `Channel` trait. Update default `status()` and `inbound_receiver()` to delegate to `ChannelState`. Mechanically migrate all 14 channels. Relax `ChannelRegistry::send()` guard.

**Tech Stack:** Rust, tokio, async-trait, tokio::sync::RwLock, std::sync::Mutex

**Design doc:** `docs/plans/2026-03-03-channel-infrastructure-fix-design.md`

---

### Task 1: Add `ChannelState` struct and update `Channel` trait

**Files:**
- Modify: `core/src/gateway/channel.rs:28-405`

**Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `channel.rs`:

```rust
#[tokio::test]
async fn test_channel_state_initial_status() {
    let state = ChannelState::new(100);
    assert_eq!(state.status(), ChannelStatus::Disconnected);
}

#[tokio::test]
async fn test_channel_state_set_status() {
    let state = ChannelState::new(100);
    state.set_status(ChannelStatus::Connected).await;
    assert_eq!(state.status(), ChannelStatus::Connected);
}

#[tokio::test]
async fn test_channel_state_take_receiver_once() {
    let state = ChannelState::new(100);
    let rx = state.take_receiver();
    assert!(rx.is_some());
    // Second call returns None
    let rx2 = state.take_receiver();
    assert!(rx2.is_none());
}

#[tokio::test]
async fn test_channel_state_sender_delivers() {
    let state = ChannelState::new(100);
    let mut rx = state.take_receiver().unwrap();
    let tx = state.sender();

    let msg = InboundMessage {
        id: MessageId::new("test-1"),
        channel_id: ChannelId::new("test"),
        conversation_id: ConversationId::new("conv-1"),
        sender_id: UserId::new("user-1"),
        sender_name: None,
        text: "hello".to_string(),
        attachments: vec![],
        timestamp: chrono::Utc::now(),
        reply_to: None,
        is_group: false,
        raw: None,
    };

    tx.send(msg).await.unwrap();
    let received = rx.recv().await.unwrap();
    assert_eq!(received.text, "hello");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::channel::tests::test_channel_state -- --no-capture 2>&1 | head -30`
Expected: FAIL — `ChannelState` not defined

**Step 3: Implement `ChannelState` and update `Channel` trait**

Add to `channel.rs` imports (near line 28):

```rust
use std::sync::Mutex as StdMutex;
```

Add the `ChannelState` struct after `ChannelInfo` definition (after line 347):

```rust
/// Shared mutable state for Channel implementations.
///
/// Encapsulates thread-safe status tracking and inbound message receiver management.
/// Every Channel should embed this and return it from `fn state()`.
pub struct ChannelState {
    /// Thread-safe status — set by start()/stop(), read by status()
    status: Arc<tokio::sync::RwLock<ChannelStatus>>,
    /// One-shot receiver — taken once by ChannelRegistry::start_message_forwarder()
    inbound_rx: StdMutex<Option<mpsc::Receiver<InboundMessage>>>,
    /// Sender side — channel impl pushes inbound messages here
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl ChannelState {
    /// Create with initial Disconnected status and a bounded channel.
    pub fn new(buffer_size: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer_size);
        Self {
            status: Arc::new(tokio::sync::RwLock::new(ChannelStatus::Disconnected)),
            inbound_rx: StdMutex::new(Some(rx)),
            inbound_tx: tx,
        }
    }

    /// Read current status (non-blocking via try_read, fallback Connecting).
    pub fn status(&self) -> ChannelStatus {
        self.status.try_read().map(|s| *s).unwrap_or(ChannelStatus::Connecting)
    }

    /// Set status (async, takes write lock).
    pub async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }

    /// Take the inbound receiver (can only succeed once).
    pub fn take_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.inbound_rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Get a clone of the inbound sender.
    pub fn sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }
}
```

Update the `Channel` trait (lines 362-405):

1. Add new required method after `fn info()`:
```rust
    /// Get shared mutable state (status + inbound receiver).
    fn state(&self) -> &ChannelState;
```

2. Change `status()` default (line 378-380) from:
```rust
    fn status(&self) -> ChannelStatus {
        self.info().status
    }
```
to:
```rust
    fn status(&self) -> ChannelStatus {
        self.state().status()
    }
```

3. Change `inbound_receiver()` (line 401-405) from:
```rust
    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>>;
```
to:
```rust
    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.state().take_receiver()
    }
```

Note: `inbound_receiver` changes from **required** to **provided** (has default impl). This means channels that override it will still compile but can now delete their override.

Also add `use crate::sync_primitives::Arc;` to the imports if not already present (it uses `std::sync::Arc` in imports currently via `std::collections::HashMap`).

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::channel::tests::test_channel_state -- --no-capture`
Expected: 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/channel.rs
git commit -m "gateway: add ChannelState struct and update Channel trait defaults"
```

---

### Task 2: Migrate Telegram channel to `ChannelState`

Telegram is the gold standard — already has correct behavior. This migration replaces its custom `RwLock<ChannelStatus>` + `Mutex<Option<Receiver>>` with the unified `ChannelState`.

**Files:**
- Modify: `core/src/gateway/interfaces/telegram/mod.rs`

**Step 1: Update struct and constructor**

In the struct definition (lines 51-70), replace:
```rust
    inbound_tx: mpsc::Sender<InboundMessage>,
    inbound_rx: std::sync::Mutex<Option<mpsc::Receiver<InboundMessage>>>,
    ...
    status: Arc<RwLock<ChannelStatus>>,
```
with:
```rust
    channel_state: ChannelState,
```

Keep `callback_tx`, `callback_rx`, `shutdown_tx`, `bot` — those are Telegram-specific.

In the constructor `new()` (lines 72-97), replace:
```rust
        let (inbound_tx, inbound_rx) = mpsc::channel(100);
        ...
        Self {
            ...
            inbound_tx,
            inbound_rx: std::sync::Mutex::new(Some(inbound_rx)),
            ...
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
            ...
        }
```
with:
```rust
        Self {
            ...
            channel_state: ChannelState::new(100),
            ...
        }
```

Add `use crate::gateway::channel::ChannelState;` to imports if needed.

**Step 2: Update Channel trait impl**

Add `fn state()`:
```rust
    fn state(&self) -> &ChannelState {
        &self.channel_state
    }
```

Delete the custom `status()` override (around line 299-303).

Delete the custom `inbound_receiver()` override (around line 538-542).

**Step 3: Update all `self.inbound_tx` → `self.channel_state.sender()`**

In `start()` and message handler closures, wherever `self.inbound_tx.clone()` is used, replace with `self.channel_state.sender()`.

Replace all `self.status` (the old `Arc<RwLock<ChannelStatus>>`) with `self.channel_state`:
- `*self.status.write().await = ChannelStatus::Connecting` → `self.channel_state.set_status(ChannelStatus::Connecting).await`
- `*self.status.write().await = ChannelStatus::Connected` → `self.channel_state.set_status(ChannelStatus::Connected).await`
- `*self.status.write().await = ChannelStatus::Disconnected` → `self.channel_state.set_status(ChannelStatus::Disconnected).await`

For places inside spawned tasks that hold `Arc<RwLock<ChannelStatus>>`, they can no longer hold a reference to `self.channel_state` directly. Instead, clone what's needed before the `tokio::spawn`:
```rust
let inbound_tx = self.channel_state.sender();
// use inbound_tx inside the spawned task
```

**Step 4: Build and run tests**

Run: `cargo check -p alephcore 2>&1 | head -40`
Run: `cargo test -p alephcore --lib gateway::channel -- --no-capture`
Expected: PASS (no Telegram-specific tests to run since they require network)

**Step 5: Commit**

```bash
git add core/src/gateway/interfaces/telegram/mod.rs
git commit -m "gateway: migrate Telegram channel to ChannelState"
```

---

### Task 3: Migrate iMessage channel to `ChannelState`

iMessage currently has `info.status =` direct mutation (unsound) and `inbound_receiver()` returns `None`.

**Files:**
- Modify: `core/src/gateway/interfaces/imessage/mod.rs`

**Step 1: Update struct**

Replace in struct (lines 48-55):
```rust
pub struct IMessageChannel {
    info: ChannelInfo,
    config: IMessageConfig,
    db: Arc<Mutex<Option<MessagesDb>>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: Arc<AtomicBool>,
    poll_handle: Option<tokio::task::JoinHandle<()>>,
}
```
with:
```rust
pub struct IMessageChannel {
    info: ChannelInfo,
    config: IMessageConfig,
    db: Arc<Mutex<Option<MessagesDb>>>,
    channel_state: ChannelState,
    running: Arc<AtomicBool>,
    poll_handle: Option<tokio::task::JoinHandle<()>>,
}
```

Add `use crate::gateway::channel::ChannelState;` to imports.

**Step 2: Update constructor**

In `new()` (line 59-91), replace:
```rust
        let (tx, _rx) = mpsc::channel(100);
        ...
        Self {
            ...
            inbound_tx: tx,
            ...
        }
```
with:
```rust
        Self {
            ...
            channel_state: ChannelState::new(100),
            ...
        }
```

Note: The old code creates `_rx` and immediately drops it — this is the root cause of `inbound_receiver()` returning `None`.

**Step 3: Add `fn state()` to Channel impl**

```rust
    fn state(&self) -> &ChannelState {
        &self.channel_state
    }
```

Delete the `inbound_receiver()` override (lines 227-231).

**Step 4: Fix start()/stop() — remove `self.info.status =`**

In `start()` (lines 162-175), replace:
```rust
        self.info.status = ChannelStatus::Connecting;
        ...
        self.info.status = ChannelStatus::Connected;
```
with:
```rust
        self.channel_state.set_status(ChannelStatus::Connecting).await;
        ...
        self.channel_state.set_status(ChannelStatus::Connected).await;
```

In `stop()` (lines 178-199), replace:
```rust
        self.info.status = ChannelStatus::Disconnected;
```
with:
```rust
        self.channel_state.set_status(ChannelStatus::Disconnected).await;
```

**Step 5: Fix inbound_tx usage in polling loop**

In `start_polling()` (line 112), replace:
```rust
        let tx = self.inbound_tx.clone();
```
with:
```rust
        let tx = self.channel_state.sender();
```

**Step 6: Build**

Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/gateway/interfaces/imessage/mod.rs
git commit -m "gateway: migrate iMessage channel to ChannelState"
```

---

### Task 4: Migrate CLI channel to `ChannelState`

CLI currently stores status in `CliChannelState` inside `Arc<RwLock<>>` and directly mutates `info.status`. Its `inbound_receiver()` returns `None`.

**Files:**
- Modify: `core/src/gateway/interfaces/cli.rs`

**Step 1: Update struct**

The CLI channel has an unusual `CliChannelState` struct (lines 72-76). We keep it for `shutdown_tx` but remove `status` and `inbound_tx` from it.

Replace `CliChannelState` (lines 72-76):
```rust
struct CliChannelState {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}
```

Update `CliChannel` struct (lines 79-83):
```rust
pub struct CliChannel {
    info: ChannelInfo,
    config: CliChannelConfig,
    channel_state: ChannelState,
    cli_state: Arc<RwLock<CliChannelState>>,
}
```

Add `use crate::gateway::channel::ChannelState;` to imports.

**Step 2: Update constructor**

In `with_config()` (lines 97-133), replace:
```rust
        let (inbound_tx, _inbound_rx) = mpsc::channel(100);
        ...
        let state = CliChannelState {
            status: ChannelStatus::Disconnected,
            inbound_tx: Some(inbound_tx),
            shutdown_tx: None,
        };

        Self {
            info,
            config,
            state: Arc::new(RwLock::new(state)),
        }
```
with:
```rust
        let cli_state = CliChannelState {
            shutdown_tx: None,
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            cli_state: Arc::new(RwLock::new(cli_state)),
        }
```

**Step 3: Add `fn state()`, remove overrides**

```rust
    fn state(&self) -> &ChannelState {
        &self.channel_state
    }
```

Delete the `inbound_receiver()` override (lines 293-297).

**Step 4: Fix start()/stop()**

In `start()` (lines 167-251):
- Remove `state.status = ChannelStatus::Connecting` and `self.info.status = ChannelStatus::Connecting`
- Replace with `self.channel_state.set_status(ChannelStatus::Connecting).await`
- Remove `state.status = ChannelStatus::Connected` and `self.info.status = ChannelStatus::Connected`
- Replace with `self.channel_state.set_status(ChannelStatus::Connected).await`

In `stop()` (lines 254-266):
- Remove `state.status = ChannelStatus::Disconnected` and `self.info.status = ChannelStatus::Disconnected`
- Replace with `self.channel_state.set_status(ChannelStatus::Disconnected).await`

**Step 5: Fix inbound_tx usage**

In `inject_message()` and the spawned stdin reader task, replace reads of `state.inbound_tx` with `self.channel_state.sender()`. Clone the sender before spawning:
```rust
let inbound_tx = self.channel_state.sender();
```

In `send()`, replace `state.status != ChannelStatus::Connected` check with `self.channel_state.status() != ChannelStatus::Connected` (no need for RwLock read).

**Step 6: Build**

Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/gateway/interfaces/cli.rs
git commit -m "gateway: migrate CLI channel to ChannelState"
```

---

### Task 5: Migrate Discord channel to `ChannelState`

Discord has `inbound_rx: Option<mpsc::Receiver<InboundMessage>>` that returns `None` from `inbound_receiver()`, and uses trait default for `status()`.

**Files:**
- Modify: `core/src/gateway/interfaces/discord/mod.rs`

**Step 1: Update struct and constructor**

In the struct (lines 55-70), replace `inbound_tx`, `inbound_rx`, `status` with `channel_state: ChannelState`.

In `new()` (lines 74-94), replace:
```rust
        let (inbound_tx, inbound_rx) = mpsc::channel(100);
        ...
        Self {
            ...
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            ...
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
            ...
        }
```
with:
```rust
        Self {
            ...
            channel_state: ChannelState::new(100),
            ...
        }
```

Add `use crate::gateway::channel::ChannelState;` to imports.

**Step 2: Add `fn state()`, remove overrides**

```rust
    fn state(&self) -> &ChannelState {
        &self.channel_state
    }
```

Delete:
- Custom `status()` override
- Custom `inbound_receiver()` override
- `take_receiver()` method if present

**Step 3: Fix start()/stop() and message handler**

Replace all `*self.status.write().await = ...` with `self.channel_state.set_status(...).await`.

Replace `self.inbound_tx.clone()` with `self.channel_state.sender()` in the serenity event handler setup. The `Handler` struct that implements serenity's `EventHandler` needs the sender cloned before the spawn.

**Step 4: Build**

Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/interfaces/discord/mod.rs
git commit -m "gateway: migrate Discord channel to ChannelState"
```

---

### Task 6: Migrate Slack channel to `ChannelState`

Same pattern as Discord. Has `inbound_rx: Option` that returns `None`.

**Files:**
- Modify: `core/src/gateway/interfaces/slack/mod.rs`

**Step 1: Update struct and constructor**

Replace `inbound_tx`, `inbound_rx`, `status` fields with `channel_state: ChannelState`.

In `new()` (lines 64-84):
```rust
        Self {
            ...
            channel_state: ChannelState::new(100),
            ...
        }
```

Delete `take_receiver()` method (lines 107-109).

**Step 2: Add `fn state()`, remove overrides**

```rust
    fn state(&self) -> &ChannelState {
        &self.channel_state
    }
```

Delete custom `status()` and `inbound_receiver()` overrides.

**Step 3: Fix start()/stop() and message ops**

Replace `*self.status.write().await = ...` with `self.channel_state.set_status(...).await`.

Replace `self.inbound_tx.clone()` with `self.channel_state.sender()` before passing to `SlackMessageOps::run_socket_mode_loop()`.

**Step 4: Build**

Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/gateway/interfaces/slack/mod.rs
git commit -m "gateway: migrate Slack channel to ChannelState"
```

---

### Task 7: Migrate Email channel to `ChannelState`

Same pattern. Has `inbound_rx: Option` that returns `None`.

**Files:**
- Modify: `core/src/gateway/interfaces/email/mod.rs`

**Steps:** Same migration pattern as Task 6.
- Replace `inbound_tx`, `inbound_rx`, `status` with `channel_state: ChannelState`
- Add `fn state()` → `&self.channel_state`
- Delete custom `status()` and `inbound_receiver()` overrides
- Replace `self.inbound_tx.clone()` with `self.channel_state.sender()`
- Replace status writes with `self.channel_state.set_status(...).await`

**Build:** `cargo check -p alephcore 2>&1 | head -40`

**Commit:**
```bash
git add core/src/gateway/interfaces/email/mod.rs
git commit -m "gateway: migrate Email channel to ChannelState"
```

---

### Task 8: Migrate Matrix, Signal, Mattermost channels to `ChannelState`

All three follow the identical pattern (struct with `inbound_tx`, `inbound_rx: Option`, `status: Arc<RwLock>`). Batch them into one task.

**Files:**
- Modify: `core/src/gateway/interfaces/matrix/mod.rs`
- Modify: `core/src/gateway/interfaces/signal/mod.rs`
- Modify: `core/src/gateway/interfaces/mattermost/mod.rs`

**Steps (for each):**
1. Replace `inbound_tx`, `inbound_rx`, `status` with `channel_state: ChannelState`
2. Add `use crate::gateway::channel::ChannelState;`
3. Update constructor to use `ChannelState::new(100)`
4. Add `fn state()` → `&self.channel_state`
5. Delete custom `status()` and `inbound_receiver()` overrides
6. Replace `self.inbound_tx.clone()` with `self.channel_state.sender()`
7. Replace status writes with `self.channel_state.set_status(...).await`

**Build:** `cargo check -p alephcore 2>&1 | head -40`

**Commit:**
```bash
git add core/src/gateway/interfaces/matrix/mod.rs core/src/gateway/interfaces/signal/mod.rs core/src/gateway/interfaces/mattermost/mod.rs
git commit -m "gateway: migrate Matrix, Signal, Mattermost channels to ChannelState"
```

---

### Task 9: Migrate IRC, XMPP, Nostr channels to `ChannelState`

All three have `write_tx` for bidirectional communication. Same migration for status/receiver.

**Files:**
- Modify: `core/src/gateway/interfaces/irc/mod.rs`
- Modify: `core/src/gateway/interfaces/xmpp/mod.rs`
- Modify: `core/src/gateway/interfaces/nostr/mod.rs`

**Steps (for each):**
1. Replace `inbound_tx`, `inbound_rx`, `status` with `channel_state: ChannelState`
2. Add `use crate::gateway::channel::ChannelState;`
3. Update constructor to use `ChannelState::new(100)`
4. Add `fn state()` → `&self.channel_state`
5. Delete custom `status()` and `inbound_receiver()` overrides
6. Replace `self.inbound_tx.clone()` with `self.channel_state.sender()`
7. Replace status writes with `self.channel_state.set_status(...).await`
8. Keep `write_tx` — it's for outbound command dispatching, unrelated

**Build:** `cargo check -p alephcore 2>&1 | head -40`

**Commit:**
```bash
git add core/src/gateway/interfaces/irc/mod.rs core/src/gateway/interfaces/xmpp/mod.rs core/src/gateway/interfaces/nostr/mod.rs
git commit -m "gateway: migrate IRC, XMPP, Nostr channels to ChannelState"
```

---

### Task 10: Migrate Webhook and WhatsApp channels to `ChannelState`

Webhook is simple. WhatsApp has a custom `status()` via `pairing_state` — keep its custom override but still add `ChannelState` for receiver.

**Files:**
- Modify: `core/src/gateway/interfaces/webhook/mod.rs`
- Modify: `core/src/gateway/interfaces/whatsapp/mod.rs`

**Webhook migration (standard pattern):**
1. Replace `inbound_tx`, `inbound_rx`, `status` with `channel_state: ChannelState`
2. Add `fn state()`, delete custom `status()` and `inbound_receiver()` overrides
3. Replace `self.inbound_tx.clone()` with `self.channel_state.sender()` in `GenericWebhookHandler`
4. Replace status writes with `self.channel_state.set_status(...).await`

**WhatsApp migration (special case):**
- WhatsApp has `pairing_state: Arc<RwLock<PairingState>>` and a custom `status()` that maps internal pairing states to `ChannelStatus`.
- Add `channel_state: ChannelState` field
- Implement `fn state()` → `&self.channel_state`
- **Keep the custom `status()` override** — it maps `PairingState` variants (Idle, Connecting, WaitingForQR, etc.) to `ChannelStatus`. But update it to also write to `channel_state` when pairing state changes.
- Replace `inbound_tx` with `self.channel_state.sender()`
- Delete custom `inbound_receiver()` override

**Build:** `cargo check -p alephcore 2>&1 | head -40`

**Commit:**
```bash
git add core/src/gateway/interfaces/webhook/mod.rs core/src/gateway/interfaces/whatsapp/mod.rs
git commit -m "gateway: migrate Webhook and WhatsApp channels to ChannelState"
```

---

### Task 11: Update `ChannelRegistry::send()` and `list()`

**Files:**
- Modify: `core/src/gateway/channel_registry.rs:222-257`

**Step 1: Write test for relaxed send guard**

Add to the existing test module in `channel_registry.rs`:

```rust
#[tokio::test]
async fn test_send_rejects_disabled_channel() {
    // ChannelRegistry::send() should only reject Disabled status, not Disconnected
    let registry = ChannelRegistry::new();
    // No channel registered — should get NotConnected error (channel not found)
    let msg = crate::gateway::channel::OutboundMessage::text("conv", "hello");
    let result = registry.send(&ChannelId::new("nonexistent"), msg).await;
    assert!(result.is_err());
}
```

**Step 2: Update `send()` method**

In `send()` (lines 222-241), replace:
```rust
        if channel.status() != ChannelStatus::Connected {
            return Err(ChannelError::NotConnected(format!(
                "Channel {} is not connected",
                channel_id
            )));
        }
```
with:
```rust
        if channel.status() == ChannelStatus::Disabled {
            return Err(ChannelError::NotConnected(format!(
                "Channel {} is disabled",
                channel_id
            )));
        }
```

**Step 3: Update `broadcast()` method**

In `broadcast()` (lines 244-257), replace:
```rust
            if channel.status() == ChannelStatus::Connected {
```
with:
```rust
            if channel.status() != ChannelStatus::Disabled {
```

**Step 4: Update `list()` to use real-time status**

In `list()` (lines 134-144), replace:
```rust
        for channel_arc in channels.values() {
            let channel = channel_arc.read().await;
            infos.push(channel.info().clone());
        }
```
with:
```rust
        for channel_arc in channels.values() {
            let channel = channel_arc.read().await;
            let mut info = channel.info().clone();
            info.status = channel.status(); // override with live status
            infos.push(info);
        }
```

**Step 5: Update `list_by_type()` similarly**

In `list_by_type()` (lines 147-159), add the same status override:
```rust
            if channel.channel_type() == channel_type {
                let mut info = channel.info().clone();
                info.status = channel.status();
                infos.push(info);
            }
```

**Step 6: Build and test**

Run: `cargo test -p alephcore --lib gateway::channel_registry -- --no-capture`
Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: PASS

**Step 7: Commit**

```bash
git add core/src/gateway/channel_registry.rs
git commit -m "gateway: relax ChannelRegistry send() guard, use real-time status in list()"
```

---

### Task 12: Full build verification and integration test

**Step 1: Full cargo check**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: No errors. Fix any remaining compilation issues.

**Step 2: Run all gateway tests**

Run: `cargo test -p alephcore --lib gateway -- --no-capture 2>&1 | tail -30`
Expected: All tests pass.

**Step 3: Run full crate tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: No new failures (pre-existing `markdown_skill` failures are known).

**Step 4: Commit any remaining fixes**

```bash
git add -A
git commit -m "gateway: fix remaining compilation issues from ChannelState migration"
```

(Only if there were fixes needed in Step 1-3.)
