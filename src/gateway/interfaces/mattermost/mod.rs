//! Mattermost Channel Implementation
//!
//! Integrates with Mattermost using the WebSocket API v4 for receiving events
//! and the REST API v4 for sending messages. No external Mattermost SDK required.
//!
//! # Protocol
//!
//! - **WebSocket**: Connects to `wss://{server}/api/v4/websocket` for real-time events.
//!   Authentication via `authentication_challenge` action with bot token.
//! - **REST API v4**: Uses bot token in `Authorization: Bearer` header for sending
//!   messages via `POST /api/v4/posts` and other API methods.
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "mattermost"
//! channel_type = "mattermost"
//! enabled = true
//!
//! [channels.config]
//! server_url = "https://mattermost.example.com"
//! bot_token = "your-bot-token"
//! allowed_channels = ["channel-id-1"]
//! ```

pub mod config;
pub mod message_ops;

pub use config::MattermostConfig;
pub use message_ops::MattermostMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendResult,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tokio::sync::{watch, RwLock};

/// Mattermost channel implementation using WebSocket + REST API v4.
pub struct MattermostChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: MattermostConfig,
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Bot's own user ID (populated after /api/v4/users/me)
    bot_user_id: Arc<RwLock<Option<String>>>,
    /// HTTP client for Mattermost API calls
    client: reqwest::Client,
    /// Optional custom API base URL for testing (e.g. mock server)
    api_base: Option<String>,
}

impl MattermostChannel {
    /// Create a new Mattermost channel
    pub fn new(id: impl Into<String>, config: MattermostConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Mattermost".to_string(),
            channel_type: "mattermost".to_string(),
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
            api_base: None,
        }
    }

    /// Create a Mattermost channel configured for testing against a mock API server.
    pub fn for_test(
        id: impl Into<String>,
        config: MattermostConfig,
        api_base: impl Into<String>,
    ) -> Self {
        let mut channel = Self::new(id, config);
        channel.api_base = Some(api_base.into());
        channel
    }

    /// Get Mattermost-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: true,
            replies: true,
            editing: true,
            deletion: true,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true, // Mattermost supports standard Markdown
            max_message_length: 16383,
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
impl Channel for MattermostChannel {
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
        tracing::info!("Starting Mattermost channel...");

        // Validate bot token via /api/v4/users/me
        let server = self.config.server_url_trimmed().to_string();
        let api_base = self.api_base.as_deref();
        match MattermostMessageOps::get_me_with_base(
            &self.client,
            &server,
            &self.config.bot_token,
            api_base,
        )
        .await
        {
            Ok((user_id, username)) => {
                tracing::info!("Mattermost bot authenticated as {username} (user_id: {user_id})");
                *self.bot_user_id.write().await = Some(user_id);
            }
            Err(e) => {
                self.set_status(ChannelStatus::Error).await;
                return Err(e);
            }
        }

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Spawn WebSocket event loop
        let client = self.client.clone();
        let config = self.config.clone();
        let bot_user_id = self.bot_user_id.clone();
        let channel_id = self.info.id.clone();
        let inbound_tx = self.channel_state.sender();
        let status = self.channel_state.status_handle();

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;

            let uid = {
                let guard = bot_user_id.read().await;
                guard.as_deref().unwrap_or("").to_string()
            };

