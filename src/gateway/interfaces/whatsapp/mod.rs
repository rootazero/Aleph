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
use crate::gateway::interfaces::whatsapp::wa_runtime::WaRuntime;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, Ordering};
use async_trait::async_trait;
use tokio::sync::{oneshot, RwLock};

use pairing::PairingState;

/// `WhatsApp` channel implementation backed by native Rust runtime.
pub struct WhatsAppChannel {
    info: ChannelInfo,
    config: WhatsAppConfig,
    channel_state: ChannelState,
    runtime: Option<WaRuntime>,
    pairing_state: Arc<RwLock<PairingState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    connected: Arc<AtomicBool>,
    test_mode: bool,
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

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            runtime: None,
            pairing_state: Arc::new(RwLock::new(PairingState::Idle)),
            shutdown_tx: None,
            connected: Arc::new(AtomicBool::new(false)),
            test_mode,
        }
    }

    pub fn for_test(id: impl Into<String>, config: WhatsAppConfig) -> Self {
        Self::with_mode(id, config, true)
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
        let mut runtime = WaRuntime::new(auth, event_tx)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create runtime: {e}")))?;
        runtime.start().await?;

        let connected = Arc::clone(&self.connected);
        let pairing_state = Arc::clone(&self.pairing_state);
        let inbound_tx = self.channel_state.sender();
        let channel_id = self.info.id.clone();
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
                                if let Some(msg) = crate::gateway::interfaces::whatsapp::wa_inbound::mapper::map_event_to_inbound(&event, &channel_id) {
                                    match policy.evaluate(&msg) {
                                        crate::gateway::interfaces::whatsapp::wa_inbound::policy::InboundPolicyResult::Accept => {
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
        if let Some(mut runtime) = self.runtime.take() {
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
        let message_id = runtime.send_message(message).await?;
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
        runtime.send_typing(conversation_id.as_str()).await
    }

    async fn mark_read(&self, message_id: &MessageId) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        runtime.mark_read(message_id.as_str()).await
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
}
