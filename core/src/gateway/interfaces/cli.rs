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

use std::io::{self, BufRead, Write};
use std::sync::Arc;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};
use uuid::Uuid;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelProvider,
    ChannelId, ChannelInfo, ChannelResult, ChannelStatus, ConversationId, InboundMessage,
    MessageId, OutboundMessage, SendResult, UserId,
};
use crate::thinker::interaction::{InteractionConstraints, InteractionManifest, InteractionParadigm};

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
    status: ChannelStatus,
    inbound_tx: Option<mpsc::Sender<InboundMessage>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// CLI channel implementation
pub struct CliChannel {
    info: ChannelInfo,
    config: CliChannelConfig,
    state: Arc<RwLock<CliChannelState>>,
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
    pub fn with_config(config: CliChannelConfig) -> Self {
        let (inbound_tx, _inbound_rx) = mpsc::channel(100);

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
            },
        };

        let state = CliChannelState {
            status: ChannelStatus::Disconnected,
            inbound_tx: Some(inbound_tx),
            shutdown_tx: None,
        };

        Self {
            info,
            config,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// Create a test message (useful for testing)
    pub async fn inject_message(&self, text: impl Into<String>) -> ChannelResult<()> {
        let state = self.state.read().await;
        if let Some(tx) = &state.inbound_tx {
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
            };

            tx.send(message)
                .await
                .map_err(|e| ChannelError::Internal(format!("Failed to inject message: {}", e)))?;
        }
        Ok(())
    }
}

#[async_trait]
impl Channel for CliChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    async fn start(&mut self) -> ChannelResult<()> {
        let mut state = self.state.write().await;

        if state.status == ChannelStatus::Connected {
            return Ok(());
        }

        state.status = ChannelStatus::Connecting;
        drop(state);

        // Update info status
        self.info.status = ChannelStatus::Connecting;

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

        // Get inbound sender
        let state_clone = self.state.clone();
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

                        let state = state_clone.read().await;
                        if let Some(tx) = &state.inbound_tx {
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
                            };

                            if tx.send(message).await.is_err() {
                                debug!("CLI channel receiver dropped");
                                break;
                            }
                        }
                    }
                }
            }
        });

        let mut state = self.state.write().await;
        state.status = ChannelStatus::Connected;
        state.shutdown_tx = Some(shutdown_tx);
        self.info.status = ChannelStatus::Connected;

        info!("CLI channel started: {}", self.info.id);
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        let mut state = self.state.write().await;

        if let Some(shutdown_tx) = state.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        state.status = ChannelStatus::Disconnected;
        self.info.status = ChannelStatus::Disconnected;

        info!("CLI channel stopped: {}", self.info.id);
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let state = self.state.read().await;
        if state.status != ChannelStatus::Connected {
            return Err(ChannelError::NotConnected("CLI channel not connected".to_string()));
        }
        drop(state);

        // Write to stdout
        let mut stdout = io::stdout().lock();
        writeln!(stdout, "\n{}", message.text)
            .map_err(|e| ChannelError::SendFailed(format!("Failed to write to stdout: {}", e)))?;
        stdout.flush()
            .map_err(|e| ChannelError::SendFailed(format!("Failed to flush stdout: {}", e)))?;

        // Print prompt for next input
        print!("{}", self.config.prompt);
        io::stdout().flush().ok();

        let message_id = MessageId::new(Uuid::new_v4().to_string());
        Ok(SendResult {
            message_id,
            timestamp: Utc::now(),
        })
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        // This is a bit tricky - we need to return the receiver only once
        // For now, return None as the registry will handle forwarding
        None
    }
}

impl ChannelProvider for CliChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest::new(InteractionParadigm::CLI)
            .with_constraints(InteractionConstraints {
                max_output_chars: None,  // CLI has no limit
                supports_streaming: true,
                prefer_compact: false,
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
            .map_err(|e| ChannelError::ConfigError(format!("Invalid CLI channel config: {}", e)))?;

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
}
