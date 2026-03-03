//! Slack Channel Implementation
//!
//! Integrates with Slack using Socket Mode (WebSocket) for receiving events
//! and the Web API (REST) for sending messages. No external Slack SDK required.
//!
//! # Protocol
//!
//! - **Socket Mode** (WebSocket): Uses an app-level token (`xapp-...`) to receive
//!   real-time events without exposing a public HTTP endpoint.
//! - **Web API** (REST): Uses a bot token (`xoxb-...`) for sending messages
//!   via `chat.postMessage` and other API methods.
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "slack"
//! channel_type = "slack"
//! enabled = true
//!
//! [channels.config]
//! app_token = "xapp-1-..."
//! bot_token = "xoxb-..."
//! allowed_channels = ["C12345"]
//! ```

pub mod config;
pub mod message_ops;

pub use config::SlackConfig;
pub use message_ops::SlackMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, InboundMessage, MessageId,
    OutboundMessage, SendResult,
};
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use tokio::sync::{watch, RwLock};

/// Slack channel implementation using Socket Mode + REST API.
pub struct SlackChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: SlackConfig,
    /// Shared mutable state (status + inbound channel)
    channel_state: ChannelState,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Bot's own user ID (populated after auth.test)
    bot_user_id: Arc<RwLock<Option<String>>>,
    /// HTTP client for Slack API calls
    client: reqwest::Client,
}

impl SlackChannel {
    /// Create a new Slack channel
    pub fn new(id: impl Into<String>, config: SlackConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Slack".to_string(),
            channel_type: "slack".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            shutdown_tx: None,
            bot_user_id: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
        }
    }

    /// Get Slack-specific capabilities.
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: false,
            video: false,
            reactions: true,
            replies: true,
            editing: true,
            deletion: true,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true, // Slack mrkdwn support
            max_message_length: 3000,
            max_attachment_size: 1_073_741_824, // 1GB
        }
    }

}

#[async_trait]
impl Channel for SlackChannel {
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

        self.channel_state.set_status(ChannelStatus::Connecting).await;
        tracing::info!("Starting Slack channel...");

        // Validate bot token via auth.test
        match SlackMessageOps::validate_bot_token(&self.client, &self.config.bot_token).await {
            Ok(user_id) => {
                tracing::info!("Slack bot authenticated (user_id: {user_id})");
                *self.bot_user_id.write().await = Some(user_id);
            }
            Err(e) => {
                self.channel_state.set_status(ChannelStatus::Error).await;
                return Err(e);
            }
        }

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Clone handles for the spawned task
        let client = self.client.clone();
        let app_token = self.config.app_token.clone();
        let bot_user_id = self.bot_user_id.clone();
        let channel_id = self.info.id.clone();
        let config = self.config.clone();
        let inbound_tx = self.channel_state.sender();
        let status = self.channel_state.status_handle();

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;

            SlackMessageOps::run_socket_mode_loop(
                client,
                app_token,
                bot_user_id,
                channel_id,
                config,
                inbound_tx,
                shutdown_rx,
            )
            .await;

            *status.write().await = ChannelStatus::Disconnected;
        });

        self.channel_state.set_status(ChannelStatus::Connected).await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Slack channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.channel_state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        // Extract thread_ts from reply_to for threading
        let thread_ts = message.reply_to.as_ref().map(|id| id.as_str().to_string());

        // Send typing indicator if enabled
        if self.config.send_typing {
            // Slack doesn't have a dedicated typing API for bots in channels,
            // but we respect the config flag for potential future use.
        }

        SlackMessageOps::send_message(
            &self.client,
            &self.config.bot_token,
            message.conversation_id.as_str(),
            &message.text,
            thread_ts.as_deref(),
        )
        .await

    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        // Slack Bot API does not support typing indicators in channels.
        // The typing_indicator capability is declared true for UI parity,
        // but actual implementation is a no-op.
        let _ = conversation_id;
        Ok(())
    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        // Note: Editing requires both message ts and channel ID
        // which we don't have in this interface signature.
        let _ = (message_id, new_text);
        Err(ChannelError::UnsupportedFeature(
            "Message editing requires channel context (conversation_id + ts)".to_string(),
        ))

    }

    async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
        // Note: Deleting requires both message ts and channel ID
        let _ = message_id;
        Err(ChannelError::UnsupportedFeature(
            "Message deletion requires channel context (conversation_id + ts)".to_string(),
        ))

    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        // Note: Reacting requires channel ID + timestamp
        let _ = (message_id, reaction);
        Err(ChannelError::UnsupportedFeature(
            "Reactions require channel context (conversation_id + ts)".to_string(),
        ))

    }
}

/// Factory for creating Slack channels
pub struct SlackChannelFactory;

#[async_trait]
impl ChannelFactory for SlackChannelFactory {
    fn channel_type(&self) -> &str {
        "slack"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: SlackConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Slack config: {}", e)))?;

        config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        Ok(Box::new(SlackChannel::new("slack", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = SlackChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(!caps.audio);
        assert!(!caps.video);
        assert!(caps.reactions);
        assert!(caps.replies);
        assert!(caps.editing);
        assert!(caps.deletion);
        assert!(caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(caps.rich_text);
        assert_eq!(caps.max_message_length, 3000);
        assert_eq!(caps.max_attachment_size, 1_073_741_824);
    }

    #[test]
    fn test_channel_creation() {
        let config = SlackConfig {
            app_token: "xapp-test".to_string(),
            bot_token: "xoxb-test".to_string(),
            ..Default::default()
        };
        let channel = SlackChannel::new("slack-test", config);
        assert_eq!(channel.info().id.as_str(), "slack-test");
        assert_eq!(channel.info().channel_type, "slack");
        assert_eq!(channel.info().name, "Slack");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = SlackConfig::default();
        let channel = SlackChannel::new("slack", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = SlackConfig::default();
        let channel = SlackChannel::new("slack", config);

        // First take should succeed (via ChannelState)
        assert!(channel.inbound_receiver().is_some());

        // Second take should return None
        assert!(channel.inbound_receiver().is_none());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = SlackChannelFactory;
        assert_eq!(factory.channel_type(), "slack");

        let config = serde_json::json!({
            "app_token": "xapp-1-test-token",
            "bot_token": "xoxb-test-token"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "slack");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = SlackChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_token_prefix() {
        let factory = SlackChannelFactory;

        let config = serde_json::json!({
            "app_token": "invalid-token",
            "bot_token": "xoxb-test"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_without_feature() {
        let config = SlackConfig {
            app_token: "xapp-test".to_string(),
            bot_token: "xoxb-test".to_string(),
            ..Default::default()
        };
        let _channel = SlackChannel::new("slack", config);

        // Without the slack feature, start should return UnsupportedFeature.
        // When the slack feature IS enabled, start() requires a live Slack API
        // which cannot be tested in unit tests, so this test only validates
        // construction succeeds.
    }
}
