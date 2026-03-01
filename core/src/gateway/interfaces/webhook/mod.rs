//! Generic Webhook Channel Implementation
//!
//! Provides a bidirectional HTTP webhook integration. Any system that can POST
//! JSON and receive POST JSON can integrate with Aleph through this channel.
//!
//! # Protocol
//!
//! - **Inbound**: Implements `WebhookHandler` trait from `webhook_receiver.rs`.
//!   Receives HTTP POST with `X-Webhook-Signature` HMAC-SHA256 header.
//! - **Outbound**: POSTs JSON to a configured `callback_url` with the same
//!   `X-Webhook-Signature` header.
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "webhook"
//! channel_type = "webhook"
//! enabled = true
//!
//! [channels.config]
//! secret = "my-hmac-secret"
//! callback_url = "https://my-service.com/aleph/callback"
//! path = "/webhook/generic"
//! allowed_senders = []
//! ```

pub mod config;
pub mod message_ops;

pub use config::WebhookChannelConfig;
pub use message_ops::WebhookMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelStatus, InboundMessage, OutboundMessage, SendResult,
};
use crate::gateway::webhook_receiver::{WebhookHandler, WebhookReceiver};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use crate::sync_primitives::Arc;
use tokio::sync::{mpsc, RwLock};

/// Generic webhook channel implementation.
///
/// Uses HTTP POST for both inbound (via `WebhookHandler` trait) and outbound
/// (via `reqwest` POST to `callback_url`) message delivery.
pub struct WebhookChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: WebhookChannelConfig,
    /// Inbound message sender (used by the WebhookHandler)
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken on first call)
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Current status
    status: Arc<RwLock<ChannelStatus>>,
    /// HTTP client for outbound requests
    client: reqwest::Client,
    /// The webhook handler (created on start, shared with WebhookReceiver)
    handler: Option<Arc<GenericWebhookHandler>>,
}

impl WebhookChannel {
    /// Create a new generic webhook channel
    pub fn new(id: impl Into<String>, config: WebhookChannelConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Webhook".to_string(),
            channel_type: "webhook".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::channel_capabilities(),
        };

        Self {
            info,
            config,
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            status: Arc::new(RwLock::new(ChannelStatus::Disconnected)),
            client: reqwest::Client::new(),
            handler: None,
        }
    }

    /// Get webhook-specific capabilities.
    ///
    /// The generic webhook channel is text-only: no attachments, no reactions,
    /// no editing, no typing indicators. Rich text (JSON) is supported, and
    /// the max message length is 1MB.
    fn channel_capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: false,
            audio: false,
            video: false,
            reactions: false,
            replies: false,
            editing: false,
            deletion: false,
            typing_indicator: false,
            read_receipts: false,
            rich_text: true,
            max_message_length: 1_048_576, // 1MB
            max_attachment_size: 0,
        }
    }

    /// Take the inbound receiver (can only be called once)
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.inbound_rx.take()
    }

    /// Get a clone of the inbound message sender.
    ///
    /// This sender should be passed to `WebhookReceiver::start()` so that
    /// incoming webhook messages are forwarded into the channel's inbound queue.
    pub fn inbound_sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Get the webhook handler for registration with WebhookReceiver.
    ///
    /// Returns `None` if `start()` has not been called yet.
    pub fn webhook_handler(&self) -> Option<Arc<GenericWebhookHandler>> {
        self.handler.clone()
    }

    /// Update internal status
    async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }
}

#[async_trait]
impl Channel for WebhookChannel {
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

        self.set_status(ChannelStatus::Connecting).await;
        tracing::info!(
            "Starting Webhook channel (path={}, callback_url={})...",
            self.config.path,
            self.config.callback_url
        );

        // Create the webhook handler
        let handler = Arc::new(GenericWebhookHandler {
            secret: self.config.secret.clone(),
            channel_id: self.info.id.clone(),
            path: self.config.path.clone(),
            allowed_senders: self.config.allowed_senders.clone(),
        });
        self.handler = Some(handler);

        // Webhook channel is ready — inbound messages arrive via WebhookReceiver
        // when the handler is registered. No background poll loop needed.
        self.set_status(ChannelStatus::Connected).await;
        tracing::info!("Webhook channel started (inbound via WebhookReceiver)");
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Webhook channel...");
        self.handler = None;
        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        WebhookMessageOps::send_outbound(
            &self.client,
            &self.config.callback_url,
            &self.config.secret,
            &message,
        )
        .await
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Already taken during construction or via take_receiver
    }
}

