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

pub mod account;
pub mod account_registry;
pub mod history_buffer;
pub mod media;
pub mod reactions;
pub mod types;
pub mod wa_auth;
pub mod wa_inbound;
pub mod wa_outbound;
pub mod wa_policy;
pub mod wa_runtime;

pub use config::{
    AccessConfig, DeliveryConfig, ReactionConfig, WhatsAppAccountConfig, WhatsAppConfig,
};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, MessageId, OutboundMessage, PairingData,
    SendResult,
};
use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
use crate::gateway::interfaces::whatsapp::wa_runtime::{ConnectionState, WaRuntime};
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
            deletion: true,
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
        match self.runtime.as_ref().map(|r| r.connection_state()) {
            Some(ConnectionState::Connected) => ChannelStatus::Connected,
            Some(ConnectionState::Connecting) => ChannelStatus::Connecting,
            Some(ConnectionState::Pairing) => ChannelStatus::Pairing,
            Some(ConnectionState::Error) => ChannelStatus::Error,
            _ => ChannelStatus::Disconnected,
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
        let mut shutdown_rx = shutdown_rx;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = event_rx.recv() => {
                        use whatsapp_rust::types::events::Event;
                        match event {
                            Event::PairingQrCode { code, timeout } => {
                                let expires_at = chrono::Utc::now() + chrono::Duration::from_std(timeout).unwrap_or_else(|_| chrono::Duration::seconds(60));
                                let mut state = pairing_state.write().await;
                                *state = PairingState::WaitingQr { qr_data: code, expires_at };
                            }
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
}
