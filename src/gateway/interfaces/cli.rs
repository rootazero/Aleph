//! CLI Channel Implementation
//!
//! A simple command-line interface channel for testing and local interaction.
//! Messages are read from stdin and written to stdout.
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::gateway::interfaces::CliChannel;
//!
//! let channel = CliChannel::new("cli".to_string());
//! channel.start().await?;
//! ```

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};
use uuid::Uuid;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, InboundMessage, MessageId,
    OutboundMessage, SendResult, UserId,
};

/// CLI channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliChannelConfig {
    /// Channel ID (defaults to "cli")
    #[serde(default = "default_cli_id")]
    pub id: String,
    /// Prompt to display before user input
    #[serde(default = "default_prompt")]
    pub prompt: String,
    /// User name for messages
    #[serde(default = "default_username")]
    pub username: String,
    /// Whether to echo sent messages
    #[serde(default)]
    pub echo_sent: bool,
}

fn default_cli_id() -> String {
    "cli".to_string()
}

fn default_prompt() -> String {
    "> ".to_string()
}

fn default_username() -> String {
    "user".to_string()
}

impl Default for CliChannelConfig {
    fn default() -> Self {
        Self {
            id: default_cli_id(),
            prompt: default_prompt(),
            username: default_username(),
            echo_sent: false,
        }
    }
}

/// CLI channel state
struct CliChannelState {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// CLI channel implementation
pub struct CliChannel {
    info: ChannelInfo,
    config: CliChannelConfig,
    cli_state: Arc<RwLock<CliChannelState>>,
    channel_state: ChannelState,
    /// Test mode: skip stdin thread, allow `inject_message` without I/O
    test_mode: bool,
}

impl CliChannel {
    /// Create a new CLI channel with default configuration
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let config = CliChannelConfig {
            id: id.clone(),
            ..Default::default()
        };
        Self::with_config(config)
    }

    /// Create a new CLI channel with custom configuration
    #[must_use]
    pub fn with_config(config: CliChannelConfig) -> Self {
        Self::with_config_and_mode(config, false)
    }

    /// Create a new CLI channel with custom configuration and test mode.
    ///
    /// In test mode, the channel skips spawning the stdin reader thread
    /// and allows `inject_message` to work without blocking on I/O.
    #[must_use]
    pub fn with_config_and_mode(config: CliChannelConfig, test_mode: bool) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(&config.id),
            name: format!("CLI Channel ({})", config.id),
            channel_type: "cli".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: ChannelCapabilities {
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
                rich_text: false,
                max_message_length: 0, // unlimited
                max_attachment_size: 0,
                stream_protocol: Default::default(),
            },
        };

        let cli_state = CliChannelState { shutdown_tx: None };

        Self {
            info,
            config,
            cli_state: Arc::new(RwLock::new(cli_state)),
            channel_state: ChannelState::new(100),
            test_mode,
        }
    }

    /// Create a CLI channel for testing (test mode enabled).
    pub fn for_test(id: impl Into<String>) -> Self {
        let config = CliChannelConfig {
            id: id.into(),
            ..Default::default()
        };
        Self::with_config_and_mode(config, true)
    }

    /// Create a test message (useful for testing)
    pub async fn inject_message(&self, text: impl Into<String>) -> ChannelResult<()> {
        let tx = self.channel_state.sender();
        let message = InboundMessage {
            id: MessageId::new(Uuid::new_v4().to_string()),
            channel_id: self.info.id.clone(),
            conversation_id: ConversationId::new("cli:main"),
            sender_id: UserId::new(&self.config.username),
            sender_name: Some(self.config.username.clone()),
            text: text.into(),
            attachments: Vec::new(),
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };

        tx.send(message)
            .map_err(|e| ChannelError::Internal(format!("Failed to inject message: {e:?}")))?;
        Ok(())
    }
}

