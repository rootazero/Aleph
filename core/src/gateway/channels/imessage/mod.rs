//! iMessage Channel Implementation
//!
//! Provides iMessage integration for the Gateway using:
//! - SQLite database polling for receiving messages
//! - AppleScript for sending messages
//!
//! # Requirements
//!
//! - macOS only
//! - Full Disk Access permission (to read chat.db)
//! - Automation permission (to control Messages.app)
//!
//! # Configuration
//!
//! ```toml
//! [channels.imessage]
//! enabled = true
//! db_path = "~/Library/Messages/chat.db"
//! poll_interval_ms = 1000
//! ```

mod db;
mod sender;
mod target;
pub mod config;
pub mod message_ops;

pub use db::MessagesDb;
pub use sender::MessageSender;
pub use target::{IMessageTarget, Service, parse_target, normalize_phone};
pub use config::{IMessageConfig, DmPolicy as IMessageDmPolicy, GroupPolicy as IMessageGroupPolicy};
pub use message_ops::IMessageMessageOps;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelStatus, InboundMessage, MessageId,
    OutboundMessage, SendResult,
};

/// iMessage channel implementation
pub struct IMessageChannel {
    info: ChannelInfo,
    config: IMessageConfig,
    db: Arc<Mutex<Option<MessagesDb>>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
    running: Arc<AtomicBool>,
    poll_handle: Option<tokio::task::JoinHandle<()>>,
}

impl IMessageChannel {
    /// Create a new iMessage channel
    pub fn new(config: IMessageConfig) -> Self {
        let (tx, _rx) = mpsc::channel(100);

        let info = ChannelInfo {
            id: ChannelId::new("imessage"),
            name: "iMessage".to_string(),
            channel_type: "imessage".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: ChannelCapabilities {
                attachments: true,
                images: true,
                audio: true,
                video: true,
                reactions: true,
                replies: false, // iMessage supports tapbacks but not threading
                editing: false,
                deletion: false,
                typing_indicator: false, // Would need more complex integration
                read_receipts: false,
                rich_text: false,
                max_message_length: 20000, // Approximate limit
                max_attachment_size: 100 * 1024 * 1024, // 100 MB
            },
        };

        Self {
            info,
            config,
            db: Arc::new(Mutex::new(None)),
            inbound_tx: tx,
            running: Arc::new(AtomicBool::new(false)),
            poll_handle: None,
        }
    }

    /// Start the message polling loop
    async fn start_polling(&mut self) -> ChannelResult<()> {
        let db_path = self.config.db_path();

        // Open database
        let db = MessagesDb::open(&db_path).map_err(|e| {
            ChannelError::ConfigError(format!("Failed to open Messages database: {}", e))
        })?;

        {
            let mut db_lock = self.db.lock().await;
            *db_lock = Some(db);
        }

        self.running.store(true, Ordering::SeqCst);

        // Clone what we need for the polling task
        let db = self.db.clone();
        let tx = self.inbound_tx.clone();
        let running = self.running.clone();
        let poll_interval = Duration::from_millis(self.config.poll_interval_ms);

        let handle = tokio::spawn(async move {
            info!("iMessage polling started");

            while running.load(Ordering::SeqCst) {
                // Poll for new messages
                let messages = {
                    let mut db_lock = db.lock().await;
                    if let Some(ref mut db) = *db_lock {
                        match db.poll_new_messages() {
                            Ok(msgs) => msgs,
                            Err(e) => {
                                error!("Failed to poll messages: {}", e);
                                vec![]
                            }
                        }
                    } else {
                        vec![]
                    }
                };

                // Send messages to channel
                for msg in messages {
                    debug!("Received iMessage: {:?}", msg.text);
                    if tx.send(msg).await.is_err() {
                        warn!("Failed to send message to channel receiver");
                        break;
                    }
                }

                tokio::time::sleep(poll_interval).await;
            }

            info!("iMessage polling stopped");
        });

        self.poll_handle = Some(handle);
        Ok(())
    }
}

#[async_trait]
impl Channel for IMessageChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Starting iMessage channel");
        self.info.status = ChannelStatus::Connecting;

        // Start polling
        self.start_polling().await?;

        self.info.status = ChannelStatus::Connected;
        info!("iMessage channel started");
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        if !self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Stopping iMessage channel");
        self.running.store(false, Ordering::SeqCst);

        // Wait for polling task to finish
        if let Some(handle) = self.poll_handle.take() {
            let _ = handle.await;
        }

        // Close database
        {
            let mut db_lock = self.db.lock().await;
            *db_lock = None;
        }

        self.info.status = ChannelStatus::Disconnected;
        info!("iMessage channel stopped");
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let target = message.conversation_id.as_str();

        // Send text
        if !message.text.is_empty() {
            MessageSender::send_text(target, &message.text)
                .await
                .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        }

        // Send attachments
        for attachment in &message.attachments {
            if let Some(path) = &attachment.path {
                MessageSender::send_file(target, std::path::Path::new(path))
                    .await
                    .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
            }
        }

        Ok(SendResult {
            message_id: MessageId::new(uuid::Uuid::new_v4().to_string()),
            timestamp: chrono::Utc::now(),
        })
    }

    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        // This is a bit hacky - we can only call this once
        // In practice, the channel manager should take ownership
        None // The receiver is taken during construction
    }
}

/// Factory for creating iMessage channels
pub struct IMessageChannelFactory;

impl IMessageChannelFactory {
    /// Create a new factory
    pub fn new() -> Self {
        Self
    }
}

impl Default for IMessageChannelFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelFactory for IMessageChannelFactory {
    fn channel_type(&self) -> &str {
        "imessage"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: IMessageConfig = serde_json::from_value(config).map_err(|e| {
            ChannelError::ConfigError(format!("Invalid iMessage config: {}", e))
        })?;

        if !config.enabled {
            return Err(ChannelError::ConfigError("iMessage channel is disabled".to_string()));
        }

        Ok(Box::new(IMessageChannel::new(config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_info() {
        let config = IMessageConfig::default();
        let channel = IMessageChannel::new(config);

        assert_eq!(channel.info().id.as_str(), "imessage");
        assert_eq!(channel.info().channel_type, "imessage");
        assert!(channel.capabilities().attachments);
    }
}
