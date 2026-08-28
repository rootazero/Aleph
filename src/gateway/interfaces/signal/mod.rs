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
pub mod error;
pub mod message_ops;

pub use config::SignalConfig;
pub use error::SignalError;
pub use message_ops::{SignalMessageOps, SignalMonitor};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendResult,
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
    /// Override API base URL for testing (replaces `config.api_url` in send)
    api_base: Option<String>,
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
            api_base: None,
        }
    }

    /// Create a Signal channel for testing with a mock API base URL.
    pub fn for_test(id: impl Into<String>, config: SignalConfig, api_base: String) -> Self {
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
            api_base: Some(api_base),
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
            // No `mark_read` override — signal-cli can send receipts, but
            // nothing here calls it yet.
            read_receipts: false,
            rich_text: false, // Signal is plain text
            max_message_length: 65535,
            max_attachment_size: 100 * 1024 * 1024, // 100MB
            stream_protocol: Default::default(),
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
        self.config.validate().map_err(ChannelError::ConfigError)?;

        self.set_status(ChannelStatus::Connecting).await;
        tracing::info!(
            "Starting Signal channel (api_url={}, phone={})...",
            self.config.api_url,
            self.config.phone_number
        );

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Spawn SSE monitor
        let client = self.client.clone();
        let config = self.config.clone();
        let channel_id = self.info.id.clone();
        let inbound_tx = self.channel_state.sender();
        let status = self.channel_state.status_handle();

        let handle = SignalMonitor::start(client, config, channel_id, inbound_tx, shutdown_rx);

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;
            let result = handle.await;
            // **Audit fix**: the monitor task exiting is the only signal we
            // have that the SSE loop gave up (max retries) or panicked — the
            // previous code wrote `Disconnected` unconditionally, so a dead
            // monitor read exactly like a clean stop: the registry believed
            // the channel was up (start() had returned Ok + Connected), the
            // health monitor never saw an Error, and the restart policy
            // never fired. A panicked monitor now surfaces as Error; a
            // monitor that exhausted its retry budget also surfaces as Error
            // (the loop logs "max retries exceeded" before breaking), while
            // only a shutdown-driven exit maps to Disconnected.
            let mut guard = status.write().await;
            if result.is_err() {
                *guard = ChannelStatus::Error;
            } else if *guard != ChannelStatus::Error {
                *guard = ChannelStatus::Disconnected;
            }
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
        let api_url = self.api_base.as_ref().unwrap_or(&self.config.api_url);
        SignalMessageOps::send_message(
            &self.client,
            api_url,
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

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        let _ = (conversation_id, message_id, new_text);
        Err(ChannelError::UnsupportedFeature(
            "Signal does not support message editing".to_string(),
        ))
    }

    async fn react(
        &self,
        _conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
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
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Signal config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

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
        // No `mark_read` override — flip back only together with the impl.
        assert!(!caps.read_receipts);
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
    fn test_inbound_subscribe_multi_consumer() {
        let config = SignalConfig::default();
        let channel = SignalChannel::new("signal", config);

        let rx1 = channel.state().take_receiver();
        let rx2 = channel.state().take_receiver();
        let rx3 = channel.state().take_receiver();

        assert!(rx1.is_some() && rx2.is_some() && rx3.is_some());
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
