//! Matrix Channel Implementation
//!
//! Integrates with Matrix using the official `matrix-sdk` Rust crate.
//! Supports sync token persistence, event deduplication, and extended event coverage.

pub mod client;
pub mod config;
pub mod dedupe;
pub mod events;
pub mod media;
pub mod outbound;
pub mod sync;

pub use config::MatrixConfig;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendResult, StreamProtocol,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tokio::sync::watch;

/// Matrix channel implementation using the official Rust SDK.
pub struct MatrixChannel {
    info: ChannelInfo,
    config: MatrixConfig,
    channel_state: ChannelState,
    shutdown_tx: Option<watch::Sender<bool>>,
    client: Option<Arc<client::MatrixSdkClient>>,
    deduper: Option<Arc<dedupe::EventDeduper>>,
    /// Test mode: skip real SDK connection, return mock results
    test_mode: bool,
}

impl MatrixChannel {
    pub fn new(id: impl Into<String>, config: MatrixConfig) -> Self {
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
            channel_state: ChannelState::new(100),
            shutdown_tx: None,
            client: None,
            deduper: None,
            test_mode: false,
        }
    }

    pub fn for_test(id: impl Into<String>, config: MatrixConfig) -> Self {
        let mut channel = Self::new(id, config);
        channel.test_mode = true;
        channel
    }

    const fn capabilities() -> ChannelCapabilities {
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
            // Matrix has `m.read` receipts, but no `mark_read` override sends
            // one. Declared honestly until it does.
            read_receipts: false,
            rich_text: true,
            max_message_length: 65535,
            max_attachment_size: 100 * 1024 * 1024,
            stream_protocol: StreamProtocol::EditBased,
        }
    }

    async fn set_status(&self, status: ChannelStatus) {
        self.channel_state.set_status(status).await;
    }

    fn client(&self) -> ChannelResult<&Arc<client::MatrixSdkClient>> {
        self.client
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("Matrix client not initialized".to_string()))
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.config.validate().map_err(ChannelError::ConfigError)?;
        self.set_status(ChannelStatus::Connecting).await;
        tracing::info!("Starting Matrix channel...");

        if self.test_mode {
            self.set_status(ChannelStatus::Connected).await;
            tracing::info!("Matrix channel started in test mode");
            return Ok(());
        }

        let sdk_client = Arc::new(client::MatrixSdkClient::connect(&self.config).await?);
        tracing::info!("Matrix bot authenticated as {}", sdk_client.user_id());

        let deduper = Arc::new(dedupe::EventDeduper::new(&self.config.dedupe_store_path())?);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        self.client = Some(sdk_client.clone());
        self.deduper = Some(deduper.clone());

        let config = self.config.clone();
        let channel_id = self.info.id.clone();
        let inbound_tx = self.channel_state.sender();
        let status = self.channel_state.status_handle();

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;
            sync::run_sync_loop(
                sdk_client,
                config,
                channel_id,
                inbound_tx,
                status.clone(),
                deduper,
                shutdown_rx,
            )
            .await;
        });

        self.set_status(ChannelStatus::Connected).await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Matrix channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.client = None;
        self.deduper = None;
        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Matrix channel not started".to_string(),
                ));
            }
            return Ok(SendResult {
                message_id: MessageId::new("matrix-test-msg-id".to_string()),
                timestamp: chrono::Utc::now(),
            });
        }
        let client = self.client()?;
        outbound::send_message(client.inner(), message).await
    }

    async fn send_typing(&self, _conversation_id: &ConversationId) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Matrix channel not started".to_string(),
                ));
            }
            return Ok(());
        }
        let client = self.client()?;
        if self.config.send_typing {
            if let Some(ref c) = self.client {
                outbound::send_typing(client.inner(), _conversation_id, c.user_id(), true).await?;
            }
        }
        Ok(())
    }

    async fn edit(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _new_text: &str,
    ) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Matrix channel not started".to_string(),
                ));
            }
            return Ok(());
        }
        let client = self.client()?;
        outbound::edit_message(client.inner(), _conversation_id, _message_id, _new_text).await?;
        Ok(())
    }

    async fn react(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _reaction: &str,
    ) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Matrix channel not started".to_string(),
                ));
            }
            return Ok(());
        }
        let client = self.client()?;
        outbound::send_reaction(client.inner(), _conversation_id, _message_id, _reaction).await?;
        Ok(())
    }

    async fn delete(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
    ) -> ChannelResult<()> {
        if self.test_mode {
            if self.status() != ChannelStatus::Connected {
                return Err(ChannelError::NotConnected(
                    "Matrix channel not started".to_string(),
                ));
            }
            return Ok(());
        }
        let client = self.client()?;
        outbound::delete_message(client.inner(), _conversation_id, _message_id).await?;
        Ok(())
    }
}

pub struct MatrixChannelFactory;

#[async_trait]
impl ChannelFactory for MatrixChannelFactory {
    fn channel_type(&self) -> &str {
        "matrix"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: MatrixConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Matrix config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

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
        assert!(caps.deletion);
        assert!(caps.typing_indicator);
        // A capability flag promises the matching `Channel` method works. No
        // `mark_read` override exists here, so `true` would be a promise the
        // trait default cannot keep. Flip this back only together with the impl.
        assert!(!caps.read_receipts);
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
        let channel = MatrixChannel::new("matrix", config);
        assert!(channel.state().take_receiver().is_some());
        assert!(channel.state().take_receiver().is_some());
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
}
