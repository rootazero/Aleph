//! Email Channel Implementation
//!
//! Integrates with email servers using IMAP for receiving messages (polling)
//! and SMTP for sending messages.
//!
//! # Protocol
//!
//! - **IMAP** (TLS): Connects to the configured IMAP server, polls for
//!   unseen messages in specified folders, and converts them to `InboundMessages`.
//! - **SMTP** (STARTTLS): Sends HTML emails via the configured SMTP server,
//!   converting Markdown body to a styled HTML email. A reply carries the
//!   inbound `Message-ID` as `In-Reply-To`/`References` and repeats the inbound
//!   subject as `Re: <subject>`, so the answer stays in the same mail thread.
//!
//! # Usage
//!
//! `Config.channels` is a MAP keyed by instance id, so the table header carries
//! the id and the keys are flat. The `[[channels]]` array-of-tables form this
//! example used until 2026-11-22 does not parse at all — the server refuses to
//! boot with `invalid type: sequence, expected a map`.
//!
//! ```toml
//! [channels.email]
//! type = "email"
//! enabled = true
//! imap_host = "imap.gmail.com"
//! smtp_host = "smtp.gmail.com"
//! username = "aleph@gmail.com"
//! password = "app-password"
//! from_address = "aleph@gmail.com"
//! poll_interval_secs = 30
//! folders = ["INBOX"]
//! ```
//!
//! `use_tls = false` is rejected by [`EmailConfig::validate`]: both protocols
//! authenticate with the same password, so neither may run in cleartext.

pub mod config;
pub mod message_ops;

