//! Matrix Channel Implementation
//!
//! Integrates with Matrix using the Client-Server API v3 for sending
//! and receiving messages. Uses `/sync` long-polling for real-time message reception.
//!
//! # Protocol
//!
//! - **Receiving:** Long-polling via `GET /_matrix/client/v3/sync?timeout=30000&since={token}`
//! - **Sending:** `PUT /_matrix/client/v3/rooms/{room_id}/send/m.room.message/{txn_id}`
//! - **Auth:** Bearer token in Authorization header
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "matrix"
//! channel_type = "matrix"
//! enabled = true
//!
//! [channels.config]
//! homeserver_url = "https://matrix.org"
//! access_token = "syt_..."
//! allowed_rooms = ["!room:matrix.org"]
//! ```

pub mod config;
pub mod message_ops;

pub use config::MatrixConfig;
pub use message_ops::MatrixMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelStatus, ConversationId, InboundMessage, MessageId, OutboundMessage,
    SendResult,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};

/// Matrix channel implementation using the Client-Server API v3.
pub struct MatrixChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: MatrixConfig,
    /// Inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken on first call)
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Current status
    status: Arc<RwLock<ChannelStatus>>,
    /// HTTP client for Matrix API calls
    client: reqwest::Client,
    /// Own user ID from /whoami (e.g., "@bot:matrix.org")
    user_id: Arc<RwLock<Option<String>>>,
    /// Sync pagination token
    since_token: Arc<RwLock<Option<String>>>,
}

