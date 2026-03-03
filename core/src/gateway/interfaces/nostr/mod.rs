//! Nostr Channel Implementation
//!
//! Integrates with Nostr relays using NIP-01 WebSocket protocol for receiving events
//! and publishing messages. No external Nostr SDK required — uses `tokio-tungstenite`
//! for WebSocket, `k256` for secp256k1 key derivation and Schnorr signing, and
//! `sha2` for SHA-256 event ID computation.
//!
//! # Protocol
//!
//! - **NIP-01**: Basic protocol flow. Events, subscriptions, relay messages.
//! - **NIP-04**: Encrypted direct messages (kind 4). Currently plaintext only;
//!   encryption is documented as a future enhancement.
//! - **Schnorr signatures (BIP-340)**: Events signed with secp256k1 Schnorr.
//!
//! # Usage
//!
//! ```toml
//! [[channels]]
//! id = "nostr"
//! channel_type = "nostr"
//! enabled = true
//!
//! [channels.config]
//! private_key = "hex-encoded-32-byte-private-key"
//! relays = ["wss://relay.damus.io", "wss://nos.lol"]
//! allowed_pubkeys = []
//! subscription_kinds = [1, 4]
//! ```

pub mod config;
pub mod message_ops;

pub use config::NostrConfig;
pub use message_ops::{NostrEvent, NostrMessageOps};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, InboundMessage, MessageId, OutboundMessage,
    SendResult,
};
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use tokio::sync::{mpsc, watch, RwLock};

/// Nostr channel implementation using NIP-01 WebSocket relay protocol.
pub struct NostrChannel {
    /// Channel information
    info: ChannelInfo,
    /// Configuration
    config: NostrConfig,
    /// Shared mutable state (status + inbound channel)
    channel_state: ChannelState,
    /// Outbound write command sender (sends raw JSON to relay)
    write_tx: Option<mpsc::Sender<String>>,
    /// Shutdown signal sender
    shutdown_tx: Option<watch::Sender<bool>>,
    /// Our own public key (derived from private key)
    own_pubkey: String,
}

impl NostrChannel {
    /// Create a new Nostr channel
    pub fn new(id: impl Into<String>, config: NostrConfig) -> Self {
        // Derive public key from private key (best-effort at construction)
        let own_pubkey = message_ops::derive_pubkey(&config.private_key).unwrap_or_default();

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Nostr".to_string(),
            channel_type: "nostr".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            write_tx: None,
            shutdown_tx: None,
            own_pubkey,
        }
    }

    /// Get Nostr-specific capabilities
    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: false,
            audio: false,
            video: false,
            reactions: true,   // NIP-25: reactions (kind 7)
            replies: true,     // Via "e" tags
            editing: false,    // Nostr events are immutable
            deletion: false,   // NIP-09 exists but relays may ignore
            typing_indicator: false,
            read_receipts: false,
            rich_text: false,  // Plain text only
            max_message_length: 65535,
            max_attachment_size: 0,
        }
    }

}

#[async_trait]
impl Channel for NostrChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Validate configuration
        self.config.validate().map_err(ChannelError::ConfigError)?;

        #[cfg(feature = "nostr")]
        {
            self.channel_state.set_status(ChannelStatus::Connecting).await;
            tracing::info!("Starting Nostr channel...");

            // Derive public key
            let own_pubkey = message_ops::derive_pubkey(&self.config.private_key)
                .map_err(|e| ChannelError::ConfigError(format!("invalid private key: {e}")))?;
            self.own_pubkey = own_pubkey.clone();

            tracing::info!(
                "Nostr identity: {}...{}",
                &own_pubkey[..8.min(own_pubkey.len())],
                &own_pubkey[own_pubkey.len().saturating_sub(8)..]
            );

            // Create shutdown channel
            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            self.shutdown_tx = Some(shutdown_tx);

            // Create write command channel for outbound messages
            let (write_tx, write_rx) = mpsc::channel(100);
            self.write_tx = Some(write_tx);

            // Spawn relay connection loop
            let config = self.config.clone();
            let channel_id = self.info.id.clone();
            let inbound_tx = self.channel_state.sender();
            let status = self.channel_state.status_handle();

            tokio::spawn(async move {
                *status.write().await = ChannelStatus::Connected;

                NostrMessageOps::run_relay_loop(
                    config,
                    own_pubkey,
                    channel_id,
                    inbound_tx,
                    write_rx,
                    shutdown_rx,
                )
                .await;

                *status.write().await = ChannelStatus::Disconnected;
            });

            self.channel_state.set_status(ChannelStatus::Connected).await;
            Ok(())
        }

        #[cfg(not(feature = "nostr"))]
        {
            Err(ChannelError::UnsupportedFeature(
                "Nostr support not compiled (enable 'nostr' feature)".to_string(),
            ))
        }
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping Nostr channel...");

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(true);
        }

        self.write_tx = None;
        self.channel_state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        #[cfg(feature = "nostr")]
        {
            let write_tx = self
                .write_tx
                .as_ref()
                .ok_or_else(|| ChannelError::NotConnected("Nostr channel not started".to_string()))?;

            // Determine if this is a DM or public note based on conversation_id
            let recipient_pubkey = if message.conversation_id.as_str() != "public" {
                Some(message.conversation_id.as_str().to_string())
            } else {
                None
            };

            // Build the event
            let mut event = if let Some(ref recipient) = recipient_pubkey {
                message_ops::build_dm(&message.text, &self.own_pubkey, recipient)
            } else {
                message_ops::build_text_note(&message.text, &self.own_pubkey)
            };

            // Sign the event
            message_ops::sign_event(&mut event, &self.config.private_key)
                .map_err(|e| ChannelError::SendFailed(format!("failed to sign event: {e}")))?;

            let event_id = event.id.clone();

            // Build and send the EVENT message
            let event_msg = message_ops::build_event_message(&event);
            write_tx
                .send(event_msg)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("write channel closed: {e}")))?;

            Ok(SendResult {
                message_id: MessageId::new(event_id),
                timestamp: chrono::Utc::now(),
            })
        }

        #[cfg(not(feature = "nostr"))]
        {
            let _ = message;
            Err(ChannelError::UnsupportedFeature(
                "Nostr support not compiled".to_string(),
            ))
        }
    }

    async fn react(&self, message_id: &MessageId, reaction: &str) -> ChannelResult<()> {
        #[cfg(feature = "nostr")]
        {
            let write_tx = self
                .write_tx
                .as_ref()
                .ok_or_else(|| ChannelError::NotConnected("Nostr channel not started".to_string()))?;

            // Build a kind-7 reaction event
            // We don't know the original event's author pubkey, use empty string
            // (relays typically don't validate this for reactions)
            let mut event = message_ops::build_reaction(
                reaction,
                message_id.as_str(),
                "", // original author pubkey unknown
                &self.own_pubkey,
            );

            // Sign the event
            message_ops::sign_event(&mut event, &self.config.private_key)
                .map_err(|e| ChannelError::SendFailed(format!("failed to sign reaction: {e}")))?;

            let event_msg = message_ops::build_event_message(&event);
            write_tx
                .send(event_msg)
                .await
                .map_err(|e| ChannelError::SendFailed(format!("write channel closed: {e}")))?;

            Ok(())
        }

        #[cfg(not(feature = "nostr"))]
        {
            let _ = (message_id, reaction);
            Err(ChannelError::UnsupportedFeature(
                "Nostr support not compiled".to_string(),
            ))
        }
    }
}

