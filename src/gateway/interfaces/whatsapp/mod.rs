//! `WhatsApp` Channel Implementation
//!
//! Native Rust integration with `WhatsApp` using whatsapp-rust.
//!
//! # Features
//!
//! - Multi-device support (scan QR code to link)
//! - Chat/group messages
//! - Image/file attachments
//! - Read receipts
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "whatsapp"
//! channel_type = "whatsapp"
//! enabled = true
//!
//! [channels.config]
//! phone_number = "+1234567890"
//! ```

pub mod config;
pub mod message;
pub mod pairing;

pub mod history_buffer;
pub mod media;
pub mod reactions;
pub mod types;
pub mod wa_auth;
pub mod wa_inbound;
pub mod wa_outbound;
pub mod wa_policy;
pub mod wa_runtime;

pub use config::{AccessConfig, DeliveryConfig, ReactionConfig, WhatsAppConfig};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, MessageId, OutboundMessage, PairingData,
    SendResult,
};
use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
use crate::gateway::interfaces::whatsapp::wa_runtime::fake::FakeWaRuntime;
use crate::gateway::interfaces::whatsapp::wa_runtime::{RealWaRuntime, WaRuntime};
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, Ordering};
use async_trait::async_trait;
use tokio::sync::{oneshot, RwLock};

use crate::gateway::interfaces::whatsapp::history_buffer::GroupHistoryBuffer;
use crate::gateway::interfaces::whatsapp::reactions::{ReactionHandler, ReactionSender};
use pairing::PairingState;

/// `WhatsApp` channel implementation backed by native Rust runtime.
pub struct WhatsAppChannel {
    info: ChannelInfo,
    config: WhatsAppConfig,
    channel_state: ChannelState,
    runtime: Option<Arc<dyn WaRuntime>>,
    /// Held alongside `runtime` so tests can reach the fake's emit
    /// methods and recorded-send bookkeeping. `None` in production
    /// paths. Set by `for_test_with_fake`; the legacy `for_test` path
    /// also installs a fake (Task 8) so the existing `test_mode`
    /// shortcut can be retired safely.
    fake: Option<Arc<FakeWaRuntime>>,
    pairing_state: Arc<RwLock<PairingState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    connected: Arc<AtomicBool>,
    test_mode: bool,
    reaction_handler: Option<Arc<ReactionHandler>>,
    history_buffer: Arc<GroupHistoryBuffer>,
}

impl WhatsAppChannel {
    pub fn new(id: impl Into<String>, config: WhatsAppConfig) -> Self {
        Self::with_mode(id, config, false)
    }

