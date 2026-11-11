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
pub mod directory;
pub(crate) mod errors;
pub mod message_ops;

pub use config::SlackConfig;
pub use directory::UserDirectory;
pub use message_ops::SlackMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendOperation, SendResult,
};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::{watch, RwLock};

/// Outbound attachment download timeout — keeps a slow URL from stalling a send.
const ATTACHMENT_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// Optional custom API base URL for testing (e.g. mock server)
    api_base: Option<String>,
    /// Workspace roster (`name → id`) backing `list_conversations`.
    ///
    /// Built on first use rather than in `new`, because `for_test` sets
    /// `api_base` *after* construction — eagerly capturing it here would pin
    /// every test's directory to the real slack.com.
    directory: tokio::sync::OnceCell<directory::ConversationDirectory>,
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
            api_base: None,
            directory: tokio::sync::OnceCell::new(),
        }
    }

    /// Create a Slack channel configured for testing against a mock API server.
    pub fn for_test(
        id: impl Into<String>,
        config: SlackConfig,
        api_base: impl Into<String>,
    ) -> Self {
        let mut channel = Self::new(id, config);
        channel.api_base = Some(api_base.into());
        channel
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
            rich_text: true,
            polls: false,
            group_icons: false, // Slack mrkdwn support
            max_message_length: 3000,
            max_attachment_size: 1_073_741_824, // 1GB
            stream_protocol: Default::default(),
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
        self.config.validate().map_err(ChannelError::ConfigError)?;

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;
        tracing::info!("Starting Slack channel...");

        // Validate bot token via auth.test
        let api_base = self.api_base.as_deref();
        match SlackMessageOps::validate_bot_token(&self.client, &self.config.bot_token, api_base)
            .await
        {
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

        let user_directory = if self.config.resolve_user_names {
            Some(Arc::new(UserDirectory::new(
                self.config.bot_token.clone(),
                self.config.directory_ttl_secs,
            )))
        } else {
            None
        };

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
                user_directory,
            )
            .await;

            *status.write().await = ChannelStatus::Disconnected;
        });

        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Slack channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    fn send(&self, message: OutboundMessage) -> SendOperation {
        let client = self.client.clone();
        let bot_token = self.config.bot_token.clone();
        let api_base = self.api_base.clone();
        let send_typing = self.config.send_typing;
        // The same media budget the inbound path uses: an outbound URL
        // download is bounded by it, so a hostile URL cannot ask the limb to
        // buffer an arbitrary body.
        let media_max_bytes = self.config.media_max_mb.saturating_mul(1024 * 1024);
        Box::pin(async move {
            let conversation_id = message.conversation_id.clone();
            let thread_ts = message.reply_to.as_ref().map(|id| id.as_str().to_string());

            // Send typing indicator if enabled (non-blocking, best-effort)
            if send_typing {
                let typing_client = client.clone();
                let token = bot_token.clone();
                let channel = conversation_id.as_str().to_string();
                let typing_api_base = api_base.clone();
                tokio::spawn(async move {
                    let _ = SlackMessageOps::post_typing_with_base(
                        &typing_client,
                        &token,
                        &channel,
                        typing_api_base.as_deref(),
                    )
                    .await;
                });
            }

            // Handle file attachments
            if !message.attachments.is_empty() {
                return Self::send_with_attachments(
                    &client,
                    &bot_token,
                    api_base.as_deref(),
                    message,
                    thread_ts.as_deref(),
                    media_max_bytes,
                )
                .await;
            }

            SlackMessageOps::send_message_with_base(
                &client,
                &bot_token,
                conversation_id.as_str(),
                &message.text,
                thread_ts.as_deref(),
                api_base.as_deref(),
            )
            .await
        })
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        SlackMessageOps::post_typing_with_base(
            &self.client,
            &self.config.bot_token,
            conversation_id.as_str(),
            self.api_base.as_deref(),
        )
        .await
    }

    async fn react(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        if !self.config.enable_reactions {
            return Err(ChannelError::UnsupportedFeature(
                "reactions are disabled".to_string(),
            ));
        }
        SlackMessageOps::add_reaction_with_base(
            &self.client,
            &self.config.bot_token,
            conversation_id.as_str(),
            message_id.as_str(),
            reaction,
            self.api_base.as_deref(),
        )
        .await
    }

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        if !self.config.enable_editing {
            return Err(ChannelError::UnsupportedFeature(
                "message editing is disabled".to_string(),
            ));
        }
        SlackMessageOps::update_message(
            &self.client,
            &self.config.bot_token,
            conversation_id.as_str(),
            message_id.as_str(),
            new_text,
        )
        .await
    }

    async fn delete(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        if !self.config.enable_deletion {
            return Err(ChannelError::UnsupportedFeature(
                "message deletion is disabled".to_string(),
            ));
        }
        SlackMessageOps::delete_message(
            &self.client,
            &self.config.bot_token,
            conversation_id.as_str(),
            message_id.as_str(),
        )
        .await
    }

    async fn list_conversations(
        &self,
        query: &str,
        limit: usize,
    ) -> ChannelResult<crate::gateway::channel::ConversationPage> {
        self.directory
            .get_or_init(|| async {
                directory::ConversationDirectory::new(
                    self.config.bot_token.clone(),
                    self.api_base.clone(),
                )
            })
            .await
            .list(query, limit)
            .await
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
        self.create_with_id("slack", config).await
    }

    async fn create_with_id(
        &self,
        id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<Box<dyn Channel>> {
        let config: SlackConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Slack config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(SlackChannel::new(id, config)))
    }
}

