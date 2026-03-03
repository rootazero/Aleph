//! IRC Channel Implementation
//!
//! Integrates with IRC servers using raw TCP and the RFC 2812 text protocol.
//! No external IRC library dependencies — uses `tokio::net::TcpStream` with
//! `tokio::io` buffered I/O directly.
//!
//! # Protocol
//!
//! - **Receiving:** Reads `\r\n`-terminated lines from the TCP stream
//! - **Sending:** `PRIVMSG <target> :<message>\r\n`
//! - **Auth:** NICK/USER registration, optional NickServ IDENTIFY
//! - **Keepalive:** PING/PONG automatic response
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "irc"
//! channel_type = "irc"
//! enabled = true
//!
//! [channels.config]
//! server = "irc.libera.chat"
//! port = 6667
//! nick = "alephbot"
//! channels = ["#aleph", "#test"]
//! ```

pub mod config;
pub mod message_ops;

pub use config::IrcConfig;
pub use message_ops::IrcMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelStatus, ConversationId, InboundMessage, MessageId, OutboundMessage,
    SendResult,
};
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use tokio::sync::{mpsc, watch, RwLock};

/// IRC channel implementation using raw TCP (RFC 2812).
pub struct IrcChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: IrcConfig,
    /// Inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken on first call)
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Current status
    status: Arc<RwLock<ChannelStatus>>,
    /// Write channel for sending raw IRC commands
    write_tx: Arc<RwLock<Option<mpsc::Sender<String>>>>,
}

impl IrcChannel {
    /// Create a new IRC channel
    pub fn new(id: impl Into<String>, config: IrcConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "IRC".to_string(),
            channel_type: "irc".to_string(),
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
            write_tx: Arc::new(RwLock::new(None)),
        }
    }

    /// Get IRC-specific capabilities
    fn capabilities() -> ChannelCapabilities {
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
            rich_text: false, // IRC has minimal formatting (mIRC codes)
            max_message_length: 400, // Conservative PRIVMSG limit
            max_attachment_size: 0,
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
impl Channel for IrcChannel {
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
            "Starting IRC channel (server={}, nick={})...",
            self.config.server,
            self.config.nick
        );

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Create write channel for outbound messages
        let (write_cmd_tx, write_cmd_rx) = mpsc::channel::<String>(64);
        *self.write_tx.write().await = Some(write_cmd_tx);

        // Spawn IRC connection loop
        let config = self.config.clone();
        let channel_id = self.info.id.clone();
        let inbound_tx = self.inbound_tx.clone();
        let status = self.status.clone();

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;

            IrcMessageOps::run_irc_loop(
                config,
                channel_id,
                inbound_tx,
                write_cmd_rx,
                shutdown_rx,
            )
            .await;

            *status.write().await = ChannelStatus::Disconnected;
        });

        self.set_status(ChannelStatus::Connected).await;
        Ok(())

    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping IRC channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        // Clear write channel
        *self.write_tx.write().await = None;

        self.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let write_tx = self.write_tx.read().await;
        let write_tx = write_tx.as_ref().ok_or_else(|| {
            ChannelError::NotConnected(
                "IRC adapter not started - call start() first".to_string(),
            )
        })?;

        IrcMessageOps::send_message(
            write_tx,
            message.conversation_id.as_str(),
            &message.text,
        )
        .await

    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Already taken during construction or via take_receiver
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        // IRC does not support typing indicators
        let _ = conversation_id;
        Err(ChannelError::UnsupportedFeature(
            "IRC does not support typing indicators".to_string(),
        ))
    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        let _ = (message_id, new_text);
        Err(ChannelError::UnsupportedFeature(
            "IRC does not support message editing".to_string(),
        ))
    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        let _ = (message_id, reaction);
        Err(ChannelError::UnsupportedFeature(
            "IRC does not support reactions".to_string(),
        ))
    }
}

/// Factory for creating IRC channels
pub struct IrcChannelFactory;

#[async_trait]
impl ChannelFactory for IrcChannelFactory {
    fn channel_type(&self) -> &str {
        "irc"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: IrcConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid IRC config: {}", e)))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(IrcChannel::new("irc", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = IrcChannel::capabilities();
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
        assert!(!caps.rich_text);
        assert_eq!(caps.max_message_length, 400);
        assert_eq!(caps.max_attachment_size, 0);
    }

    #[test]
    fn test_channel_creation() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            nick: "alephbot".to_string(),
            channels: vec!["#test".to_string()],
            ..Default::default()
        };
        let channel = IrcChannel::new("irc-test", config);
        assert_eq!(channel.info().id.as_str(), "irc-test");
        assert_eq!(channel.info().channel_type, "irc");
        assert_eq!(channel.info().name, "IRC");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = IrcConfig::default();
        let channel = IrcChannel::new("irc", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = IrcConfig::default();
        let mut channel = IrcChannel::new("irc", config);

        // First take should succeed
        assert!(channel.take_receiver().is_some());

        // Second take should return None
        assert!(channel.take_receiver().is_none());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = IrcChannelFactory;
        assert_eq!(factory.channel_type(), "irc");

        let config = serde_json::json!({
            "server": "irc.libera.chat",
            "nick": "alephbot",
            "channels": ["#test"]
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "irc");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = IrcChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_missing_server() {
        let factory = IrcChannelFactory;

        let config = serde_json::json!({
            "nick": "alephbot",
            "channels": ["#test"]
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_channel() {
        let factory = IrcChannelFactory;

        let config = serde_json::json!({
            "server": "irc.libera.chat",
            "nick": "alephbot",
            "channels": ["nochanprefix"]
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_with_all_options() {
        let factory = IrcChannelFactory;

        let config = serde_json::json!({
            "server": "irc.libera.chat",
            "port": 6697,
            "nick": "alephbot",
            "password": "secret",
            "channels": ["#aleph", "#test"],
            "use_tls": true,
            "realname": "Aleph IRC Bot"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_start_without_feature() {
        let config = IrcConfig {
            server: "irc.libera.chat".to_string(),
            nick: "alephbot".to_string(),
            channels: vec!["#test".to_string()],
            ..Default::default()
        };
        let _channel = IrcChannel::new("irc", config);

        // Without the irc feature, start should return UnsupportedFeature.
        // When the irc feature IS enabled, start() requires a live IRC server
        // which cannot be tested in unit tests, so this test only validates
        // construction succeeds.
    }

    #[tokio::test]
    async fn test_send_typing_unsupported() {
        let config = IrcConfig::default();
        let channel = IrcChannel::new("irc", config);

        let conv_id = ConversationId::new("#test");
        let result = channel.send_typing(&conv_id).await;
        assert!(result.is_err());
        if let Err(ChannelError::UnsupportedFeature(msg)) = result {
            assert!(msg.contains("typing"));
        }
    }

    #[tokio::test]
    async fn test_edit_unsupported() {
        let config = IrcConfig::default();
        let channel = IrcChannel::new("irc", config);

        let msg_id = MessageId::new("test-msg");
        let result = channel.edit(&msg_id, "new text").await;
        assert!(result.is_err());
        if let Err(ChannelError::UnsupportedFeature(msg)) = result {
            assert!(msg.contains("editing"));
        }
    }

    #[tokio::test]
    async fn test_react_unsupported() {
        let config = IrcConfig::default();
        let channel = IrcChannel::new("irc", config);

        let msg_id = MessageId::new("test-msg");
        let result = channel.react(&msg_id, "thumbsup").await;
        assert!(result.is_err());
        if let Err(ChannelError::UnsupportedFeature(msg)) = result {
            assert!(msg.contains("reactions"));
        }
    }
}