#[async_trait]
impl Channel for CliChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if self.channel_state.status() == ChannelStatus::Connected {
            return Ok(());
        }

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;

        // In test mode, skip spawning the stdin reader thread
        if self.test_mode {
            self.channel_state
                .set_status(ChannelStatus::Connected)
                .await;
            info!("CLI channel started in test mode: {}", self.info.id);
            return Ok(());
        }

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

        // Clone sender from channel_state for the spawned task
        let inbound_tx = self.channel_state.sender();
        let config = self.config.clone();

        // Create a channel for lines from the blocking reader thread
        let (line_tx, mut line_rx) = mpsc::channel::<String>(10);

        // Spawn blocking reader thread for stdin
        std::thread::spawn(move || {
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(text) => {
                        if line_tx.blocking_send(text).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Spawn async task to process lines
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        debug!("CLI channel shutting down");
                        break;
                    }
                    Some(text) = line_rx.recv() => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }

                        let message = InboundMessage {
                            id: MessageId::new(Uuid::new_v4().to_string()),
                            channel_id: ChannelId::new(&config.id),
                            conversation_id: ConversationId::new("cli:main"),
                            sender_id: UserId::new(&config.username),
                            sender_name: Some(config.username.clone()),
                            text,
                            attachments: Vec::new(),
                            timestamp: Utc::now(),
                            reply_to: None,
                            is_group: false,
                            raw: None,
                            metadata: vec![],
                        };

                        if inbound_tx.send(message).is_err() {
                            debug!("CLI channel receiver dropped");
                            break;
                        }
                    }
                }
            }
        });

        let mut cli_state = self.cli_state.write().await;
        cli_state.shutdown_tx = Some(shutdown_tx);
        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;

        info!("CLI channel started: {}", self.info.id);
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        let mut cli_state = self.cli_state.write().await;

        if let Some(shutdown_tx) = cli_state.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        drop(cli_state);

        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;

        info!("CLI channel stopped: {}", self.info.id);
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.channel_state.status() != ChannelStatus::Connected {
            return Err(ChannelError::NotConnected(
                "CLI channel not connected".to_string(),
            ));
        }

        // In test mode, skip stdout I/O
        if !self.test_mode {
            let mut stdout = io::stdout().lock();
            writeln!(stdout, "\n{}", message.text)
                .map_err(|e| ChannelError::SendFailed(format!("Failed to write to stdout: {e}")))?;
            stdout
                .flush()
                .map_err(|e| ChannelError::SendFailed(format!("Failed to flush stdout: {e}")))?;

            print!("{}", self.config.prompt);
            io::stdout().flush().ok();
        }

        let message_id = MessageId::new(Uuid::new_v4().to_string());
        Ok(SendResult {
            message_id,
            timestamp: Utc::now(),
        })
    }
}

/// Factory for creating CLI channels
pub struct CliChannelFactory;

#[async_trait]
impl ChannelFactory for CliChannelFactory {
    fn channel_type(&self) -> &str {
        "cli"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: CliChannelConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid CLI channel config: {e}")))?;

        Ok(Box::new(CliChannel::with_config(config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cli_channel_creation() {
        let channel = CliChannel::new("test-cli");
        assert_eq!(channel.id().as_str(), "test-cli");
        assert_eq!(channel.channel_type(), "cli");
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[tokio::test]
    async fn test_cli_channel_config() {
        let config = CliChannelConfig {
            id: "custom-cli".to_string(),
            prompt: ">>> ".to_string(),
            username: "alice".to_string(),
            echo_sent: true,
        };

        let channel = CliChannel::with_config(config);
        assert_eq!(channel.id().as_str(), "custom-cli");
    }

    #[tokio::test]
    async fn test_cli_channel_capabilities() {
        let channel = CliChannel::new("cli");
        let caps = channel.capabilities();

        assert!(!caps.attachments);
        assert!(!caps.reactions);
        assert!(!caps.rich_text);
        assert_eq!(caps.max_message_length, 0);
    }

    #[tokio::test]
    async fn test_cli_factory() {
        let factory = CliChannelFactory;
        assert_eq!(factory.channel_type(), "cli");

        let config = serde_json::json!({
            "id": "factory-cli",
            "prompt": "$ "
        });

        let channel = factory.create(config).await.unwrap();
        assert_eq!(channel.id().as_str(), "factory-cli");
    }

    #[tokio::test]
    async fn test_cli_test_mode_start_stop() {
        let mut channel = CliChannel::for_test("test-cli");
        assert_eq!(channel.status(), ChannelStatus::Disconnected);

        channel.start().await.unwrap();
        assert_eq!(channel.status(), ChannelStatus::Connected);

        channel.stop().await.unwrap();
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[tokio::test]
    async fn test_cli_test_mode_send() {
        let mut channel = CliChannel::for_test("test-cli");
        channel.start().await.unwrap();

        let msg = OutboundMessage::text("cli:main", "Hello test");
        let result = channel.send(msg).await;
        assert!(result.is_ok());

        channel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_cli_test_mode_inject_and_receive() {
        let mut channel = CliChannel::for_test("test-cli");
        channel.start().await.unwrap();

        let mut rx = channel.state().take_receiver().unwrap();

        channel.inject_message("Injected message").await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.text, "Injected message");
        assert_eq!(received.conversation_id.as_str(), "cli:main");

        channel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_cli_send_without_start() {
        let channel = CliChannel::for_test("test-cli");
        let msg = OutboundMessage::text("cli:main", "Hello");
        let result = channel.send(msg).await;
        assert!(matches!(result, Err(ChannelError::NotConnected(_))));
    }
}
