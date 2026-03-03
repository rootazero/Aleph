//! XMPP Channel Implementation
//!
//! Integrates with XMPP servers using raw TCP and manual XML stanza handling
//! (RFC 6120/6121). Supports MUC (Multi-User Chat) group chat via XEP-0045.
//! No external XMPP library dependencies — uses `tokio::net::TcpStream` with
//! manual stanza building/parsing.
//!
//! # Protocol
//!
//! - **Connection:** Raw TCP to server:port, XML stream negotiation
//! - **Auth:** SASL PLAIN over the XML stream
//! - **Receiving:** Parse `<message>` stanzas from the stream
//! - **Sending:** Build and write `<message>` stanzas to the stream
//! - **MUC:** Join rooms via `<presence>` with MUC extension
//! - **Keepalive:** Respond to `<iq>` ping requests
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "xmpp"
//! channel_type = "xmpp"
//! enabled = true
//!
//! [channels.config]
//! jid = "bot@example.com"
//! password = "secret"
//! muc_rooms = ["room@conference.example.com"]
//! ```

pub mod config;
pub mod message_ops;

pub use config::XmppConfig;
pub use message_ops::XmppMessageOps;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelStatus, ConversationId, InboundMessage, MessageId, OutboundMessage,
    SendResult,
};
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use tokio::sync::{mpsc, watch, RwLock};

/// XMPP channel implementation using raw TCP (RFC 6120/6121 + XEP-0045 MUC).
pub struct XmppChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: XmppConfig,
    /// Inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Inbound message receiver (taken on first call)
    inbound_rx: Option<mpsc::Receiver<InboundMessage>>,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Current status
    status: Arc<RwLock<ChannelStatus>>,
    /// Write channel for sending raw XMPP stanzas
    write_tx: Arc<RwLock<Option<mpsc::Sender<String>>>>,
}

impl XmppChannel {
    /// Create a new XMPP channel
    pub fn new(id: impl Into<String>, config: XmppConfig) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "XMPP".to_string(),
            channel_type: "xmpp".to_string(),
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

    /// Get XMPP-specific capabilities
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
            typing_indicator: true,  // XEP-0085 Chat State Notifications
            read_receipts: true,     // XEP-0184 Message Delivery Receipts
            rich_text: false,        // Using plain text for simplicity
            max_message_length: 65535,
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
impl Channel for XmppChannel {
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
            "Starting XMPP channel (jid={}, rooms={})...",
            self.config.jid,
            self.config.muc_rooms.len()
        );

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Create write channel for outbound stanzas
        let (write_cmd_tx, write_cmd_rx) = mpsc::channel::<String>(64);
        *self.write_tx.write().await = Some(write_cmd_tx);

        // Spawn XMPP connection loop
        let config = self.config.clone();
        let channel_id = self.info.id.clone();
        let inbound_tx = self.inbound_tx.clone();
        let status = self.status.clone();

        tokio::spawn(async move {
            *status.write().await = ChannelStatus::Connected;

            XmppMessageOps::run_xmpp_loop(
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
        tracing::info!("Stopping XMPP channel...");

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
                "XMPP adapter not started - call start() first".to_string(),
            )
        })?;

        // Determine message type based on conversation ID
        // MUC rooms contain '@conference' or similar patterns
        // We use groupchat for any room in the muc_rooms list
        let conversation = message.conversation_id.as_str();
        let msg_type = if self.config.muc_rooms.iter().any(|r| r == conversation) {
            "groupchat"
        } else {
            "chat"
        };

        XmppMessageOps::send_message(write_tx, conversation, &message.text, msg_type).await

    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        None // Already taken during construction or via take_receiver
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        // XMPP supports typing via XEP-0085 Chat State Notifications
        // For now, send a <composing/> chat state
        let write_tx = self.write_tx.read().await;
        if let Some(write_tx) = write_tx.as_ref() {
            let stanza = format!(
                "<message to='{}' type='chat'>\
                 <composing xmlns='http://jabber.org/protocol/chatstates'/>\
                 </message>",
                conversation_id.as_str()
            );
            let _ = write_tx.send(stanza).await;
        }
        Ok(())

    }

    async fn edit(&self, message_id: &MessageId, new_text: &str) -> ChannelResult<()> {
        let _ = (message_id, new_text);
        Err(ChannelError::UnsupportedFeature(
            "XMPP message editing not implemented".to_string(),
        ))
    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        let _ = (message_id, reaction);
        Err(ChannelError::UnsupportedFeature(
            "XMPP reactions not implemented".to_string(),
        ))
    }
}