    fn with_mode(id: impl Into<String>, config: WhatsAppConfig, test_mode: bool) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "WhatsApp".to_string(),
            channel_type: "whatsapp".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        // `ReactionHandler` wraps a `ReactionSender`; the production sender is
        // a thin adapter over `WaRuntime::send_reaction`. In `test_mode` and
        // pre-`start()` we hold a no-op sender so the handler is safely
        // callable but cannot accidentally reach the network. `start()`
        // replaces the handler with one wired to the real `WaRuntime`.
        let reaction_handler = Some(Arc::new(ReactionHandler::new(
            config.reactions.level,
            config.reactions.ack.clone(),
            Arc::new(NoopReactionSender),
        )));
        let history_buffer = Arc::new(GroupHistoryBuffer::new(config.history.clone()));

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            runtime: None,
            fake: None,
            pairing_state: Arc::new(RwLock::new(PairingState::Idle)),
            shutdown_tx: None,
            connected: Arc::new(AtomicBool::new(false)),
            test_mode,
            reaction_handler,
            history_buffer,
        }
    }

    /// Construct a channel backed by `FakeWaRuntime`. Returns both the
    /// channel and the fake so tests can call `emit_*` and inspect
    /// recorded sends. The fake defaults to `PairingState::Connected`
    /// so the existing `for_test` tests that assert `Connected` after
    /// `start()` keep passing; tests that want to exercise the QR
    /// pairing flow call `fake.set_pairing_state(PairingState::Idle)`
    /// (and the matching `WhatsAppChannel::reset_pairing_state_for_test`)
    /// before driving events.
    pub fn for_test_with_fake(
        id: impl Into<String>,
        config: WhatsAppConfig,
    ) -> (Self, Arc<FakeWaRuntime>) {
        let mut channel = Self::with_mode(id, config, false);
        let fake = FakeWaRuntime::new();
        // Wire the fake as the runtime so the channel's event loop can
        // pull events off the fake's internal mpsc.
        let runtime: Arc<dyn WaRuntime> = Arc::clone(&fake) as Arc<dyn WaRuntime>;
        channel.runtime = Some(runtime);
        channel.fake = Some(Arc::clone(&fake));
        (channel, fake)
    }

    /// Test-only helper: reset the channel's pairing-state mirror to a
    /// known starting point. `FakeWaRuntime` carries its own pairing
    /// state internally; the channel carries a separate
    /// `Arc<RwLock<PairingState>>` that drives `Channel::status()`. Both
    /// must be in sync for assertions to be meaningful, so scenario tests
    /// call this alongside `fake.set_pairing_state(...)`.
    pub async fn reset_pairing_state_for_test(&self, state: PairingState) {
        *self.pairing_state.write().await = state;
    }

    /// Test-only getter: snapshot the channel's pairing state. The
    /// fake's `wait_for_pairing_state` accepts a getter closure so
    /// tests can poll the channel's `PairingState` (the same field
    /// `Channel::status()` reads from) without reaching into private
    /// state.
    pub async fn pairing_state(&self) -> PairingState {
        self.pairing_state.read().await.clone()
    }

    /// Spawn the channel's event loop pulling from the fake's mpsc.
    /// Mirrors the production path's `tokio::spawn(async move { ... })`
    /// block in `start()` but takes the rx from `fake.take_event_receiver`
    /// and the runtime adapter from the fake itself rather than the
    /// `RealWaRuntime`/`WaAuthManager` plumbing that the production path
    /// uses.
    async fn run_fake_event_loop(&mut self, fake: Arc<FakeWaRuntime>) -> ChannelResult<()> {
        let mut event_rx = fake
            .take_event_receiver()
            .ok_or_else(|| ChannelError::Internal("fake runtime receiver already taken".into()))?;

        // Replace the no-op reaction handler with one wired to the
        // fake's `send_reaction`. Without this, ack reactions on
        // accepted inbound messages would silently no-op (the
        // `NoopReactionSender` from `with_mode`).
        let reaction_handler = Arc::new(ReactionHandler::new(
            self.config.reactions.level,
            self.config.reactions.ack.clone(),
            Arc::new(WaRuntimeReactionAdapter {
                runtime: Arc::clone(&fake) as Arc<dyn WaRuntime>,
            }),
        ));
        self.reaction_handler = Some(Arc::clone(&reaction_handler));

        let connected = Arc::clone(&self.connected);
        let pairing_state = Arc::clone(&self.pairing_state);
        let inbound_tx = self.channel_state.sender();
        let channel_id = self.info.id.clone();
        let history_buffer = Arc::clone(&self.history_buffer);
        let reaction_handler = Arc::clone(&reaction_handler);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let access = AccessConfig {
            dm_policy: self.config.access.dm_policy,
            allow_from: self.config.access.allow_from.clone(),
            group_policy: self.config.access.group_policy,
            group_allow_from: self.config.access.group_allow_from.clone(),
            groups: self.config.access.groups.clone(),
        };
        let policy = crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicy::new(
            access,
            vec![],
        );
        let driver = PairingStateDriver::new(Arc::clone(&pairing_state));
        let mut shutdown_rx = shutdown_rx;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = event_rx.recv() => {
                        use whatsapp_rust::types::events::Event;
                        // Drive PairingState from the same 4 events the
                        // production path drives (Task 2). The fake's
                        // synthetic `start()` event is `PairSuccess`, so
                        // by the time we get here `pairing_state` is
                        // already `Connected` and this call is a
                        // no-op.
                        driver.apply(&event).await;
                        match event {
                            Event::Connected(_) => {
                                connected.store(true, Ordering::SeqCst);
                            }
                            Event::Disconnected(_) => {
                                connected.store(false, Ordering::SeqCst);
                            }
                            _ => {
                                if let Some(msg) = crate::gateway::interfaces::whatsapp::wa_inbound::mapper::map_event_to_inbound(&event, &channel_id, &history_buffer).await {
                                    match policy.evaluate(&msg) {
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::Accept => {
                                            if let Err(e) = reaction_handler.send_ack(&msg).await {
                                                tracing::debug!(
                                                    channel = %channel_id,
                                                    error = %e,
                                                    "reaction_handler.send_ack failed (non-fatal)"
                                                );
                                            }
                                            if inbound_tx.send(msg).is_err() {
                                                break;
                                            }
                                        }
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::Block(reason) => {
                                            tracing::debug!(channel = %channel_id, sender = msg.sender_id.as_str(), reason, "Inbound message blocked by policy");
                                        }
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::NeedsPairing(sender) => {
                                            tracing::info!(channel = %channel_id, %sender, "Inbound DM needs pairing");
                                            if inbound_tx.send(msg).is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
            connected.store(false, Ordering::SeqCst);
            let mut state = pairing_state.write().await;
            *state = PairingState::Idle;
        });

        self.shutdown_tx = Some(shutdown_tx);
        Ok(())
    }

    /// Access the `FakeWaRuntime` backing this channel, if any. Returns
    /// `None` for channels built via `new()` or any non-fake path.
    pub fn fake_runtime(&self) -> Option<Arc<FakeWaRuntime>> {
        self.fake.as_ref().map(Arc::clone)
    }

    pub fn for_test(id: impl Into<String>, config: WhatsAppConfig) -> Self {
        // Task 8: prefer the FakeWaRuntime-backed channel so the event
        // loop is exercised end-to-end in tests. The pre-Task-8
        // `test_mode` shortcut still lives in `start()` as a fallback
        // for callers that explicitly do not want a fake.
        let (channel, _fake) = Self::for_test_with_fake(id, config);
        channel
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: true,
            replies: true,
            editing: false,
            // No `delete` override. The Cloud API cannot retract a sent
            // message, so this can never be `true` for this transport.
            deletion: false,
            typing_indicator: true,
            read_receipts: true,
            rich_text: true,
            polls: false,
            // WhatsApp group icon update: wacore 0.5
            // `SetProfilePictureSpec::set_group(jid, bytes)` dispatched
            // via `Client::execute`. Wired in `set_group_icon` below.
            group_icons: true,
            max_message_length: 65536,
            max_attachment_size: 100 * 1024 * 1024,
            stream_protocol: Default::default(),
        }
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    fn status(&self) -> ChannelStatus {
        if self.test_mode {
            return self.channel_state.status();
        }
        // Route `status()` through `PairingState` (single source of truth per
        // spec §3.1 D1). `Channel::status()` is sync; the runtime's
        // `pairing_state` is a tokio `RwLock`, so we use `try_read` to avoid
        // the panic that `blocking_read` raises from within an async
        // context. If the lock happens to be held by a writer at this
        // exact instant we fall back to `Disconnected`, which is
        // `fail-closed` per CLAUDE.md §8.
        match self.pairing_state.try_read() {
            Ok(state) => state.to_channel_status(),
            Err(_) => ChannelStatus::Disconnected,
        }
    }

    async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        let state = self.pairing_state.read().await;
        match &*state {
            PairingState::WaitingQr { qr_data, .. } => Ok(PairingData::QrCode(qr_data.clone())),
            _ => Ok(PairingData::None),
        }
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.config.validate().map_err(ChannelError::ConfigError)?;

        if self.test_mode {
            self.channel_state
                .set_status(ChannelStatus::Connected)
                .await;
            tracing::info!("WhatsApp channel started in test mode");
            return Ok(());
        }

        // Fake runtime path (Task 8): the fake carries its own event
        // channel and a default `PairingState::Connected`. We mirror
        // that initial state into the channel's `pairing_state` so
        // `Channel::status()` agrees with the fake immediately after
        // `start()` returns, and skip the production-only
        // `Initializing` flip below.
        if let Some(fake) = &self.fake {
            fake.start()
                .await
                .map_err(|e| ChannelError::Internal(format!("fake runtime start: {e}")))?;
            *self.pairing_state.write().await = fake.pairing_state().await;
            return self.run_fake_event_loop(Arc::clone(fake)).await;
        }

        *self.pairing_state.write().await = PairingState::Initializing;

        // Honour the configured account id so multi-account auth/session storage
        // (vault key + per-account SQLite db) is keyed correctly. Falls back to
        // "default" to preserve single-account behaviour.
        let account_id = self
            .config
            .default_account_id
            .as_deref()
            .unwrap_or("default");
        let auth = WaAuthManager::new(account_id);
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
        let runtime = RealWaRuntime::new(auth, event_tx)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create runtime: {e}")))?;
        let runtime: Arc<dyn WaRuntime> = Arc::new(runtime);
        runtime
            .start()
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to start runtime: {e}")))?;

        // Replace the no-op reaction handler installed in `with_mode` with
        // one wired to the real `WaRuntime` adapter. The handler is the only
        // route reactions take to the network, so this is the wiring that
        // turns `config.reactions` from a parsed-and-ignored field into
        // something the inbound event loop actually consumes (Task 5).
        let reaction_handler = Arc::new(ReactionHandler::new(
            self.config.reactions.level,
            self.config.reactions.ack.clone(),
            Arc::new(WaRuntimeReactionAdapter {
                runtime: Arc::clone(&runtime),
            }),
        ));
        self.reaction_handler = Some(Arc::clone(&reaction_handler));

        let connected = Arc::clone(&self.connected);
        let pairing_state = Arc::clone(&self.pairing_state);
        let inbound_tx = self.channel_state.sender();
        let channel_id = self.info.id.clone();
        let history_buffer = Arc::clone(&self.history_buffer);
        let reaction_handler = Arc::clone(&reaction_handler);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let access = AccessConfig {
            dm_policy: self.config.access.dm_policy,
            allow_from: self.config.access.allow_from.clone(),
            group_policy: self.config.access.group_policy,
            group_allow_from: self.config.access.group_allow_from.clone(),
            groups: self.config.access.groups.clone(),
        };
        let policy = crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicy::new(
            access,
            vec![],
        );
        let driver = PairingStateDriver::new(Arc::clone(&pairing_state));
        let mut shutdown_rx = shutdown_rx;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = event_rx.recv() => {
                        use whatsapp_rust::types::events::Event;
                        // Drive PairingState from the 4 events that have a
                        // stable source in `whatsapp_rust::types::events::Event`
                        // (Task 2). Other events are a no-op for PairingState.
                        driver.apply(&event).await;
                        match event {
                            Event::Connected(_) => {
                                connected.store(true, Ordering::SeqCst);
                            }
                            Event::Disconnected(_) => {
                                connected.store(false, Ordering::SeqCst);
                            }
                            _ => {
                                if let Some(msg) = crate::gateway::interfaces::whatsapp::wa_inbound::mapper::map_event_to_inbound(&event, &channel_id, &history_buffer).await {
                                    match policy.evaluate(&msg) {
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::Accept => {
                                            // Spec §3.2 / Task 5: fire the configured
                                            // ack reaction (pre-reply emoji) right
                                            // after the inbound message is accepted
                                            // by policy. `ReactionHandler::send_ack`
                                            // is a no-op for `Off`/`Minimal` levels
                                            // and for group `Mentions` mode, so this
                                            // is safe to call unconditionally on
                                            // every accepted inbound.
                                            if let Err(e) = reaction_handler.send_ack(&msg).await {
                                                tracing::debug!(
                                                    channel = %channel_id,
                                                    error = %e,
                                                    "reaction_handler.send_ack failed (non-fatal)"
                                                );
                                            }
                                            if inbound_tx.send(msg).is_err() {
                                                break;
                                            }
                                        }
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::Block(reason) => {
                                            tracing::debug!(channel = %channel_id, sender = msg.sender_id.as_str(), reason, "Inbound message blocked by policy");
                                        }
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::NeedsPairing(sender) => {
                                            tracing::info!(channel = %channel_id, %sender, "Inbound DM needs pairing");
                                            if inbound_tx.send(msg).is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
            connected.store(false, Ordering::SeqCst);
            let mut state = pairing_state.write().await;
            *state = PairingState::Idle;
        });

        self.runtime = Some(runtime);
        self.shutdown_tx = Some(shutdown_tx);
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await;
        }
        *self.pairing_state.write().await = PairingState::Idle;
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.test_mode {
            return Ok(SendResult {
                message_id: MessageId::new(format!(
                    "wa-test-{}",
                    chrono::Utc::now().timestamp_millis()
                )),
                timestamp: chrono::Utc::now(),
            });
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        let message_id = runtime
            .send_message(message)
            .await
            .map_err(|e| ChannelError::Internal(format!("send_message: {e}")))?;
        Ok(SendResult {
            message_id,
            timestamp: chrono::Utc::now(),
        })
    }

    async fn send_typing(
        &self,
        conversation_id: &crate::gateway::channel::ConversationId,
    ) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        runtime
            .send_typing(conversation_id.as_str())
            .await
            .map_err(|e| ChannelError::Internal(format!("send_typing: {e}")))
    }

    async fn mark_read(&self, message_id: &MessageId) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        runtime
            .mark_read(message_id.as_str())
            .await
            .map_err(|e| ChannelError::Internal(format!("mark_read: {e}")))
    }

    async fn react(
        &self,
        conversation_id: &crate::gateway::channel::ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        runtime
            .send_reaction(conversation_id.as_str(), message_id.as_str(), reaction)
            .await
            .map_err(|e| ChannelError::Internal(format!("send_reaction: {e}")))
    }

    async fn set_group_icon(
        &self,
        conversation_id: &crate::gateway::channel::ConversationId,
        icon_data_url: &str,
    ) -> ChannelResult<()> {
        // Pre-decode the data URL (RFC 2397) so we can fail with a clean
        // ChannelError::Internal before touching the runtime. The shared
        // decoder (src/gateway/data_url.rs) is the same one BlueBubbles'
        // set_chat_icon uses — single source of truth for the contract.
        let (_mime, bytes) = crate::gateway::data_url::decode(icon_data_url)
            .map_err(|e| ChannelError::Internal(format!("set_group_icon: {e}")))?;
        if self.test_mode {
            // test_mode stands in for an unconnected adapter; mirror the
            // other methods' behavior (no-op) rather than surfacing a
            // transport-level success.
            return Ok(());
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        runtime
            .set_group_picture(conversation_id.as_str(), bytes)
            .await
            .map_err(|e| ChannelError::Internal(format!("set_group_picture: {e}")))
    }
}

/// Pre-`start()` and `test_mode` stand-in for the real sender. Returning
/// `Ok(())` keeps `ReactionHandler::send_ack` callable from anywhere but
/// guarantees no network traffic until the real adapter is wired in.
struct NoopReactionSender;

#[async_trait]
impl ReactionSender for NoopReactionSender {
    async fn send_reaction(&self, _jid: &str, _msg_id: &str, _emoji: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Thin adapter that routes `ReactionSender::send_reaction` to
/// `WaRuntime::send_reaction`. `WaRuntime` is cheaply `Clone` (all internal
/// state is `Arc`/`Mutex`/`Sender`); sharing one `Arc<dyn WaRuntime>` clone
/// with the handler is the minimum-friction wiring called for by spec §3.2.
struct WaRuntimeReactionAdapter {
    runtime: Arc<dyn WaRuntime>,
}

#[async_trait]
impl ReactionSender for WaRuntimeReactionAdapter {
    async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> Result<(), String> {
        self.runtime
            .send_reaction(jid, msg_id, emoji)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Drives [`PairingState`] from `whatsapp_rust` runtime events.
///
/// Owns only the `PairingState`; the legacy atomic `connected` flag is
/// kept by the caller. This is intentionally the thinnest possible
/// adapter so that `to_channel_status()` stays the single source of
/// truth for `Channel::status()` (Task 3).
///
/// wacore does not emit a generic `Scanned` event for QR-scanned flow
/// (only `QrScannedWithoutMultidevice`), so the spec's `Scanned` arm
/// from §3.1 D1 is omitted. `PairError` is mapped to `Failed` because
/// it carries a concrete error reason and would otherwise become a
/// silent no-op.
struct PairingStateDriver {
    state: Arc<RwLock<PairingState>>,
}

impl PairingStateDriver {
    fn new(state: Arc<RwLock<PairingState>>) -> Self {
        Self { state }
    }

    async fn apply(&self, event: &whatsapp_rust::types::events::Event) {
        use whatsapp_rust::types::events::Event;
        let mut s = self.state.write().await;
        match event {
            Event::PairingQrCode { code, timeout } => {
                let expires_at = chrono::Utc::now()
                    + chrono::Duration::from_std(*timeout)
                        .unwrap_or_else(|_| chrono::Duration::seconds(60));
                *s = PairingState::WaitingQr {
                    qr_data: code.clone(),
                    expires_at,
                };
            }
            Event::PairSuccess(p) => {
                *s = PairingState::Connected {
                    device_name: p.business_name.clone(),
                    phone_number: p.id.to_string(),
                };
            }
            Event::PairError(p) => {
                *s = PairingState::Failed {
                    error: p.error.clone(),
                };
            }
            Event::Disconnected(_) => {
                *s = PairingState::Disconnected {
                    reason: "remote disconnected".to_string(),
                };
            }
            _ => {}
        }
    }
}

/// Factory for creating `WhatsApp` channels
pub struct WhatsAppChannelFactory;

#[async_trait]
impl ChannelFactory for WhatsAppChannelFactory {
    fn channel_type(&self) -> &str {
        "whatsapp"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: WhatsAppConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid WhatsApp config: {e}")))?;
        Ok(Box::new(WhatsAppChannel::new("whatsapp", config)))
    }
}

fn whatsapp_factory_creator(
    _config: crate::gateway::channel::ChannelConfig,
) -> ChannelResult<Arc<dyn ChannelFactory>> {
    Ok(Arc::new(WhatsAppChannelFactory))
}

/// Register the `WhatsApp` channel factory with the global channel plugin
/// registry so that config-driven creation (`create_channel_from_config`)
/// can instantiate it. Without this the entire `WhatsApp` channel is
/// unreachable through configuration.
pub fn register_with_plugin() {
    let _ = crate::gateway::interfaces::plugin::register("whatsapp", whatsapp_factory_creator);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::whatsapp::pairing::PairingState;

    #[test]
    fn test_factory_channel_type() {
        assert_eq!(WhatsAppChannelFactory.channel_type(), "whatsapp");
    }

    #[test]
    fn test_factory_creator_builds_factory() {
        let config = crate::gateway::channel::ChannelConfig {
            id: "whatsapp".into(),
            channel_type: "whatsapp".into(),
            enabled: true,
            config: serde_json::json!({}),
        };
        let factory = whatsapp_factory_creator(config).expect("factory creator should succeed");
        assert_eq!(factory.channel_type(), "whatsapp");
    }

    /// TDD Task 2: PairingStateDriver must drive `PairingState` from the
    /// 4 events that have a stable source in `whatsapp_rust::types::events::Event`
    /// (`PairingQrCode`, `PairSuccess`, `PairError`, `Disconnected`). wacore does
    /// not emit a generic `Scanned` event, so that arm is omitted.
    #[tokio::test]
    async fn pairing_state_driven_by_events() {
        let state = Arc::new(RwLock::new(PairingState::Idle));
        let driver = PairingStateDriver::new(state.clone());

        // QR ready: Idle → WaitingQr
        driver
            .apply(&whatsapp_rust::types::events::Event::PairingQrCode {
                code: "abc".to_string(),
                timeout: std::time::Duration::from_secs(60),
            })
            .await;
        assert!(
            matches!(*state.read().await, PairingState::WaitingQr { .. }),
            "expected WaitingQr, got {:?}",
            *state.read().await
        );

        // PairSuccess: WaitingQr → Connected
        let pair_success = whatsapp_rust::types::events::PairSuccess {
            id: whatsapp_rust::Jid::new("1234", "s.whatsapp.net"),
            lid: whatsapp_rust::Jid::new("abcd", "lid"),
            business_name: "My Phone".to_string(),
            platform: "smba".to_string(),
        };
        driver
            .apply(&whatsapp_rust::types::events::Event::PairSuccess(
                pair_success,
            ))
            .await;
        match &*state.read().await {
            PairingState::Connected {
                device_name,
                phone_number,
            } => {
                assert_eq!(device_name, "My Phone");
                assert_eq!(phone_number, "1234@s.whatsapp.net");
            }
            other => panic!("expected Connected, got {other:?}"),
        }

        // Disconnected: Connected → Disconnected
        driver
            .apply(&whatsapp_rust::types::events::Event::Disconnected(
                whatsapp_rust::types::events::Disconnected,
            ))
            .await;
        assert!(matches!(
            *state.read().await,
            PairingState::Disconnected { .. }
        ));

        // PairError: Disconnected → Failed
        driver
            .apply(&whatsapp_rust::types::events::Event::PairError(
                whatsapp_rust::types::events::PairError {
                    id: whatsapp_rust::Jid::new("1234", "s.whatsapp.net"),
                    lid: whatsapp_rust::Jid::new("abcd", "lid"),
                    business_name: String::new(),
                    platform: String::new(),
                    error: "401 logout".to_string(),
                },
            ))
            .await;
        assert!(matches!(*state.read().await, PairingState::Failed { .. }));
    }

    /// TDD Task 3: `WhatsAppChannel::status()` must read `PairingState`
    /// (single source of truth per spec §3.1 D1), not the legacy
    /// `runtime.connection_state()` path. Without this fix, mutating
    /// `pairing_state` has no effect on `status()` — which is the bug the
    /// spec calls out under CLAUDE.md §8 (`fail-closed`).
    ///
    /// Deviation from the plan text: the plan asserts `ChannelStatus::Pairing`
    /// for `QrExpired`, but `PairingState::to_channel_status()` is unchanged
    /// per spec §5.2 and maps `QrExpired` to `ChannelStatus::Connecting`. We
    /// assert the actual mapped value.
    #[tokio::test]
    async fn status_reads_pairing_state() {
        use crate::gateway::channel::ChannelStatus;
        let channel = WhatsAppChannel::new("wa-test", WhatsAppConfig::default());
        *channel.pairing_state.write().await = PairingState::QrExpired;
        assert_eq!(channel.status(), ChannelStatus::Connecting);

        *channel.pairing_state.write().await = PairingState::Connected {
            device_name: "Phone".to_string(),
            phone_number: "+1234".to_string(),
        };
        assert_eq!(channel.status(), ChannelStatus::Connected);

        *channel.pairing_state.write().await = PairingState::Failed {
            error: "x".to_string(),
        };
        assert_eq!(channel.status(), ChannelStatus::Error);
    }

    #[test]
    fn capabilities_advertise_group_icons() {
        // Pin the flip so a future edit cannot silently turn off
        // group_icons and orphan the WhatsApp set_group_icon impl.
        let caps = WhatsAppChannel::capabilities();
        assert!(
            caps.group_icons,
            "WhatsApp has wacore SetProfilePictureSpec::set_group — wire must reflect it"
        );
    }

    /// The fake records every (group_jid, byte_len) tuple so a test can
    /// assert the channel routed the data URL through decode + send
    /// without reaching for a real WhatsApp connection. Pin both the
    /// jid and the decoded byte length here so a future edit cannot
    /// silently swap to 0-byte or stub-send the bytes.
    #[tokio::test]
    async fn set_group_icon_routes_through_fake_runtime() {
        use crate::gateway::channel::ConversationId;
        let config = WhatsAppConfig::default();
        let (channel, fake) = WhatsAppChannel::for_test_with_fake("wa-test", config);
        // `for_test_with_fake` already wires `fake` as the runtime;
        // no manual reassignment.

        // 8-byte base64 payload: iVBORw0KGgo == the 8-byte PNG signature
        // (0x89 0x50 0x4E 0x47 0x0D 0x0A 0x1A 0x0A). The fake records
        // byte_len — we want the decoded length, not the base64 length.
        let data_url = "data:image/png;base64,iVBORw0KGgo=";
        let cid = ConversationId::new("1234567890@g.us");

        channel
            .set_group_icon(&cid, data_url)
            .await
            .expect("set_group_icon should succeed via fake");

        let recorded = fake.sent_group_pictures().await;
        assert_eq!(recorded.len(), 1, "fake should record exactly one call");
        assert_eq!(recorded[0].0, "1234567890@g.us");
        assert_eq!(
            recorded[0].1, 8,
            "decoded bytes should be the 8-byte PNG signature"
        );
    }

    /// Non-base64 data: URLs (RFC 2397 violations) are rejected at the
    /// channel layer before reaching the runtime. The fake must therefore
    /// see zero calls — the channel's surface is the contract, the fake
    /// is just bookkeeping.
    #[tokio::test]
    async fn set_group_icon_rejects_non_base64_data_url() {
        use crate::gateway::channel::ConversationId;
        let config = WhatsAppConfig::default();
        let (channel, fake) = WhatsAppChannel::for_test_with_fake("wa-test", config);

        let cid = ConversationId::new("1234567890@g.us");
        let err = channel
            .set_group_icon(&cid, "data:image/png,Hello%20World")
            .await
            .expect_err("non-base64 must be rejected");
        // The error names the data URL contract, not a generic failure.
        assert!(
            err.to_string().contains("only base64"),
            "expected RFC 2397 violation, got: {err}"
        );
        let recorded = fake.sent_group_pictures().await;
        assert!(
            recorded.is_empty(),
            "fake must not be called on validation failure"
        );
    }
}