/// WebhookHandler implementation for the generic webhook channel.
///
/// This struct is registered with the shared `WebhookReceiver` HTTP server
/// to handle incoming POST requests on the configured path.
pub struct GenericWebhookHandler {
    /// HMAC-SHA256 secret for signature verification
    secret: String,
    /// Channel ID to tag inbound messages with
    channel_id: ChannelId,
    /// URL path this handler listens on
    path: String,
    /// Allowed sender IDs (empty = all allowed)
    allowed_senders: Vec<String>,
}

#[async_trait]
impl WebhookHandler for GenericWebhookHandler {
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> bool {
        let signature = headers
            .get("X-Webhook-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        WebhookReceiver::verify_signature(&self.secret, body, signature)
    }

    async fn handle(
        &self,
        _headers: &HeaderMap,
        body: Bytes,
    ) -> ChannelResult<Vec<InboundMessage>> {
        let mut messages = WebhookMessageOps::parse_inbound_payload(&body, &self.channel_id)?;

        // Filter by allowed senders if configured
        if !self.allowed_senders.is_empty() {
            messages.retain(|msg| {
                self.allowed_senders
                    .iter()
                    .any(|s| s == msg.sender_id.as_str())
            });
        }

        Ok(messages)
    }

    fn path(&self) -> &str {
        &self.path
    }
}

/// Factory for creating generic webhook channels
pub struct WebhookChannelFactory;

#[async_trait]
impl ChannelFactory for WebhookChannelFactory {
    fn channel_type(&self) -> &str {
        "webhook"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: WebhookChannelConfig = serde_json::from_value(config).map_err(|e| {
            ChannelError::ConfigError(format!("Invalid Webhook config: {}", e))
        })?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(WebhookChannel::new("webhook", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::ChannelId;

    // --- Capabilities ---

    #[test]
    fn test_channel_capabilities() {
        let caps = WebhookChannel::channel_capabilities();
        assert!(!caps.attachments);
        assert!(!caps.images);
        assert!(!caps.audio);
        assert!(!caps.video);
        assert!(!caps.reactions);
        assert!(!caps.replies);
        assert!(!caps.editing);
        assert!(!caps.deletion);
        assert!(!caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(caps.rich_text);
        assert_eq!(caps.max_message_length, 1_048_576);
        assert_eq!(caps.max_attachment_size, 0);
    }

    // --- Channel creation ---

    #[test]
    fn test_channel_creation() {
        let config = WebhookChannelConfig {
            secret: "test-secret".to_string(),
            callback_url: "https://example.com/cb".to_string(),
            ..Default::default()
        };
        let channel = WebhookChannel::new("webhook-test", config);
        assert_eq!(channel.info().id.as_str(), "webhook-test");
        assert_eq!(channel.info().channel_type, "webhook");
        assert_eq!(channel.info().name, "Webhook");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = WebhookChannelConfig::default();
        let channel = WebhookChannel::new("webhook", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = WebhookChannelConfig::default();
        let mut channel = WebhookChannel::new("webhook", config);

        // First take should succeed
        assert!(channel.take_receiver().is_some());

        // Second take should return None
        assert!(channel.take_receiver().is_none());
    }

    #[test]
    fn test_webhook_handler_not_available_before_start() {
        let config = WebhookChannelConfig::default();
        let channel = WebhookChannel::new("webhook", config);
        assert!(channel.webhook_handler().is_none());
    }

    // --- Factory ---

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = WebhookChannelFactory;
        assert_eq!(factory.channel_type(), "webhook");

        let config = serde_json::json!({
            "secret": "my-secret",
            "callback_url": "https://example.com/cb"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "webhook");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = WebhookChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_empty_secret() {
        let factory = WebhookChannelFactory;

        let config = serde_json::json!({
            "secret": "",
            "callback_url": "https://example.com/cb"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_with_all_options() {
        let factory = WebhookChannelFactory;

        let config = serde_json::json!({
            "secret": "my-secret",
            "callback_url": "https://example.com/cb",
            "path": "/webhook/custom",
            "allowed_senders": ["user-1", "user-2"]
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());
    }

    // --- WebhookHandler trait ---

    #[test]
    fn test_webhook_handler_verify_valid() {
        let handler = GenericWebhookHandler {
            secret: "test-secret".to_string(),
            channel_id: ChannelId::new("webhook"),
            path: "/webhook/generic".to_string(),
            allowed_senders: Vec::new(),
        };

        let body = b"test body content";
        let sig = WebhookReceiver::compute_signature("test-secret", body);

        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Signature", sig.parse().unwrap());

        assert!(handler.verify(&headers, body));
    }

    #[test]
    fn test_webhook_handler_verify_invalid() {
        let handler = GenericWebhookHandler {
            secret: "test-secret".to_string(),
            channel_id: ChannelId::new("webhook"),
            path: "/webhook/generic".to_string(),
            allowed_senders: Vec::new(),
        };

        let body = b"test body content";

        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Signature", "sha256=invalid".parse().unwrap());

        assert!(!handler.verify(&headers, body));
    }

    #[test]
    fn test_webhook_handler_verify_missing_header() {
        let handler = GenericWebhookHandler {
            secret: "test-secret".to_string(),
            channel_id: ChannelId::new("webhook"),
            path: "/webhook/generic".to_string(),
            allowed_senders: Vec::new(),
        };

        let body = b"test body content";
        let headers = HeaderMap::new(); // No signature header

        assert!(!handler.verify(&headers, body));
    }

    #[test]
    fn test_webhook_handler_path() {
        let handler = GenericWebhookHandler {
            secret: "s".to_string(),
            channel_id: ChannelId::new("webhook"),
            path: "/webhook/custom".to_string(),
            allowed_senders: Vec::new(),
        };

        assert_eq!(handler.path(), "/webhook/custom");
    }

    #[tokio::test]
    async fn test_webhook_handler_handle_valid() {
        let handler = GenericWebhookHandler {
            secret: "s".to_string(),
            channel_id: ChannelId::new("webhook"),
            path: "/webhook/generic".to_string(),
            allowed_senders: Vec::new(),
        };

        let body = serde_json::to_vec(&serde_json::json!({
            "sender_id": "user-1",
            "sender_name": "Alice",
            "message": "Hello!"
        }))
        .unwrap();

        let headers = HeaderMap::new();
        let messages = handler
            .handle(&headers, Bytes::from(body))
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "Hello!");
        assert_eq!(messages[0].sender_id.as_str(), "user-1");
        assert_eq!(messages[0].channel_id.as_str(), "webhook");
    }

    #[tokio::test]
    async fn test_webhook_handler_handle_filters_senders() {
        let handler = GenericWebhookHandler {
            secret: "s".to_string(),
            channel_id: ChannelId::new("webhook"),
            path: "/webhook/generic".to_string(),
            allowed_senders: vec!["allowed-user".to_string()],
        };

        let body = serde_json::to_vec(&serde_json::json!([
            {"sender_id": "allowed-user", "message": "Allowed"},
            {"sender_id": "blocked-user", "message": "Blocked"}
        ]))
        .unwrap();

        let headers = HeaderMap::new();
        let messages = handler
            .handle(&headers, Bytes::from(body))
            .await
            .unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].sender_id.as_str(), "allowed-user");
    }

    #[tokio::test]
    async fn test_webhook_handler_handle_invalid_json() {
        let handler = GenericWebhookHandler {
            secret: "s".to_string(),
            channel_id: ChannelId::new("webhook"),
            path: "/webhook/generic".to_string(),
            allowed_senders: Vec::new(),
        };

        let headers = HeaderMap::new();
        let result = handler
            .handle(&headers, Bytes::from("not json"))
            .await;

        assert!(result.is_err());
    }

    // --- Start/Stop lifecycle ---

    #[tokio::test]
    async fn test_start_creates_handler() {
        let config = WebhookChannelConfig {
            secret: "my-secret".to_string(),
            callback_url: "https://example.com/cb".to_string(),
            ..Default::default()
        };
        let mut channel = WebhookChannel::new("webhook", config);

        assert!(channel.webhook_handler().is_none());

        channel.start().await.unwrap();

        assert!(channel.webhook_handler().is_some());
        let handler = channel.webhook_handler().unwrap();
        assert_eq!(handler.path(), "/webhook/generic");
    }

    #[tokio::test]
    async fn test_start_with_invalid_config() {
        let config = WebhookChannelConfig::default(); // Empty secret
        let mut channel = WebhookChannel::new("webhook", config);

        let result = channel.start().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stop_clears_handler() {
        let config = WebhookChannelConfig {
            secret: "my-secret".to_string(),
            callback_url: "https://example.com/cb".to_string(),
            ..Default::default()
        };
        let mut channel = WebhookChannel::new("webhook", config);

        channel.start().await.unwrap();
        assert!(channel.webhook_handler().is_some());

        channel.stop().await.unwrap();
        assert!(channel.webhook_handler().is_none());
    }
}
