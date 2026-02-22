//! WhatsApp Channel Implementation
//!
//! Integration with WhatsApp using a bridge or native Rust library.
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

pub mod bridge_manager;
pub mod bridge_protocol;
pub mod config;
pub mod message;
pub mod pairing;
pub mod rpc_client;

pub use config::WhatsAppConfig;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelStatus, InboundMessage, MessageId, OutboundMessage, PairingData,
    SendResult,
};
use async_trait::async_trait;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

use bridge_manager::{BridgeManager, BridgeManagerConfig};
use bridge_protocol::BridgeEvent;
use pairing::PairingState;
use rpc_client::BridgeRpcClient;

/// WhatsApp channel implementation backed by a Go bridge process.
pub struct WhatsAppChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: WhatsAppConfig,
    /// Inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken by the channel registry)
    #[allow(dead_code)]
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Bridge process manager
    bridge_manager: BridgeManager,
    /// JSON-RPC client for communicating with the bridge
    rpc_client: Option<BridgeRpcClient>,
    /// Fine-grained pairing state
    pairing_state: Arc<RwLock<PairingState>>,
    /// Shutdown signal sender for the event loop
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl WhatsAppChannel {
    /// Create a new WhatsApp channel
    pub fn new(id: impl Into<String>, config: WhatsAppConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "WhatsApp".to_string(),
            channel_type: "whatsapp".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        let bridge_config = Self::build_bridge_config(&config);
        let bridge_manager = BridgeManager::new(bridge_config);

        Self {
            info,
            config,
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            bridge_manager,
            rpc_client: None,
            pairing_state: Arc::new(RwLock::new(PairingState::Idle)),
            shutdown_tx: None,
        }
    }

    /// Get WhatsApp-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: true,
            replies: true,
            editing: false, // WhatsApp editing is limited
            deletion: true,
            typing_indicator: true,
            read_receipts: true,
            rich_text: true,
            max_message_length: 65536,
            max_attachment_size: 100 * 1024 * 1024, // 100MB
        }
    }

    /// Build a BridgeManagerConfig from the WhatsAppConfig.
    fn build_bridge_config(config: &WhatsAppConfig) -> BridgeManagerConfig {
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("whatsapp");

        let binary_path = config
            .bridge_binary
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("whatsapp-bridge"));

        BridgeManagerConfig {
            binary_path,
            socket_path: base_dir.join("bridge.sock"),
            data_dir: base_dir.join("data"),
            max_restarts: config.max_restarts,
            restart_delay_secs: 3,
        }
    }
}

