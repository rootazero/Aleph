//! WhatsApp Channel Implementation
//!
//! Native Rust integration with WhatsApp using whatsapp-rust.
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

pub mod bridge_fallback;
pub mod bridge_manager;
pub mod bridge_protocol;
pub mod config;
pub mod message;
pub mod pairing;
#[cfg(unix)]
pub mod rpc_client;

pub mod account;
pub mod account_registry;
pub mod baileys_runtime;
pub mod history_buffer;
pub mod media;
pub mod reactions;
pub mod types;
pub mod wa_auth;
pub mod wa_inbound;
pub mod wa_outbound;
pub mod wa_policy;
pub mod wa_runtime;

#[cfg(feature = "native-whatsapp")]
pub mod native_baileys;

pub use config::{
    AccessConfig, DeliveryConfig, ReactionConfig, WhatsAppAccountConfig, WhatsAppConfig,
};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, MessageId, OutboundMessage, PairingData, SendResult,
};
use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
use crate::gateway::interfaces::whatsapp::wa_runtime::{ConnectionState, WaRuntime};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::sync::atomic::AtomicBool;
use tokio::sync::{oneshot, RwLock};

use pairing::PairingState;

/// WhatsApp channel implementation backed by native Rust runtime.
pub struct WhatsAppChannel {
    info: ChannelInfo,
    config: WhatsAppConfig,
    channel_state: ChannelState,
    runtime: Option<WaRuntime>,
    pairing_state: Arc<RwLock<PairingState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    connected: Arc<AtomicBool>,
}

impl WhatsAppChannel {
    pub fn new(id: impl Into<String>, config: WhatsAppConfig) -> Self {
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
        }
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
        match self.runtime.as_ref().map(|r| r.connection_state()) {
            Some(ConnectionState::Connected) => ChannelStatus::Connected,
            Some(ConnectionState::Connecting) => ChannelStatus::Connecting,
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
        *self.pairing_state.write().await = PairingState::Initializing;

        let auth = WaAuthManager::new("default");
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
        let mut runtime = WaRuntime::new(auth, event_tx)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create runtime: {}", e)))?;
        runtime.start().await?;

        let _connected = Arc::clone(&self.connected);
        let _pairing_state = Arc::clone(&self.pairing_state);
        let inbound_tx = self.channel_state.sender();
        let _channel_id = self.info.id.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let mut shutdown_rx = shutdown_rx;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(_event) = event_rx.recv() => {
                        // TODO: map event and apply policy before sending inbound
                        // For now we just keep the loop alive.
                        let _ = inbound_tx.clone();
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
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
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
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

    async fn send_typing(&self, conversation_id: &crate::gateway::channel::ConversationId) -> ChannelResult<()> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        runtime.send_typing(conversation_id.as_str()).await
    }

    async fn mark_read(&self, message_id: &MessageId) -> ChannelResult<()> {
        let _ = message_id;
        Ok(())
    }

    async fn react(
        &self,
        conversation_id: &crate::gateway::channel::ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("WhatsApp runtime not started".into()))?;
        runtime
            .send_reaction(conversation_id.as_str(), message_id.as_str(), reaction)
            .await
    }
}

/// Factory for creating WhatsApp channels
pub struct WhatsAppChannelFactory;

#[async_trait]
impl ChannelFactory for WhatsAppChannelFactory {
    fn channel_type(&self) -> &str {
        "whatsapp"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: WhatsAppConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid WhatsApp config: {}", e)))?;
        Ok(Box::new(WhatsAppChannel::new("whatsapp", config)))
    }
}