            MattermostMessageOps::run_ws_loop(
                client,
                config,
                uid,
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
        tracing::info!("Stopping Mattermost channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        // Extract root_id from reply_to for threading
        let root_id = message.reply_to.as_ref().map(|id| id.as_str().to_string());

        let server = self.config.server_url_trimmed().to_string();
        let api_base = self.api_base.as_deref();

        // Send typing indicator if enabled
        if self.config.send_typing {
            let _ = MattermostMessageOps::send_typing_with_base(
                &self.client,
                &server,
                &self.config.bot_token,
                message.conversation_id.as_str(),
                api_base,
            )
            .await;
        }

        MattermostMessageOps::send_message_with_base(
            &self.client,
            &server,
            &self.config.bot_token,
            message.conversation_id.as_str(),
            &message.text,
            root_id.as_deref(),
            api_base,
        )
        .await
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        if self.config.send_typing {
            let server = self.config.server_url_trimmed().to_string();
            let api_base = self.api_base.as_deref();
            MattermostMessageOps::send_typing_with_base(
                &self.client,
                &server,
                &self.config.bot_token,
                conversation_id.as_str(),
                api_base,
            )
            .await
        } else {
            Ok(())
        }
    }

    async fn edit(
        &self,
        _conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        let server = self.config.server_url_trimmed().to_string();
        let api_base = self.api_base.as_deref();

        MattermostMessageOps::edit_message_with_base(
            &self.client,
            &server,
            &self.config.bot_token,
            message_id.as_str(),
            new_text,
            api_base,
        )
        .await
    }

    async fn delete(
        &self,
        _conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        let server = self.config.server_url_trimmed().to_string();
        let api_base = self.api_base.as_deref();

        MattermostMessageOps::delete_message_with_base(
            &self.client,
            &server,
            &self.config.bot_token,
            message_id.as_str(),
            api_base,
        )
        .await
    }

    async fn react(
        &self,
        _conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        let server = self.config.server_url_trimmed().to_string();
        let api_base = self.api_base.as_deref();

        let user_id = {
            let guard = self.bot_user_id.read().await;
            guard.as_deref().unwrap_or("").to_string()
        };

        MattermostMessageOps::react_with_base(
            &self.client,
            &server,
            &self.config.bot_token,
            &user_id,
            message_id.as_str(),
            reaction,
            api_base,
        )
        .await
    }
}

/// Factory for creating Mattermost channels
pub struct MattermostChannelFactory;

#[async_trait]
impl ChannelFactory for MattermostChannelFactory {
    fn channel_type(&self) -> &str {
        "mattermost"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: MattermostConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Mattermost config: {}", e)))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(MattermostChannel::new("mattermost", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = MattermostChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(caps.audio);
        assert!(caps.video);
        assert!(caps.reactions);
        assert!(caps.replies);
        assert!(caps.editing);
        assert!(caps.deletion);
        assert!(caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(caps.rich_text);
        assert_eq!(caps.max_message_length, 16383);
        assert_eq!(caps.max_attachment_size, 100 * 1024 * 1024);
    }

    #[test]
    fn test_channel_creation() {
        let config = MattermostConfig {
            server_url: "https://mm.example.com".to_string(),
            bot_token: "test-token".to_string(),
            ..Default::default()
        };
        let channel = MattermostChannel::new("mm-test", config);
        assert_eq!(channel.info().id.as_str(), "mm-test");
        assert_eq!(channel.info().channel_type, "mattermost");
        assert_eq!(channel.info().name, "Mattermost");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = MattermostConfig::default();
        let channel = MattermostChannel::new("mattermost", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = MattermostConfig::default();
        let channel = MattermostChannel::new("mattermost", config);

        // Broadcast semantics: every call subscribes a fresh receiver.
        assert!(channel.state().take_receiver().is_some());
        assert!(channel.state().take_receiver().is_some());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = MattermostChannelFactory;
        assert_eq!(factory.channel_type(), "mattermost");

        let config = serde_json::json!({
            "server_url": "https://mm.example.com",
            "bot_token": "test-token-abc123"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "mattermost");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = MattermostChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_server_url() {
        let factory = MattermostChannelFactory;

        let config = serde_json::json!({
            "server_url": "ftp://invalid",
            "bot_token": "test-token"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_without_feature() {
        let config = MattermostConfig {
            server_url: "https://mm.example.com".to_string(),
            bot_token: "test-token".to_string(),
            ..Default::default()
        };
        let _channel = MattermostChannel::new("mattermost", config);

        // Without the mattermost feature, start should return UnsupportedFeature.
        // When the mattermost feature IS enabled, start() requires a live server
        // which cannot be tested in unit tests, so this test only validates
        // construction succeeds.
    }
}
