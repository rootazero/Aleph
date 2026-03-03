//! Signal Channel Implementation
//!
//! Integrates with Signal via signal-cli's REST API wrapper for sending
//! and receiving messages. Uses periodic polling for message reception.
//!
//! # Protocol
//!
//! - **Receiving:** Poll `GET /v1/receive/{phone_number}` periodically
//! - **Sending:** `POST /v2/send` with `{"number":"+1...", "message":"..."}`
//! - **Auth:** None (signal-cli handles registration separately)
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "signal"
//! channel_type = "signal"
//! enabled = true
//!
//! [channels.config]
//! api_url = "http://localhost:8080"
//! phone_number = "+1234567890"
//! allowed_users = ["+9876543210"]
//! ```

pub mod config;
pub mod message_ops;

pub use config::SignalConfig;
pub use message_ops::SignalMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, InboundMessage, MessageId,
    OutboundMessage, SendResult,
};
use async_trait::async_trait;
use tokio::sync::watch;

/// Signal channel implementation using the signal-cli REST API.
pub struct SignalChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: SignalConfig,
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// HTTP client for signal-cli API calls
    client: reqwest::Client,
}

impl SignalChannel {
    /// Create a new Signal channel
    pub fn new(id: impl Into<String>, config: SignalConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Signal".to_string(),
            channel_type: "signal".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            shutdown_tx: None,
            client: reqwest::Client::new(),
        }
    }

    /// Get Signal-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: true,
            replies: true,
            editing: false,
            deletion: false,
            typing_indicator: true,
            read_receipts: true,
            rich_text: false, // Signal is plain text
            max_message_length: 65535,
            max_attachment_size: 100 * 1024 * 1024, // 100MB
        }
    }

    /// Update internal status
    async fn set_status(&self, status: ChannelStatus) {
        self.channel_state.set_status(status).await;
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Validate configuration
        self.config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        self.set_status(ChannelStatus::Connecting).await;
        tracing::info!(
            "Starting Signal channel (api_url={}, phone={})...",
            self.config.api_url,
            self.config.phone_number
        );

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

            // Spawn polling loop
            let client = self.client.clone();
            let config = self.config.clone();
            let channel_id = self.info.id.clone();
            let inbound_tx = self.channel_state.sender();
            let status = self.channel_state.status_handle();

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;

            SignalMessageOps::run_poll_loop(
                client,
                config,
                channel_id,
                inbound_tx,
                shutdown_rx,
            )
            .await;

            *status.write().await = ChannelStatus::Disconnected;
        });

        self.set_status(ChannelStatus::Connected).await;
        Ok(())

    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Signal channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        SignalMessageOps::send_message(
            &self.client,
            &self.config.api_url,
            &self.config.phone_number,
            message.conversation_id.as_str(),
            &message.text,
        )
        .await

    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        // Signal typing indicators would need signal-cli support.
        // Current signal-cli REST API doesn't expose typing indicators.
        let _ = conversation_id;
        Ok(())
    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        let _ = (message_id, new_text);
        Err(ChannelError::UnsupportedFeature(
            "Signal does not support message editing".to_string(),
        ))
    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        // Signal supports reactions but signal-cli REST API support varies.
        let _ = (message_id, reaction);
        Err(ChannelError::UnsupportedFeature(
            "Signal reactions require signal-cli v0.12+ REST API".to_string(),
        ))
    }
}

/// Factory for creating Signal channels
pub struct SignalChannelFactory;

#[async_trait]
impl ChannelFactory for SignalChannelFactory {
    fn channel_type(&self) -> &str {
        "signal"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: SignalConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Signal config: {}", e)))?;

        config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        Ok(Box::new(SignalChannel::new("signal", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = SignalChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(caps.audio);
        assert!(caps.video);
        assert!(caps.reactions);
        assert!(caps.replies);
        assert!(!caps.editing);
        assert!(!caps.deletion);
        assert!(caps.typing_indicator);
        assert!(caps.read_receipts);
        assert!(!caps.rich_text);
        assert_eq!(caps.max_message_length, 65535);
        assert_eq!(caps.max_attachment_size, 100 * 1024 * 1024);
    }

    #[test]
    fn test_channel_creation() {
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };
        let channel = SignalChannel::new("signal-test", config);
        assert_eq!(channel.info().id.as_str(), "signal-test");
        assert_eq!(channel.info().channel_type, "signal");
        assert_eq!(channel.info().name, "Signal");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = SignalConfig::default();
        let channel = SignalChannel::new("signal", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = SignalConfig::default();
        let channel = SignalChannel::new("signal", config);

        // First take should succeed (via ChannelState)
        assert!(channel.state().take_receiver().is_some());

        // Second take should return None
        assert!(channel.state().take_receiver().is_none());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = SignalChannelFactory;
        assert_eq!(factory.channel_type(), "signal");

        let config = serde_json::json!({
            "phone_number": "+1234567890"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "signal");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = SignalChannelFactory;

        // Missing required phone_number field
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_phone() {
        let factory = SignalChannelFactory;

        let config = serde_json::json!({
            "phone_number": "1234567890"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_with_all_options() {
        let factory = SignalChannelFactory;

        let config = serde_json::json!({
            "api_url": "http://signal:9080",
            "phone_number": "+1234567890",
            "allowed_users": ["+9876543210"],
            "poll_interval_secs": 5,
            "send_typing": false
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_start_without_feature() {
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };
        let _channel = SignalChannel::new("signal", config);

        // Without the signal feature, start should return UnsupportedFeature.
        // When the signal feature IS enabled, start() requires a live signal-cli
        // instance which cannot be tested in unit tests, so this test only
        // validates construction succeeds.
    }
}