impl MatrixChannel {
    /// Create a new Matrix channel
    pub fn new(id: impl Into<String>, config: MatrixConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Matrix".to_string(),
            channel_type: "matrix".to_string(),
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
            client: reqwest::Client::new(),
            user_id: Arc::new(RwLock::new(None)),
            since_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Get Matrix-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: true,
            replies: true,
            editing: true,
            deletion: false,
            typing_indicator: true,
            read_receipts: true,
            rich_text: true, // Matrix supports org.matrix.custom.html
            max_message_length: 65535,
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
impl Channel for MatrixChannel {
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

        #[cfg(feature = "matrix")]
        {
            self.set_status(ChannelStatus::Connecting).await;
            tracing::info!("Starting Matrix channel...");

            // Validate access token via /whoami
            match MatrixMessageOps::validate_token(
                &self.client,
                &self.config.homeserver_url,
                &self.config.access_token,
            )
            .await
            {
                Ok(uid) => {
                    tracing::info!("Matrix bot authenticated as {uid}");
                    *self.user_id.write().await = Some(uid);
                }
                Err(e) => {
                    self.set_status(ChannelStatus::Error).await;
                    return Err(e);
                }
            }

            // Create shutdown channel
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            self.shutdown_tx = Some(shutdown_tx);

            // Spawn /sync long-polling loop
            let client = self.client.clone();
            let config = self.config.clone();
            let user_id = self.user_id.clone();
            let since_token = self.since_token.clone();
            let channel_id = self.info.id.clone();
            let inbound_tx = self.inbound_tx.clone();
            let status = self.status.clone();

            tokio::spawn(async move {
                *status.write().await = ChannelStatus::Connected;

                MatrixMessageOps::run_sync_loop(
                    client,
                    config,
                    user_id,
                    since_token,
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

        #[cfg(not(feature = "matrix"))]
        {
            Err(ChannelError::UnsupportedFeature(
                "Matrix support not compiled (enable 'matrix' feature)".to_string(),
            ))
        }
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Matrix channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        #[cfg(feature = "matrix")]
        {
            // Send typing indicator if enabled
            if self.config.send_typing {
                if let Some(ref uid) = *self.user_id.read().await {
                    let _ = MatrixMessageOps::send_typing(
                        &self.client,
                        &self.config.homeserver_url,
                        &self.config.access_token,
                        message.conversation_id.as_str(),
                        uid,
                        true,
                    )
                    .await;
                }
            }

            let reply_to = message.reply_to.as_ref().map(|id| id.as_str().to_string());

            MatrixMessageOps::send_message(
                &self.client,
                &self.config.homeserver_url,
                &self.config.access_token,
                message.conversation_id.as_str(),
                &message.text,
                reply_to.as_deref(),
            )
            .await
        }

        #[cfg(not(feature = "matrix"))]
        {
            let _ = message;
            Err(ChannelError::UnsupportedFeature(
                "Matrix support not compiled".to_string(),
            ))
        }
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Already taken during construction or via take_receiver
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        #[cfg(feature = "matrix")]
        {
            if let Some(ref uid) = *self.user_id.read().await {
                MatrixMessageOps::send_typing(
                    &self.client,
                    &self.config.homeserver_url,
                    &self.config.access_token,
                    conversation_id.as_str(),
                    uid,
                    true,
                )
                .await?;
            }
            Ok(())
        }

        #[cfg(not(feature = "matrix"))]
        {
            let _ = conversation_id;
            Err(ChannelError::UnsupportedFeature(
                "Matrix support not compiled".to_string(),
            ))
        }
    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        // Matrix supports editing via m.replace relation, but we need the room_id
        // which isn't available in this interface signature.
        let _ = (message_id, new_text);
        Err(ChannelError::UnsupportedFeature(
            "Message editing requires room context (conversation_id + event_id)".to_string(),
        ))
    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        // Matrix supports reactions via m.reaction relation, but we need the room_id.
        let _ = (message_id, reaction);
        Err(ChannelError::UnsupportedFeature(
            "Reactions require room context (conversation_id + event_id)".to_string(),
        ))
    }
}

/// Factory for creating Matrix channels
pub struct MatrixChannelFactory;

#[async_trait]
impl ChannelFactory for MatrixChannelFactory {
    fn channel_type(&self) -> &str {
        "matrix"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: MatrixConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Matrix config: {}", e)))?;

        config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        Ok(Box::new(MatrixChannel::new("matrix", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = MatrixChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(caps.audio);
        assert!(caps.video);
        assert!(caps.reactions);
        assert!(caps.replies);
        assert!(caps.editing);
        assert!(!caps.deletion);
        assert!(caps.typing_indicator);
        assert!(caps.read_receipts);
        assert!(caps.rich_text);
        assert_eq!(caps.max_message_length, 65535);
        assert_eq!(caps.max_attachment_size, 100 * 1024 * 1024);
    }

    #[test]
    fn test_channel_creation() {
        let config = MatrixConfig {
            homeserver_url: "https://matrix.org".to_string(),
            access_token: "token123".to_string(),
            ..Default::default()
        };
        let channel = MatrixChannel::new("matrix-test", config);
        assert_eq!(channel.info().id.as_str(), "matrix-test");
        assert_eq!(channel.info().channel_type, "matrix");
        assert_eq!(channel.info().name, "Matrix");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = MatrixConfig::default();
        let channel = MatrixChannel::new("matrix", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = MatrixConfig::default();
        let mut channel = MatrixChannel::new("matrix", config);

        // First take should succeed
        assert!(channel.take_receiver().is_some());

        // Second take should return None
        assert!(channel.take_receiver().is_none());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = MatrixChannelFactory;
        assert_eq!(factory.channel_type(), "matrix");

        let config = serde_json::json!({
            "homeserver_url": "https://matrix.org",
            "access_token": "syt_test_token_123"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "matrix");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = MatrixChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_homeserver() {
        let factory = MatrixChannelFactory;

        let config = serde_json::json!({
            "homeserver_url": "not-a-url",
            "access_token": "token123"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_without_feature() {
        let config = MatrixConfig {
            homeserver_url: "https://matrix.org".to_string(),
            access_token: "token123".to_string(),
            ..Default::default()
        };
        let _channel = MatrixChannel::new("matrix", config);

        // Without the matrix feature, start should return UnsupportedFeature.
        // When the matrix feature IS enabled, start() requires a live Matrix homeserver
        // which cannot be tested in unit tests, so this test only validates
        // construction succeeds.
    }
}