/// Background event loop that processes BridgeEvents.
///
/// Updates pairing state and forwards inbound messages to the channel's
/// inbound_tx sender.
async fn event_loop(
    mut event_rx: mpsc::Receiver<BridgeEvent>,
    pairing_state: Arc<RwLock<PairingState>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
    channel_id: ChannelId,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                match event {
                    BridgeEvent::Ready => {
                        tracing::info!(channel = %channel_id, "WhatsApp bridge ready");
                    }
                    BridgeEvent::Qr { qr_data, expires_in_secs } => {
                        tracing::info!(
                            channel = %channel_id,
                            expires_in_secs,
                            "QR code received for pairing"
                        );
                        *pairing_state.write().await = PairingState::WaitingQr {
                            qr_data,
                            expires_at: Utc::now() + chrono::Duration::seconds(expires_in_secs as i64),
                        };
                    }
                    BridgeEvent::QrExpired => {
                        tracing::info!(channel = %channel_id, "QR code expired");
                        *pairing_state.write().await = PairingState::QrExpired;
                    }
                    BridgeEvent::Scanned => {
                        tracing::info!(channel = %channel_id, "QR code scanned");
                        *pairing_state.write().await = PairingState::Scanned;
                    }
                    BridgeEvent::Syncing { progress } => {
                        tracing::debug!(
                            channel = %channel_id,
                            progress,
                            "Syncing in progress"
                        );
                        *pairing_state.write().await = PairingState::Syncing { progress };
                    }
                    BridgeEvent::Connected { device_name, phone_number } => {
                        tracing::info!(
                            channel = %channel_id,
                            device_name = %device_name,
                            phone_number = %phone_number,
                            "WhatsApp connected"
                        );
                        *pairing_state.write().await = PairingState::Connected {
                            device_name,
                            phone_number,
                        };
                    }
                    BridgeEvent::Disconnected { reason } => {
                        tracing::warn!(
                            channel = %channel_id,
                            reason = %reason,
                            "WhatsApp disconnected"
                        );
                        *pairing_state.write().await = PairingState::Disconnected { reason };
                    }
                    BridgeEvent::Error { message: msg } => {
                        tracing::error!(
                            channel = %channel_id,
                            error = %msg,
                            "WhatsApp bridge error"
                        );
                        *pairing_state.write().await = PairingState::Failed { error: msg };
                    }
                    BridgeEvent::Message { .. } => {
                        if let Some(msg) = message::bridge_message_to_inbound(&event, &channel_id) {
                            if inbound_tx.send(msg).await.is_err() {
                                tracing::debug!(
                                    channel = %channel_id,
                                    "Inbound receiver dropped, stopping event loop"
                                );
                                break;
                            }
                        }
                    }
                    BridgeEvent::Receipt { message_id, receipt_type } => {
                        tracing::debug!(
                            channel = %channel_id,
                            message_id = %message_id,
                            receipt_type = %receipt_type,
                            "Receipt received"
                        );
                    }
                }
            }
            _ = &mut shutdown_rx => {
                tracing::info!(channel = %channel_id, "Event loop shutdown requested");
                break;
            }
        }
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn status(&self) -> ChannelStatus {
        // status() is sync but pairing_state uses tokio::sync::RwLock.
        // Use try_read() which is non-blocking; fall back to Connecting
        // if the lock is contended (likely during state transitions).
        match self.pairing_state.try_read() {
            Ok(state) => state.to_channel_status(),
            Err(_) => ChannelStatus::Connecting,
        }
    }

    async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        let state = self.pairing_state.read().await;
        match &*state {
            PairingState::WaitingQr { qr_data, .. } => {
                Ok(PairingData::QrCode(qr_data.clone()))
            }
            _ => Ok(PairingData::None),
        }
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.config.validate().map_err(ChannelError::ConfigError)?;

        // 1. Set pairing state to Initializing
        *self.pairing_state.write().await = PairingState::Initializing;
        tracing::info!("Starting WhatsApp channel...");

        // 2. Start the bridge process
        self.bridge_manager.start().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to start bridge: {}", e))
        })?;

        // 3. Create event channel and RPC client
        let (event_tx, event_rx) = mpsc::channel(64);
        let socket_path = self.bridge_manager.socket_path().clone();
        let rpc_client = BridgeRpcClient::new(&socket_path, event_tx);

        // 4. Connect RPC client with retries
        rpc_client.connect(5, 500).await.map_err(|e| {
            ChannelError::Internal(format!("Failed to connect to bridge RPC: {}", e))
        })?;

        // 5. Tell the bridge to connect to WhatsApp
        let _: serde_json::Value = rpc_client
            .call("bridge.connect", None)
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("bridge.connect RPC failed: {}", e))
            })?;

        // 6. Spawn event loop
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let pairing_state = Arc::clone(&self.pairing_state);
        let inbound_tx = self.inbound_tx.clone();
        let channel_id = self.info.id.clone();

        tokio::spawn(event_loop(
            event_rx,
            pairing_state,
            inbound_tx,
            channel_id,
            shutdown_rx,
        ));

        self.rpc_client = Some(rpc_client);
        self.shutdown_tx = Some(shutdown_tx);

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping WhatsApp channel...");

        // 1. Send shutdown signal to event loop
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // 2. Disconnect RPC client
        if let Some(ref rpc_client) = self.rpc_client {
            rpc_client.disconnect().await;
        }
        self.rpc_client = None;

        // 3. Stop bridge process
        self.bridge_manager.stop().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to stop bridge: {}", e))
        })?;

        // 4. Set pairing state to Idle
        *self.pairing_state.write().await = PairingState::Idle;

        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        // Check if connected
        let state = self.pairing_state.read().await;
        if !state.is_connected() {
            return Err(ChannelError::NotConnected(
                format!("WhatsApp not connected (state: {})", state.description()),
            ));
        }
        drop(state);

        // Get the RPC client
        let rpc_client = self.rpc_client.as_ref().ok_or_else(|| {
            ChannelError::Internal("RPC client not initialized".to_string())
        })?;

        // Convert outbound message to bridge SendRequest
        let send_request = message::outbound_to_send_request(&message);
        let params = serde_json::to_value(&send_request).map_err(|e| {
            ChannelError::SendFailed(format!("Failed to serialize send request: {}", e))
        })?;

        // Call bridge.send RPC
        let response: bridge_protocol::SendResponse = rpc_client
            .call("bridge.send", Some(params))
            .await
            .map_err(|e| {
                ChannelError::SendFailed(format!("bridge.send RPC failed: {}", e))
            })?;

        Ok(SendResult {
            message_id: MessageId::new(response.id),
            timestamp: Utc::now(),
        })
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        // NOTE: This always returns None because we can't take from &self.
        // The receiver is taken during construction via take() in the registry.
        None
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
