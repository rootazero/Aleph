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
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendResult, StreamProtocol,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use tokio::sync::{watch, RwLock};

/// Matrix channel implementation using the Client-Server API v3.
pub struct MatrixChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: MatrixConfig,
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
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
            client: reqwest::Client::new(),
            user_id: Arc::new(RwLock::new(None)),
            since_token: Arc::new(RwLock::new(None)),
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
            editing: true,
            deletion: true,
            typing_indicator: true,
            read_receipts: true,
            rich_text: true,
            max_message_length: 65535,
            max_attachment_size: 100 * 1024 * 1024,
            stream_protocol: StreamProtocol::EditBased,
        }
    }

    /// Update internal status
    async fn set_status(&self, status: ChannelStatus) {
        self.channel_state.set_status(status).await;
    }

    async fn send_with_attachments(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let room_id = message.conversation_id.as_str();
        let reply_to = message.reply_to.as_ref().map(|id| id.as_str().to_string());

        let mut last_result = None;

        if !message.text.is_empty() {
            last_result = Some(
                MatrixMessageOps::send_message(
                    &self.client,
                    &self.config.homeserver_url,
                    &self.config.access_token,
                    room_id,
                    &message.text,
                    reply_to.as_deref(),
                )
                .await?,
            );
        }

        for attachment in &message.attachments {
            let (content, mime_type, filename) = self.prepare_attachment(attachment).await?;

            let mxc_uri = MatrixMessageOps::upload_media(
                &self.client,
                &self.config.homeserver_url,
                &self.config.access_token,
                content,
                &mime_type,
                filename.as_deref(),
            )
            .await?;

            let msgtype = match mime_type.starts_with("image/") {
                true => "m.image",
                false if mime_type.starts_with("audio/") => "m.audio",
                false if mime_type.starts_with("video/") => "m.video",
                _ => "m.file",
            };

            let body = serde_json::json!({
                "msgtype": msgtype,
                "body": filename.as_deref().unwrap_or("attachment"),
                "url": mxc_uri,
            });

            let txn_id = uuid::Uuid::new_v4().to_string();
            let url = format!(
                "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
                self.config.homeserver_url, room_id, txn_id
            );

            let resp = self
                .client
                .put(&url)
                .bearer_auth(&self.config.access_token)
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    ChannelError::SendFailed(format!("Matrix attachment send failed: {e}"))
                })?;

            if !resp.status().is_success() {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                return Err(ChannelError::SendFailed(format!(
                    "Matrix attachment send failed ({status}): {resp_body}"
                )));
            }

            let resp_json: serde_json::Value = resp.json().await.map_err(|e| {
                ChannelError::SendFailed(format!("Matrix attachment response parse failed: {e}"))
            })?;

            let event_id = resp_json["event_id"]
                .as_str()
                .unwrap_or(&txn_id)
                .to_string();

            last_result = Some(SendResult {
                message_id: MessageId::new(event_id),
                timestamp: chrono::Utc::now(),
            });
        }

        last_result.ok_or_else(|| {
            ChannelError::SendFailed("No message or attachments to send".to_string())
        })
    }

    async fn prepare_attachment(
        &self,
        attachment: &crate::gateway::channel::Attachment,
    ) -> ChannelResult<(Vec<u8>, String, Option<String>)> {
        let filename = attachment
            .filename
            .as_deref()
            .or_else(|| attachment.id.as_str().split('/').next_back())
            .map(String::from);

        if let Some(data) = &attachment.data {
            return Ok((data.clone(), attachment.mime_type.clone(), filename));
        }

        if let Some(url) = &attachment.url {
            if url.starts_with("http://") || url.starts_with("https://") {
                let resp = self.client.get(url).send().await.map_err(|e| {
                    ChannelError::ReceiveFailed(format!("Attachment download failed: {e}"))
                })?;

                if !resp.status().is_success() {
                    return Err(ChannelError::ReceiveFailed(format!(
                        "Attachment download failed: {}",
                        resp.status()
                    )));
                }

                let content_type = resp
                    .headers()
                    .get("Content-Type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(&attachment.mime_type)
                    .to_string();

                let bytes = resp.bytes().await.map_err(|e| {
                    ChannelError::ReceiveFailed(format!(
                        "Attachment download body read failed: {e}"
                    ))
                })?;

                return Ok((bytes.to_vec(), content_type, filename));
            }

            if url.starts_with("mxc://") {
                let (content, _) = MatrixMessageOps::download_media(
                    &self.client,
                    &self.config.homeserver_url,
                    url,
                )
                .await?;
                return Ok((content, attachment.mime_type.clone(), filename));
            }
        }

        if let Some(path) = &attachment.path {
            let content = tokio::fs::read(path).await.map_err(|e| {
                ChannelError::ReceiveFailed(format!("Failed to read attachment file: {e}"))
            })?;
            return Ok((content, attachment.mime_type.clone(), filename));
        }

        Err(ChannelError::ReceiveFailed(
            "Attachment has no content (no data, url, or path)".to_string(),
        ))
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
        // Validate configuration
        self.config.validate().map_err(ChannelError::ConfigError)?;

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
        let inbound_tx = self.channel_state.sender();
        let status = self.channel_state.status_handle();

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

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Matrix channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
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
        let room_id = message.conversation_id.as_str();

        if !message.attachments.is_empty() {
            return self.send_with_attachments(message).await;
        }

        MatrixMessageOps::send_message(
            &self.client,
            &self.config.homeserver_url,
            &self.config.access_token,
            room_id,
            &message.text,
            reply_to.as_deref(),
        )
        .await
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
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

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        MatrixMessageOps::edit_message(
            &self.client,
            &self.config.homeserver_url,
            &self.config.access_token,
            conversation_id.as_str(),
            message_id.as_str(),
            new_text,
        )
        .await?;
        Ok(())
    }

    async fn react(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        MatrixMessageOps::send_reaction(
            &self.client,
            &self.config.homeserver_url,
            &self.config.access_token,
            conversation_id.as_str(),
            message_id.as_str(),
            reaction,
        )
        .await?;
        Ok(())
    }

    async fn delete(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        MatrixMessageOps::delete_message(
            &self.client,
            &self.config.homeserver_url,
            &self.config.access_token,
            conversation_id.as_str(),
            message_id.as_str(),
        )
        .await?;
        Ok(())
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
        let channel = MatrixChannel::new("matrix", config);

        // Broadcast semantics: every call subscribes a fresh receiver.
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