pub use config::EmailConfig;
pub use message_ops::{
    new_inbound_thread_store, EmailMessageOps, InboundThread, InboundThreadStore,
};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, InboundMessage, MessageId, OutboundMessage,
    SendOperation, SendResult,
};
use async_trait::async_trait;
use tokio::sync::{broadcast, watch};

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
    /// Test mode: skip real SMTP/IMAP operations
    test_mode: bool,
    /// Identity of the last inbound mail per conversation, so replies thread
    inbound_threads: InboundThreadStore,
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
            test_mode: false,
            inbound_threads: new_inbound_thread_store(),
        }
    }

    /// Create an Email channel for testing (no real SMTP/IMAP operations).
    pub fn for_test(id: impl Into<String>, config: EmailConfig) -> Self {
        let mut channel = Self::new(id, config);
        channel.test_mode = true;
        channel
    }

    /// Get Email-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: false,
            audio: false,
            video: false,
            reactions: false,
            replies: true, // via Re: subject
            editing: false,
            deletion: false,
            typing_indicator: false,
            read_receipts: false,
            rich_text: true, // HTML email
            polls: false,
            group_icons: false,
            max_message_length: 1_048_576, // 1MB
            max_attachment_size: 0,
            stream_protocol: Default::default(),
        }
    }

    /// Take the inbound receiver (can only be called once)
    #[must_use]
    pub fn take_receiver(&self) -> Option<broadcast::Receiver<InboundMessage>> {
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
        self.config.validate().map_err(ChannelError::ConfigError)?;

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;
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

        if self.test_mode {
            self.channel_state
                .set_status(ChannelStatus::Connected)
                .await;
            return Ok(());
        }

        // Reach the server before claiming `Connected`: a rejected login or an
        // unreachable host must fail `start`, not disappear into a warning in
        // a poll loop nobody is watching.
        if let Err(error) = EmailMessageOps::check_imap_connection(&self.config).await {
            self.channel_state.set_status(ChannelStatus::Error).await;
            return Err(error);
        }

        // Spawn IMAP polling loop
        let config = self.config.clone();
        let channel_id = self.info.id.clone();
        let inbound_tx = self.channel_state.sender();
        let status = self.channel_state.status_handle();
        let threads = self.inbound_threads.clone();

        tokio::spawn(async move {
            match EmailMessageOps::run_imap_poll_loop(
                config,
                inbound_tx,
                channel_id,
                threads,
                shutdown_rx,
            )
            .await
            {
                Ok(()) => *status.write().await = ChannelStatus::Disconnected,
                // A fatal poll failure (bad config, rejected login) is
                // reported as `Error`, never as a healthy disconnect.
                Err(error) => {
                    tracing::error!("Email IMAP poll loop stopped with a fatal error: {error}");
                    *status.write().await = ChannelStatus::Error;
                }
            }
        });

        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Email channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    fn send(&self, message: OutboundMessage) -> SendOperation {
        let config = self.config.clone();
        let test_mode = self.test_mode;
        let inbound_threads = self.inbound_threads.clone();
        Box::pin(async move {
            // Fail closed: the SMTP path carries text only, so an attachment or
            // button request must error out instead of being dropped.
            if !message.attachments.is_empty() {
                return Err(ChannelError::UnsupportedFeature(
                    "Email does not support attachments".to_string(),
                ));
            }
            if message.inline_keyboard.is_some() {
                return Err(ChannelError::UnsupportedFeature(
                    "Email does not support buttons".to_string(),
                ));
            }

            // Test mode: skip real SMTP, return mock success
            if test_mode {
                return Ok(SendResult {
                    message_id: MessageId::new(format!("email-{}", chrono::Utc::now().timestamp())),
                    timestamp: chrono::Utc::now(),
                });
            }

            // conversation_id is the recipient email address
            let to = message.conversation_id.as_str();

            // Explicit subject wins; otherwise repeat the subject of the last
            // inbound mail from this address so the reply lands in its thread.
            let subject = message
                .metadata
                .get("subject")
                .cloned()
                .or_else(|| message_ops::reply_subject_for(&inbound_threads, to))
                .unwrap_or_else(|| "Message from Aleph".to_string());

            // Same for the identity the reply points at: an explicit
            // `reply_to` wins, else the `Message-ID` of the last inbound mail.
            let in_reply_to = message
                .reply_to
                .as_ref()
                .map(|id| id.as_str().to_string())
                .or_else(|| message_ops::reply_message_id_for(&inbound_threads, to));

            EmailMessageOps::send_email(
                &config,
                to,
                &subject,
                &message.text,
                in_reply_to.as_deref(),
            )
            .await
        })
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
        self.create_with_id("email", config).await
    }

    async fn create_with_id(
        &self,
        id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<Box<dyn Channel>> {
        let config: EmailConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Email config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(EmailChannel::new(id, config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::ConversationId;

    #[test]
    fn test_channel_capabilities() {
        let caps = EmailChannel::capabilities();
        assert!(!caps.attachments);
        assert!(!caps.images);
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
        assert_eq!(caps.max_attachment_size, 0);
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

        // Broadcast semantics: every call subscribes a fresh receiver.
        assert!(channel.take_receiver().is_some());
        assert!(channel.take_receiver().is_some());
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
    async fn test_send_rejects_attachments() {
        use crate::gateway::channel::Attachment;

        let channel = EmailChannel::for_test("email", EmailConfig::default());
        let mut message = OutboundMessage::text("user@test.com", "hello");
        message.attachments.push(Attachment {
            id: "a1".to_string(),
            mime_type: "image/png".to_string(),
            filename: Some("a.png".to_string()),
            size: Some(3),
            url: None,
            path: None,
            data: None,
            is_voice_note: false,
            is_video_note: false,
        });

        match channel.send(message).await.unwrap_err() {
            ChannelError::UnsupportedFeature(msg) => assert!(msg.contains("attachments"), "{msg}"),
            other => panic!("Expected UnsupportedFeature, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_send_rejects_inline_keyboard() {
        use crate::gateway::channel::{InlineButton, InlineKeyboard};

        let channel = EmailChannel::for_test("email", EmailConfig::default());
        let mut message = OutboundMessage::text("user@test.com", "hello");
        message.inline_keyboard = Some(InlineKeyboard {
            rows: vec![vec![InlineButton {
                text: "OK".to_string(),
                callback_data: "ok".to_string(),
            }]],
        });

        match channel.send(message).await.unwrap_err() {
            ChannelError::UnsupportedFeature(msg) => assert!(msg.contains("buttons"), "{msg}"),
            other => panic!("Expected UnsupportedFeature, got: {other:?}"),
        }
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
