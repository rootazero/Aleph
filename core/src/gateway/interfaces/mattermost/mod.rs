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
    ChannelResult, ChannelStatus, ConversationId, InboundMessage, MessageId, OutboundMessage,
    SendResult,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};

/// Mattermost channel implementation using WebSocket + REST API v4.
pub struct MattermostChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: MattermostConfig,
    /// Inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken on first call)
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Current status
    status: Arc<RwLock<ChannelStatus>>,
    /// Bot's own user ID (populated after /api/v4/users/me)
    bot_user_id: Arc<RwLock<Option<String>>>,
    /// HTTP client for Mattermost API calls
    client: reqwest::Client,
}

impl MattermostChannel {
    /// Create a new Mattermost channel
    pub fn new(id: impl Into<String>, config: MattermostConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);

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
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            shutdown_tx: None,
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
            bot_user_id: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
        }
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
        }
    }

    /// Take the inbound receiver (can only be called once)
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.inbound_rx.take()
    }

    /// Update internal status
    async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }
}

#[async_trait]
impl Channel for MattermostChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn status(&self) -> ChannelStatus {
        self.info.status
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Validate configuration
        self.config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        #[cfg(feature = "mattermost")]
        {
            self.set_status(ChannelStatus::Connecting).await;
            tracing::info!("Starting Mattermost channel...");

            // Validate bot token via /api/v4/users/me
            let server = self.config.server_url_trimmed().to_string();
            match MattermostMessageOps::get_me(&self.client, &server, &self.config.bot_token).await
            {
                Ok((user_id, username)) => {
                    tracing::info!(
                        "Mattermost bot authenticated as {username} (user_id: {user_id})"
                    );
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
            let inbound_tx = self.inbound_tx.clone();
            let status = self.status.clone();

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

        #[cfg(not(feature = "mattermost"))]
        {
            Err(ChannelError::UnsupportedFeature(
                "Mattermost support not compiled (enable 'mattermost' feature)".to_string(),
            ))
        }
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
        #[cfg(feature = "mattermost")]
        {
            // Extract root_id from reply_to for threading
            let root_id = message.reply_to.as_ref().map(|id| id.as_str().to_string());

            let server = self.config.server_url_trimmed().to_string();

            // Send typing indicator if enabled
            if self.config.send_typing {
                let _ = MattermostMessageOps::send_typing(
                    &self.client,
                    &server,
                    &self.config.bot_token,
                    message.conversation_id.as_str(),
                )
                .await;
            }

            MattermostMessageOps::send_message(
                &self.client,
                &server,
                &self.config.bot_token,
                message.conversation_id.as_str(),
                &message.text,
                root_id.as_deref(),
            )
            .await
        }

        #[cfg(not(feature = "mattermost"))]
        {
            let _ = message;
            Err(ChannelError::UnsupportedFeature(
                "Mattermost support not compiled".to_string(),
            ))
        }
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Already taken during construction or via take_receiver
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        #[cfg(feature = "mattermost")]
        {
            if self.config.send_typing {
                let server = self.config.server_url_trimmed().to_string();
                MattermostMessageOps::send_typing(
                    &self.client,
                    &server,
                    &self.config.bot_token,
                    conversation_id.as_str(),
                )
                .await
            } else {
                Ok(())
            }
        }

        #[cfg(not(feature = "mattermost"))]
        {
            let _ = conversation_id;
            Err(ChannelError::UnsupportedFeature(
                "Mattermost support not compiled".to_string(),
            ))
        }
    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        #[cfg(feature = "mattermost")]
        {
            // Mattermost edit requires PUT /api/v4/posts/{post_id}
            let server = self.config.server_url_trimmed().to_string();
            let url = format!("{server}/api/v4/posts/{}", message_id.as_str());

            let body = serde_json::json!({
                "id": message_id.as_str(),
                "message": new_text,
            });

            let resp = self
                .client
                .put(&url)
                .bearer_auth(&self.config.bot_token)
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("edit request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                return Err(ChannelError::SendFailed(format!(
                    "Mattermost edit failed {status}: {resp_body}"
                )));
            }

            Ok(())
        }

        #[cfg(not(feature = "mattermost"))]
        {
            let _ = (message_id, new_text);
            Err(ChannelError::UnsupportedFeature(
                "Mattermost support not compiled".to_string(),
            ))
        }
    }

    async fn delete(&self, message_id: &MessageId) -> ChannelResult<()> {
        #[cfg(feature = "mattermost")]
        {
            // Mattermost delete: DELETE /api/v4/posts/{post_id}
            let server = self.config.server_url_trimmed().to_string();
            let url = format!("{server}/api/v4/posts/{}", message_id.as_str());

            let resp = self
                .client
                .delete(&url)
                .bearer_auth(&self.config.bot_token)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("delete request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                return Err(ChannelError::SendFailed(format!(
                    "Mattermost delete failed {status}: {resp_body}"
                )));
            }

            Ok(())
        }

        #[cfg(not(feature = "mattermost"))]
        {
            let _ = message_id;
            Err(ChannelError::UnsupportedFeature(
                "Mattermost support not compiled".to_string(),
            ))
        }
    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        #[cfg(feature = "mattermost")]
        {
            // Mattermost reaction: POST /api/v4/reactions
            let server = self.config.server_url_trimmed().to_string();
            let url = format!("{server}/api/v4/reactions");

            let user_id = {
                let guard = self.bot_user_id.read().await;
                guard.as_deref().unwrap_or("").to_string()
            };

            let body = serde_json::json!({
                "user_id": user_id,
                "post_id": message_id.as_str(),
                "emoji_name": reaction,
            });

            let resp = self
                .client
                .post(&url)
                .bearer_auth(&self.config.bot_token)
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("react request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                return Err(ChannelError::SendFailed(format!(
                    "Mattermost reaction failed {status}: {resp_body}"
                )));
            }

            Ok(())
        }

        #[cfg(not(feature = "mattermost"))]
        {
            let _ = (message_id, reaction);
            Err(ChannelError::UnsupportedFeature(
                "Mattermost support not compiled".to_string(),
            ))
        }
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

        config
            .validate()
            .map_err(ChannelError::ConfigError)?;

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
        let mut channel = MattermostChannel::new("mattermost", config);

        // First take should succeed
        assert!(channel.take_receiver().is_some());

        // Second take should return None
        assert!(channel.take_receiver().is_none());
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
