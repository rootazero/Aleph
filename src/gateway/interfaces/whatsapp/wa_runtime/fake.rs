//! `FakeWaRuntime` — an in-process `WaRuntime` impl for end-to-end tests.
//!
//! Before R1 there was no `WaRuntime` trait, only a `test_mode: bool` field
//! on `WhatsAppChannel` that short-circuited every method. R1 Task 7
//! extracts the trait; this file is Task 8's payload — a fake impl that
//! drives the channel's event loop without a real WhatsApp connection.
//!
//! ## What the fake records
//!
//! The fake stores everything the real `RealWaRuntime` stores, but in
//! plain `Vec`s and `Mutex`es instead of `BotHandle` / `SqliteStore`:
//!
//! - `pairing` — current `PairingState`. Defaults to `Connected` so the
//!   existing `test_mode` tests (which assert `Connected` after `start()`)
//!   keep passing. Tests that exercise the QR pairing flow call
//!   `set_pairing_state(PairingState::Idle)` before `start()`.
//! - `sent_messages` — every `send_message` arg the channel made.
//! - `sent_reactions` — every `send_reaction` triple.
//! - `sent_typing` — every `send_typing` conversation id.
//! - `sent_reads` — every `mark_read` message id.
//!
//! ## Synchronisation
//!
//! `emit_*` methods push a `WaEvent` into an internal `mpsc::Sender` and
//! return immediately. The channel's event loop consumes the receiver on
//! a separate tokio task, so callers use `wait_for_pairing_state(...)` to
//! block until the state machine reflects the event. This is the same
//! shape the production transport has with respect to `WhatsAppChannel`,
//! so the seam is exercised end-to-end.

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use crate::gateway::channel::{MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::pairing::PairingState;
use crate::gateway::interfaces::whatsapp::wa_runtime::traits::{
    WaEvent, WaRuntime, WaRuntimeError,
};
use crate::sync_primitives::{Arc, Mutex as StdMutex};
use std::time::Duration;

/// In-process fake runtime for end-to-end testing.
///
/// All state lives behind `Arc<Mutex<...>>` so the struct itself is cheap
/// to clone. The fake carries no real I/O; outbound `send_*` methods
/// record the call and return success, and inbound events are pushed
/// into an internal channel that the `WhatsAppChannel` event loop
/// consumes via `take_event_receiver()`.
pub struct FakeWaRuntime {
    inner: Arc<FakeWaRuntimeInner>,
    /// `std::sync::Mutex` (not `tokio::sync::Mutex`) because
    /// `take_event_receiver` is a sync trait method and is invoked
    /// from inside the channel's async event loop — `tokio::sync::Mutex::blocking_lock`
    /// panics from within a runtime, whereas `std::sync::Mutex::lock`
    /// is always safe (at the cost of a brief OS-level block in the
    /// contended path, which is zero in the happy case).
    event_rx: StdMutex<Option<mpsc::Receiver<WaEvent>>>,
}

struct FakeWaRuntimeInner {
    pairing: Mutex<PairingState>,
    event_tx: mpsc::Sender<WaEvent>,
    sent_messages: Mutex<Vec<OutboundMessage>>,
    sent_reactions: Mutex<Vec<(String, String, String)>>,
    sent_typing: Mutex<Vec<String>>,
    sent_reads: Mutex<Vec<String>>,
}

impl FakeWaRuntime {
    /// Construct a fresh fake in `PairingState::Connected`. The default
    /// matches the behaviour of the `test_mode` shortcut that R1 is
    /// replacing: existing tests that asserted `Connected` after
    /// `start()` keep working without modification.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Self::with_pairing(PairingState::Connected {
            device_name: "Fake Device".to_string(),
            phone_number: "+10000000000".to_string(),
        })
    }

    /// Construct a fake starting from a specific `PairingState`. Useful
    /// for tests that want to exercise the QR-pairing flow without the
    /// fake auto-connecting on `start()`.
    #[must_use]
    pub fn with_pairing(initial: PairingState) -> Arc<Self> {
        let (event_tx, event_rx) = mpsc::channel(64);
        let inner = Arc::new(FakeWaRuntimeInner {
            pairing: Mutex::new(initial),
            event_tx,
            sent_messages: Mutex::new(Vec::new()),
            sent_reactions: Mutex::new(Vec::new()),
            sent_typing: Mutex::new(Vec::new()),
            sent_reads: Mutex::new(Vec::new()),
        });
        Arc::new(Self {
            inner,
            event_rx: StdMutex::new(Some(event_rx)),
        })
    }

    /// Override the fake's pairing state directly. Tests use this to
    /// reset between scenarios or to position the state machine at a
    /// non-default starting point before `start()`.
    pub async fn set_pairing_state(&self, state: PairingState) {
        *self.inner.pairing.lock().await = state;
    }

    /// Snapshot the fake's current pairing state.
    pub async fn pairing_state(&self) -> PairingState {
        self.inner.pairing.lock().await.clone()
    }

    /// Block until the fake's pairing state matches the predicate, or
    /// the timeout expires. Polls every 5ms — cheap, and bounded, so a
    /// missing event surfaces as a test failure rather than a hang.
    ///
    /// Polls the fake's **internal** pairing state, not the channel's.
    /// The fake carries its own mirror because the event loop's
    /// `PairingStateDriver` writes to the channel's `pairing_state`
    /// field, not the fake's, and tests need a uniform place to
    /// observe the fake's own state. For tests that care about what
    /// `Channel::status()` reports, poll the channel directly — its
    /// `status()` is sync and `try_read`-based, so it does not need
    /// this helper.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the timeout elapses before the predicate is
    /// satisfied. Callers should `expect("...")` so the test message
    /// names the predicate that wasn't met.
    pub async fn wait_for_pairing_state(
        &self,
        pred: impl Fn(&PairingState) -> bool,
        timeout: Duration,
    ) -> Result<(), ()> {
        let start = std::time::Instant::now();
        loop {
            {
                let guard = self.inner.pairing.lock().await;
                if pred(&guard) {
                    return Ok(());
                }
            }
            if start.elapsed() > timeout {
                return Err(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Emit a `WaEvent::PairingQrCode` so the event loop's
    /// `PairingStateDriver` maps it to `PairingState::WaitingQr`.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the event channel is closed (the
    /// `WhatsAppChannel` has been dropped). Production paths do not need
    /// to handle this — tests should.
    pub async fn emit_qr(&self, code: impl Into<String>) -> Result<(), ()> {
        self.inner
            .event_tx
            .send(WaEvent::PairingQrCode {
                code: code.into(),
                timeout: Duration::from_secs(60),
            })
            .await
            .map_err(|_| ())
    }

    /// Emit a `WaEvent::PairSuccess`. The `PairingStateDriver` arm
    /// maps this to `PairingState::Connected { device_name, phone_number }`.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the event channel is closed.
    pub async fn emit_pair_success(&self) -> Result<(), ()> {
        let pair_success = whatsapp_rust::types::events::PairSuccess {
            id: whatsapp_rust::Jid::new("1234", "s.whatsapp.net"),
            lid: whatsapp_rust::Jid::new("abcd", "lid"),
            business_name: "Fake Phone".to_string(),
            platform: "fake".to_string(),
        };
        self.inner
            .event_tx
            .send(WaEvent::PairSuccess(pair_success))
            .await
            .map_err(|_| ())
    }

    /// Emit a `WaEvent::Disconnected`. The `PairingStateDriver` arm
    /// maps this to `PairingState::Disconnected { reason: "remote disconnected" }`
    /// (the driver's reason string is fixed; the fake's `reason` arg is
    /// kept for callers that want a record of why the disconnect was
    /// simulated).
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the event channel is closed.
    pub async fn emit_disconnected(&self, _reason: impl Into<String>) -> Result<(), ()> {
        self.inner
            .event_tx
            .send(WaEvent::Disconnected(
                whatsapp_rust::types::events::Disconnected,
            ))
            .await
            .map_err(|_| ())
    }

    /// Emit a `WaEvent::Message` carrying a single text body. The event
    /// loop's inbound mapper (`map_event_to_inbound`) translates this
    /// into a generic `InboundMessage` and pushes it onto the channel's
    /// inbound broadcast.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the event channel is closed.
    pub async fn emit_text_message(
        &self,
        chat_jid: &str,
        sender: &str,
        text: &str,
    ) -> Result<(), ()> {
        let (user, server) = split_jid(chat_jid);
        let chat = whatsapp_rust::Jid::new(user, server);
        let sender_jid = whatsapp_rust::Jid::new(sender, "s.whatsapp.net");
        let msg = whatsapp_rust::waproto::whatsapp::Message {
            conversation: Some(text.to_string()),
            ..Default::default()
        };
        let info = whatsapp_rust::types::message::MessageInfo {
            source: whatsapp_rust::types::message::MessageSource {
                chat: chat.clone(),
                sender: sender_jid.clone(),
                is_from_me: false,
                is_group: chat_jid.ends_with("@g.us"),
                addressing_mode: None,
                sender_alt: None,
                recipient_alt: None,
                broadcast_list_owner: None,
                recipient: None,
            },
            id: format!("fake-msg-{}", Utc::now().timestamp_millis()),
            server_id: 0,
            r#type: "text".to_string(),
            push_name: sender.to_string(),
            timestamp: Utc::now(),
            category: String::new(),
            multicast: false,
            media_type: String::new(),
            edit: whatsapp_rust::types::message::EditAttribute::default(),
            bot_info: None,
            meta_info: whatsapp_rust::types::message::MsgMetaInfo::default(),
            verified_name: None,
            device_sent_meta: None,
        };
        self.inner
            .event_tx
            .send(WaEvent::Message(Box::new(msg), info))
            .await
            .map_err(|_| ())
    }

    /// Snapshot of every `send_message` arg the channel has made.
    pub async fn sent_messages(&self) -> Vec<OutboundMessage> {
        self.inner.sent_messages.lock().await.clone()
    }

    /// Snapshot of every `send_reaction` triple the channel has made:
    /// `(conversation_id, message_id, emoji)`.
    pub async fn sent_reactions(&self) -> Vec<(String, String, String)> {
        self.inner.sent_reactions.lock().await.clone()
    }

    /// Snapshot of every `send_typing` conversation id the channel has
    /// notified. Order is insertion order.
    pub async fn sent_typing(&self) -> Vec<String> {
        self.inner.sent_typing.lock().await.clone()
    }

    /// Snapshot of every `mark_read` message id the channel has marked.
    pub async fn sent_reads(&self) -> Vec<String> {
        self.inner.sent_reads.lock().await.clone()
    }

    /// True if `take_event_receiver()` has already been called.
    pub async fn receiver_taken(&self) -> bool {
        // `try_lock` avoids blocking the async task. If a parallel
        // `take_event_receiver` is in flight we report the previous
        // state — tests that care about exact timing call
        // `wait_for_pairing_state` instead.
        self.event_rx
            .try_lock()
            .map(|g| g.is_none())
            .unwrap_or(false)
    }
}

/// Split a JID string like `1234@s.whatsapp.net` into `("1234", "s.whatsapp.net")`.
/// Falls back to `("jid", "s.whatsapp.net")` for inputs without `@`.
fn split_jid(jid: &str) -> (&str, &str) {
    match jid.split_once('@') {
        Some((u, s)) => (u, s),
        None => (jid, "s.whatsapp.net"),
    }
}

#[async_trait]
impl WaRuntime for FakeWaRuntime {
    async fn start(&self) -> Result<(), WaRuntimeError> {
        // No-op. The fake has no transport to spin up. Tests drive
        // pairing state and inbound events through `emit_*` directly,
        // so there is nothing for `start` to schedule.
        Ok(())
    }

    async fn shutdown(&self) {
        // No-op. Closing the event channel would do nothing here
        // because the channel layer only takes the receiver, not the
        // sender.
    }

    async fn pairing_phase(&self) -> PairingState {
        self.inner.pairing.lock().await.clone()
    }

    async fn send_message(&self, msg: OutboundMessage) -> Result<MessageId, WaRuntimeError> {
        self.inner.sent_messages.lock().await.push(msg.clone());
        Ok(MessageId::new(format!(
            "fake-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )))
    }

    async fn send_typing(&self, conversation_id: &str) -> Result<(), WaRuntimeError> {
        self.inner
            .sent_typing
            .lock()
            .await
            .push(conversation_id.to_string());
        Ok(())
    }

    async fn mark_read(&self, message_id: &str) -> Result<(), WaRuntimeError> {
        self.inner
            .sent_reads
            .lock()
            .await
            .push(message_id.to_string());
        Ok(())
    }

    async fn send_reaction(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), WaRuntimeError> {
        self.inner.sent_reactions.lock().await.push((
            conversation_id.to_string(),
            message_id.to_string(),
            emoji.to_string(),
        ));
        Ok(())
    }

    fn take_event_receiver(&self) -> Option<mpsc::Receiver<WaEvent>> {
        // `std::sync::Mutex::lock` is safe to call from within an async
        // runtime context (unlike `tokio::sync::Mutex::blocking_lock`).
        // The lock is held for one `Option::take` — nanoseconds — so the
        // brief OS-level block is invisible.
        self.event_rx.lock().ok().and_then(|mut g| g.take())
    }
}