/// Factory for creating XMPP channels
pub struct XmppChannelFactory;

#[async_trait]
impl ChannelFactory for XmppChannelFactory {
    fn channel_type(&self) -> &str {
        "xmpp"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: XmppConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid XMPP config: {}", e)))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(XmppChannel::new("xmpp", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = XmppChannel::capabilities();
        assert!(!caps.attachments);
        assert!(!caps.images);
        assert!(!caps.audio);
        assert!(!caps.video);
        assert!(!caps.reactions);
        assert!(!caps.replies);
        assert!(!caps.editing);
        assert!(!caps.deletion);
        assert!(caps.typing_indicator);
        assert!(caps.read_receipts);
        assert!(!caps.rich_text);
        assert_eq!(caps.max_message_length, 65535);
        assert_eq!(caps.max_attachment_size, 0);
    }

    #[test]
    fn test_channel_creation() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let channel = XmppChannel::new("xmpp-test", config);
        assert_eq!(channel.info().id.as_str(), "xmpp-test");
        assert_eq!(channel.info().channel_type, "xmpp");
        assert_eq!(channel.info().name, "XMPP");
    }

    #[test]
    fn test_channel_initial_status() {
        let config = XmppConfig::default();
        let channel = XmppChannel::new("xmpp", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = XmppConfig::default();
        let mut channel = XmppChannel::new("xmpp", config);

        // First take should succeed
        assert!(channel.take_receiver().is_some());

        // Second take should return None
        assert!(channel.take_receiver().is_none());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = XmppChannelFactory;
        assert_eq!(factory.channel_type(), "xmpp");

        let config = serde_json::json!({
            "jid": "bot@example.com",
            "password": "secret"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "xmpp");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = XmppChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_missing_password() {
        let factory = XmppChannelFactory;

        let config = serde_json::json!({
            "jid": "bot@example.com"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_jid() {
        let factory = XmppChannelFactory;

        let config = serde_json::json!({
            "jid": "no-at-sign",
            "password": "secret"
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_with_all_options() {
        let factory = XmppChannelFactory;

        let config = serde_json::json!({
            "jid": "bot@example.com",
            "password": "secret",
            "server": "xmpp.example.com",
            "port": 5223,
            "muc_rooms": ["room@conference.example.com"],
            "use_tls": false,
            "nick": "mybot"
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_muc_room() {
        let factory = XmppChannelFactory;

        let config = serde_json::json!({
            "jid": "bot@example.com",
            "password": "secret",
            "muc_rooms": ["invalid-room"]
        });

        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_start_without_feature() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let _channel = XmppChannel::new("xmpp", config);

        // Without the xmpp feature, start should return UnsupportedFeature.
        // When the xmpp feature IS enabled, start() requires a live XMPP server
        // which cannot be tested in unit tests, so this test only validates
        // construction succeeds.
    }

    #[tokio::test]
    async fn test_edit_unsupported() {
        let config = XmppConfig::default();
        let channel = XmppChannel::new("xmpp", config);

        let msg_id = MessageId::new("test-msg");
        let result = channel.edit(&msg_id, "new text").await;
        assert!(result.is_err());
        if let Err(ChannelError::UnsupportedFeature(msg)) = result {
            assert!(msg.contains("editing"));
        }
    }

    #[tokio::test]
    async fn test_react_unsupported() {
        let config = XmppConfig::default();
        let channel = XmppChannel::new("xmpp", config);

        let msg_id = MessageId::new("test-msg");
        let result = channel.react(&msg_id, "thumbsup").await;
        assert!(result.is_err());
        if let Err(ChannelError::UnsupportedFeature(msg)) = result {
            assert!(msg.contains("reactions"));
        }
    }

    #[tokio::test]
    async fn test_send_without_start() {
        let config = XmppConfig {
            jid: "bot@example.com".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let channel = XmppChannel::new("xmpp", config);

        let msg = OutboundMessage::text("alice@example.com", "Hello!");

        // Send should fail because adapter is not started
        // (write_tx is None)
        let result = channel.send(msg).await;

        // When compiled without the xmpp feature, it returns UnsupportedFeature
        // When compiled with the xmpp feature, it returns NotConnected
        assert!(result.is_err());
    }
}