// SlackChannel helper methods for file uploads
impl SlackChannel {
    /// Send a message with file attachments.
    async fn send_with_attachments(
        client: &reqwest::Client,
        bot_token: &str,
        api_base: Option<&str>,
        message: OutboundMessage,
        thread_ts: Option<&str>,
        media_max_bytes: u64,
    ) -> ChannelResult<SendResult> {
        let conversation_id = message.conversation_id.as_str();

        let mut last_result = None;
        let mut caption_sent = false;

        for (i, attachment) in message.attachments.iter().enumerate() {
            let caption = if i == 0 && !message.text.trim().is_empty() {
                Some(message.text.as_str())
            } else {
                None
            };

            // Get file data based on attachment source
            let upload_result = if let Some(data) = &attachment.data {
                let filename = attachment.filename.as_deref().unwrap_or("file");
                SlackMessageOps::upload_file_with_base(
                    client,
                    bot_token,
                    conversation_id,
                    data.as_slice(),
                    filename,
                    None,
                    Some(attachment.mime_type.as_str()),
                    caption,
                    thread_ts,
                    api_base,
                )
                .await
            } else if let Some(path) = &attachment.path {
                let file_data = tokio::fs::read(path).await.map_err(|e| {
                    ChannelError::SendFailed(format!("Failed to read file {path}: {e}"))
                })?;
                let filename = attachment.filename.as_deref().unwrap_or(path);
                SlackMessageOps::upload_file_with_base(
                    client,
                    bot_token,
                    conversation_id,
                    file_data.as_slice(),
                    filename,
                    None,
                    Some(attachment.mime_type.as_str()),
                    caption,
                    thread_ts,
                    api_base,
                )
                .await
            } else if let Some(url) = &attachment.url {
                let file_data = match Self::download_url(client, url, media_max_bytes).await? {
                    Some(data) => data,
                    None => {
                        // Over the media budget: drop this attachment, keep the
                        // rest of the message. One oversized file must not
                        // fail the whole send.
                        tracing::warn!(
                            "Skipping attachment {}: download exceeds the media cap ({} bytes)",
                            attachment.id,
                            media_max_bytes
                        );
                        continue;
                    }
                };
                let filename = attachment.filename.as_deref().unwrap_or("file");
                SlackMessageOps::upload_file_with_base(
                    client,
                    bot_token,
                    conversation_id,
                    file_data.as_slice(),
                    filename,
                    None,
                    Some(attachment.mime_type.as_str()),
                    caption,
                    thread_ts,
                    api_base,
                )
                .await
            } else {
                tracing::warn!(
                    "Attachment {} has no data, path, or URL - skipping",
                    attachment.id
                );
                continue;
            };

            match upload_result {
                Ok(file_id) => {
                    // The caption rode along with this upload — the text is
                    // already delivered, so it must not be sent again.
                    if caption.is_some() {
                        caption_sent = true;
                    }
                    last_result = Some(SendResult {
                        message_id: MessageId::new(file_id),
                        timestamp: chrono::Utc::now(),
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to upload attachment {}: {}", attachment.id, e);
                    if last_result.is_some() {
                        // Some attachments are already delivered — a retry
                        // would duplicate them. `SendFailed` is the
                        // ambiguous, never-retried verdict.
                        return Err(ChannelError::SendFailed(format!(
                            "Partial success: some attachments delivered, but subsequent upload failed: {}",
                            e
                        )));
                    }
                    return Err(e);
                }
            }
        }

        // The body goes out with the first attachment; sending it again
        // here would repeat it. When nothing was delivered (the only
        // attachment was dropped), the body still has to go out.
        if !caption_sent
            && !message.text.trim().is_empty()
            && (message.attachments.len() > 1 || last_result.is_none())
        {
            let formatted = crate::gateway::formatter::MessageFormatter::format(
                &message.text,
                crate::gateway::formatter::MarkupFormat::SlackMrkdwn,
            );
            let result = SlackMessageOps::send_message(
                client,
                bot_token,
                conversation_id,
                &formatted,
                thread_ts,
            )
            .await?;
            last_result = Some(result);
        }

        last_result.ok_or_else(|| ChannelError::SendFailed("No attachments to send".to_string()))
    }

    /// Download an outbound attachment URL under the media budget.
    ///
    /// `Ok(None)` means the body exceeded `max_bytes`: the caller drops that
    /// attachment instead of failing the whole message. `Err` is a transport
    /// failure and keeps the previous behaviour of failing the send.
    async fn download_url(
        client: &reqwest::Client,
        url: &str,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, ChannelError> {
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to download {url}: {e}")))?;

        if !response.status().is_success() {
            return Err(ChannelError::SendFailed(format!(
                "Download failed with status: {}",
                response.status()
            )));
        }

        // Bounded read: an over-budget Content-Length is refused before a
        // chunk is read, and the body is cut off before it crosses the cap.
        // `response.bytes()` would buffer the whole body first.
        let body = tokio::time::timeout(
            ATTACHMENT_DOWNLOAD_TIMEOUT,
            crate::utils::reqwest_limit::bytes_with_limit(
                response,
                usize::try_from(max_bytes).unwrap_or(usize::MAX),
            ),
        )
        .await
        .map_err(|_| {
            ChannelError::SendFailed(format!(
                "Download timed out after {ATTACHMENT_DOWNLOAD_TIMEOUT:?}"
            ))
        })?;

        match body {
            Ok(Some(bytes)) => Ok(Some(bytes)),
            Ok(None) => Ok(None),
            Err(e) => Err(ChannelError::SendFailed(format!(
                "Failed to read download body: {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

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
    fn test_inbound_subscribe() {
        let config = SlackConfig::default();
        let channel = SlackChannel::new("slack", config);

        // inbound_subscribe returns a broadcast receiver (multiple subscribers allowed)
        let mut rx1 = channel.inbound_subscribe();
        let mut rx2 = channel.inbound_subscribe();
        // Both receivers should be valid (broadcast allows multiple subscribers)
        assert!(rx1.recv().now_or_never().is_none()); // no message sent yet
        assert!(rx2.recv().now_or_never().is_none());
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

    /// An outbound attachment URL is bounded by the media budget.
    ///
    /// The body is refused instead of being buffered whole, and the refusal
    /// is a value (not an error) so the caller can drop that one attachment
    /// and still deliver the rest of the message.
    #[tokio::test]
    async fn download_url_refuses_a_body_over_the_media_budget() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 4096]))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let out = SlackChannel::download_url(&client, &server.uri(), 1024)
            .await
            .expect("download must not be a transport error");
        assert!(
            out.is_none(),
            "a body over the media budget must be refused, not buffered"
        );
    }

    /// The other side of the same seam: a body under the budget comes back
    /// whole.
    #[tokio::test]
    async fn download_url_returns_a_body_under_the_media_budget() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::any())
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let out = SlackChannel::download_url(&client, &server.uri(), 1024)
            .await
            .expect("download must not be a transport error");
        assert_eq!(out.as_deref(), Some(&b"hello"[..]));
    }
}
