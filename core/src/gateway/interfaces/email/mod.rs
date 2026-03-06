//! Email Channel Implementation
//!
//! Integrates with email servers using IMAP for receiving messages (polling)
//! and SMTP for sending messages. Uses the subject line for agent routing
//! (e.g., `[coder] Fix this bug`).
//!
//! # Protocol
//!
//! - **IMAP** (polling): Connects to the configured IMAP server, polls for
//!   unseen messages in specified folders, and converts them to InboundMessages.
//! - **SMTP** (STARTTLS): Sends HTML emails via the configured SMTP server,
//!   converting Markdown body to a styled HTML email.
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "email"
//! channel_type = "email"
//! enabled = true
//!
//! [channels.config]
//! imap_host = "imap.gmail.com"
//! smtp_host = "smtp.gmail.com"
//! username = "aleph@gmail.com"
//! password = "app-password"
//! from_address = "aleph@gmail.com"
//! poll_interval_secs = 30
//! folders = ["INBOX"]
//! ```

pub mod config;
pub mod message_ops;

pub use config::EmailConfig;
pub use message_ops::EmailMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, InboundMessage, OutboundMessage,
    SendResult,
};
use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

/// Email channel implementation using IMAP + SMTP.
pub struct EmailChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: EmailConfig,
    /// Unified channel state (status + inbound sender/receiver)
    channel_state: ChannelState,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl EmailChannel {
    /// Create a new Email channel
    pub fn new(id: impl Into<String>, config: EmailConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Email".to_string(),
            channel_type: "email".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            shutdown_tx: None,
        }
    }

    /// Get Email-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: false,
            video: false,
            reactions: false,
            replies: true,       // via Re: subject
            editing: false,
            deletion: false,
            typing_indicator: false,
            read_receipts: false,
            rich_text: true,     // HTML email
            max_message_length: 1_048_576, // 1MB
            max_attachment_size: 25 * 1024 * 1024, // 25MB
        }
    }

    /// Take the inbound receiver (can only be called once)
    pub fn take_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.channel_state.take_receiver()
    }
}

#[async_trait]
impl Channel for EmailChannel {
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
        tracing::info!(
            "Starting Email channel (IMAP: {}:{}, SMTP: {}:{})...",
            self.config.imap_host,
            self.config.imap_port,
            self.config.smtp_host,
            self.config.smtp_port,
        );

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Spawn IMAP polling loop
        let config = self.config.clone();
        let channel_id = self.info.id.clone();
        let inbound_tx = self.channel_state.sender();
        let status = self.channel_state.status_handle();

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;

            EmailMessageOps::run_imap_poll_loop(
                config,
                inbound_tx,
                channel_id,
                shutdown_rx,
            )
            .await;

            *status.write().await = ChannelStatus::Disconnected;
        });

        self.channel_state.set_status(ChannelStatus::Connected).await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Email channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.channel_state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        // conversation_id is the recipient email address
        let to = message.conversation_id.as_str();

        // Use subject from metadata if provided, otherwise default
        let subject = message
            .metadata
            .get("subject")
            .cloned()
            .unwrap_or_else(|| "Message from Aleph".to_string());

        EmailMessageOps::send_email(&self.config, to, &subject, &message.text).await

    }

    // inbound_receiver() — uses default from Channel trait via state()
    // send_typing() — uses default from Channel trait (returns UnsupportedFeature via capabilities)
}

/// Factory for creating Email channels
pub struct EmailChannelFactory;

#[async_trait]
impl ChannelFactory for EmailChannelFactory {
    fn channel_type(&self) -> &str {
        "email"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: EmailConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Email config: {}", e)))?;

        config
            .validate()
            .map_err(ChannelError::ConfigError)?;

        Ok(Box::new(EmailChannel::new("email", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = EmailChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(!caps.audio);
        assert!(!caps.video);
        assert!(!caps.reactions);
        assert!(caps.replies);
        assert!(!caps.editing);
        assert!(!caps.deletion);
        assert!(!caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(caps.rich_text);
        assert_eq!(caps.max_message_length, 1_048_576);
        assert_eq!(caps.max_attachment_size, 25 * 1024 * 1024);
    }

    #[test]
    fn test_channel_creation() {
        let config = EmailConfig {
            imap_host: "imap.test.com".to_string(),
            smtp_host: "smtp.test.com".to_string(),
            username: "user@test.com".to_string(),
            password: "pass".to_string(),
            from_address: "aleph@test.com".to_string(),
            ..Default::default()
        };
        let channel = EmailChannel::new("email-test", config);
        assert_eq!(channel.info().id.as_str(), "email-test");
        assert_eq!(channel.info().channel_type, "email");
        assert_eq!(channel.info().name, "Email");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = EmailConfig::default();
        let channel = EmailChannel::new("email", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = EmailConfig::default();
        let channel = EmailChannel::new("email", config);

        // First take should succeed
        assert!(channel.take_receiver().is_some());

        // Second take should return None
        assert!(channel.take_receiver().is_none());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = EmailChannelFactory;
        assert_eq!(factory.channel_type(), "email");

        let config = serde_json::json!({
            "imap_host": "imap.gmail.com",
            "smtp_host": "smtp.gmail.com",
            "username": "user@gmail.com",
            "password": "app-password",
            "from_address": "aleph@gmail.com"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "email");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = EmailChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_email() {
        let factory = EmailChannelFactory;

        let config = serde_json::json!({
            "imap_host": "imap.test.com",
            "smtp_host": "smtp.test.com",
            "username": "user",
            "password": "pass",
            "from_address": "not-an-email"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_without_feature() {
        let config = EmailConfig {
            imap_host: "imap.test.com".to_string(),
            smtp_host: "smtp.test.com".to_string(),
            username: "user@test.com".to_string(),
            password: "pass".to_string(),
            from_address: "aleph@test.com".to_string(),
            ..Default::default()
        };
        let _channel = EmailChannel::new("email", config);

        // Without the email feature, start should return UnsupportedFeature.
        // When the email feature IS enabled, start() requires live IMAP/SMTP servers
        // which cannot be tested in unit tests, so this test only validates
        // construction succeeds.
    }

    #[tokio::test]
    async fn test_send_typing_unsupported() {
        let config = EmailConfig::default();
        let channel = EmailChannel::new("email", config);
        let result = channel
            .send_typing(&ConversationId::new("user@test.com"))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ChannelError::UnsupportedFeature(msg) => {
                assert!(msg.contains("typing"));
            }
            other => panic!("Expected UnsupportedFeature, got: {:?}", other),
        }
    }
}