/// Factory for creating Nostr channels
pub struct NostrChannelFactory;

#[async_trait]
impl ChannelFactory for NostrChannelFactory {
    fn channel_type(&self) -> &str {
        "nostr"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: NostrConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Nostr config: {}", e)))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(NostrChannel::new("nostr", config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A valid 32-byte hex private key for testing (NOT a real key)
    const TEST_PRIVKEY: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn test_channel_capabilities() {
        let caps = NostrChannel::capabilities();
        assert!(!caps.attachments);
        assert!(!caps.images);
        assert!(!caps.audio);
        assert!(!caps.video);
        assert!(caps.reactions);
        assert!(caps.replies);
        assert!(!caps.editing);
        assert!(!caps.deletion);
        assert!(!caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(!caps.rich_text);
        assert_eq!(caps.max_message_length, 65535);
        assert_eq!(caps.max_attachment_size, 0);
    }

    #[test]
    fn test_channel_creation() {
        let config = NostrConfig {
            private_key: TEST_PRIVKEY.to_string(),
            relays: vec!["wss://relay.example.com".to_string()],
            ..Default::default()
        };
        let channel = NostrChannel::new("nostr-test", config);
        assert_eq!(channel.info().id.as_str(), "nostr-test");
        assert_eq!(channel.info().channel_type, "nostr");
        assert_eq!(channel.info().name, "Nostr");
        // Public key should be derived from private key
        assert!(!channel.own_pubkey.is_empty());
        assert_eq!(channel.own_pubkey.len(), 64);
    }

    #[test]
    fn test_channel_initial_status() {
        let config = NostrConfig::default();
        let channel = NostrChannel::new("nostr", config);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[test]
    fn test_take_receiver() {
        let config = NostrConfig::default();
        let channel = NostrChannel::new("nostr", config);

        // First take should succeed
        assert!(channel.state().take_receiver().is_some());

        // Second take should return None
        assert!(channel.state().take_receiver().is_none());
    }

    #[tokio::test]
    async fn test_factory_create_valid() {
        let factory = NostrChannelFactory;
        assert_eq!(factory.channel_type(), "nostr");

        let config = serde_json::json!({
            "private_key": TEST_PRIVKEY,
            "relays": ["wss://relay.damus.io"]
        });

        let result = factory.create(config).await;
        assert!(result.is_ok());

        let channel = result.unwrap();
        assert_eq!(channel.info().channel_type, "nostr");
    }

    #[tokio::test]
    async fn test_factory_create_invalid_config() {
        let factory = NostrChannelFactory;

        // Missing required fields
        let config = serde_json::json!({});
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_invalid_relay_url() {
        let factory = NostrChannelFactory;

        let config = serde_json::json!({
            "private_key": TEST_PRIVKEY,
            "relays": ["https://not-a-websocket"]
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_factory_create_no_relays() {
        let factory = NostrChannelFactory;

        let config = serde_json::json!({
            "private_key": TEST_PRIVKEY,
            "relays": []
        });
        let result = factory.create(config).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_channel_creation_with_invalid_key() {
        // With an invalid private key, pubkey derivation fails gracefully
        let config = NostrConfig {
            private_key: "invalid".to_string(),
            relays: vec!["wss://relay.example.com".to_string()],
            ..Default::default()
        };
        let channel = NostrChannel::new("nostr-test", config);
        // Pubkey should be empty due to derivation failure
        assert!(channel.own_pubkey.is_empty());
    }
}
