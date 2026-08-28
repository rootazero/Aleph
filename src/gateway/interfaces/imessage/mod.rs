//! iMessage Channel Implementation
//!
//! Provides iMessage integration for the Gateway using two transports:
//!
//! - **Local** (`IMessageChannel`): SQLite database polling for receiving +
//!   AppleScript for sending. Requires macOS, Full Disk Access, and
//!   Automation permission — only *registered* on macOS.
//! - **BlueBubbles** (`BlueBubblesChannel`): REST + webhook against a running
//!   BlueBubbles server. Pure HTTP — compiles and runs on any OS.
//!
//! # Configuration
//!
//! ```toml
//! [channels.imessage]
//! enabled = true
//! db_path = "~/Library/Messages/chat.db"
//! poll_interval_ms = 1000
//! ```

pub mod bluebubbles;
pub mod config;
mod db;
pub mod reaction;
mod sender;
mod target;

pub use bluebubbles::{BlueBubblesChannel, BlueBubblesConfig};
pub use config::{
    DmPolicy as IMessageDmPolicy, GroupPolicy as IMessageGroupPolicy, IMessageConfig,
};
pub use db::MessagesDb;
pub use sender::MessageSender;
pub use target::{normalize_phone, parse_target, IMessageTarget, Service};

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, Ordering};
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, MessageId, OutboundMessage, SendResult,
};
// Reuse the gateway-shared, SQLite-backed monotonic cursor (currently homed in
// the telegram module) to persist the catch-up watermark across restarts.
use crate::gateway::interfaces::telegram::offset::OffsetTracker;

/// iMessage channel implementation
pub struct IMessageChannel {
    info: ChannelInfo,
    config: IMessageConfig,
    db: Arc<Mutex<Option<MessagesDb>>>,
    running: Arc<AtomicBool>,
    poll_handle: Option<tokio::task::JoinHandle<()>>,
    channel_state: ChannelState,
    test_mode: bool,
    /// Persistent catch-up cursor (last processed message ROWID). When wired,
    /// the daemon resumes from the last persisted ROWID after a restart instead
    /// of skipping straight to the newest message, recovering messages that
    /// arrived while it was offline.
    offset_tracker: Option<Arc<OffsetTracker>>,
}

impl IMessageChannel {
    /// Create a new iMessage channel
    #[must_use]
    pub fn new(config: IMessageConfig) -> Self {
        Self::with_mode(config, false)
    }

    fn with_mode(config: IMessageConfig, test_mode: bool) -> Self {
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
                reactions: false, // AppleScript cannot send tapbacks
                replies: false,
                editing: false,
                deletion: false,
                typing_indicator: false,
                read_receipts: false,
                rich_text: false,
                max_message_length: 20000,              // Approximate limit
                max_attachment_size: 100 * 1024 * 1024, // 100 MB
                stream_protocol: Default::default(),
            },
        };

        Self {
            info,
            config,
            db: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
            poll_handle: None,
            channel_state: ChannelState::new(100),
            test_mode,
            offset_tracker: None,
        }
    }

    #[must_use]
    pub fn for_test(config: IMessageConfig) -> Self {
        Self::with_mode(config, true)
    }

    /// Wire a persistent catch-up cursor so the channel resumes from the last
    /// processed message ROWID after a restart (recovering offline messages)
    /// instead of skipping to the newest message.
    pub fn set_offset_tracker(&mut self, tracker: Arc<OffsetTracker>) {
        self.offset_tracker = Some(tracker);
    }

    /// Start the message polling loop
    async fn start_polling(&mut self) -> ChannelResult<()> {
        let db_path = self.config.db_path();

        // Open database
        let mut db = MessagesDb::open(&db_path).map_err(|e| {
            ChannelError::ConfigError(format!("Failed to open Messages database: {e}"))
        })?;

        // Apply attachment handling policy from config.
        db.set_attachment_policy(
            self.config.include_attachments,
            self.config.max_attachment_size,
        );

        // Catch-up: if a watermark was persisted from a previous run, resume
        // from it so messages received while offline are recovered. On first
        // run (no persisted cursor) keep the default of skipping to the newest
        // message to avoid replaying the entire history.
        if let Some(ref tracker) = self.offset_tracker {
            let persisted = tracker.load();
            if persisted > 0 {
                info!(rowid = persisted, "iMessage resuming from persisted cursor");
                db.resume_from_rowid(persisted);
            }
        }

        {
            let mut db_lock = self.db.lock().await;
            *db_lock = Some(db);
        }

        self.running.store(true, Ordering::SeqCst);

        // Clone what we need for the polling task
        let db = self.db.clone();
        let tx = self.channel_state.sender();
        let running = self.running.clone();
        let poll_interval = Duration::from_millis(self.config.poll_interval_ms);
        let offset_tracker = self.offset_tracker.clone();

        let handle = tokio::spawn(async move {
            info!("iMessage polling started");

            while running.load(Ordering::SeqCst) {
                // Poll for new messages, capturing the watermark to persist.
                let (messages, watermark) = {
                    let mut db_lock = db.lock().await;
                    if let Some(ref mut db) = *db_lock {
                        match db.poll_new_messages() {
                            Ok(msgs) => (msgs, db.last_rowid()),
                            Err(e) => {
                                error!("Failed to poll messages: {}", e);
                                (vec![], db.last_rowid())
                            }
                        }
                    } else {
                        (vec![], 0)
                    }
                };

                // Send messages to channel
                for msg in messages {
                    debug!("Received iMessage: {:?}", msg.text);
                    if tx.send(msg).is_err() {
                        warn!("Failed to send message to channel receiver");
                        break;
                    }
                }

                // Persist the catch-up watermark so a restart resumes here.
                // `advance` is monotonic, so a stale value is a no-op.
                if let Some(ref tracker) = offset_tracker {
                    if watermark > 0 {
                        tracker.advance(watermark, "imessage").await;
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

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if self.test_mode {
            self.channel_state
                .set_status(ChannelStatus::Connected)
                .await;
            info!("iMessage channel started in test mode");
            return Ok(());
        }

        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        if !self.config.enabled {
            info!("iMessage channel is disabled, skipping start");
            return Ok(());
        }

        info!("Starting iMessage channel");
        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;

        // Start polling
        self.start_polling().await?;

        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;
        info!("iMessage channel started");
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
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

        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        info!("iMessage channel stopped");
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.test_mode {
            return Ok(SendResult {
                message_id: MessageId::new(uuid::Uuid::new_v4().to_string()),
                timestamp: chrono::Utc::now(),
            });
        }

        if !self.running.load(Ordering::SeqCst) {
            return Err(ChannelError::NotConnected(
                "iMessage channel not started".to_string(),
            ));
        }

        let target = message.conversation_id.as_str();

        // Route by parsed target: chat_id:/chat_guid: prefixed targets use
        // chat-id send (group chat); phone/email/bare identifiers fall through
        // to participant send_text (existing behaviour).
        // Note: inbound group messages carry a bare chat_identifier (no prefix),
        // so they still route to send_text — fully fixing AppleScript group
        // replies requires macOS-runtime chat-GUID resolution (out of scope;
        // BlueBubbles transport handles groups robustly).
        if !message.text.is_empty() {
            // Re-check `running` immediately before the AppleScript call: a
            // `stop()` landing between the gate above and the script
            // invocation would otherwise fire one stale outbound message per
            // restart window (the script is a real side effect — it texts a
            // human). Same rationale applies to the attachment loop below.
            if !self.running.load(Ordering::SeqCst) {
                return Err(ChannelError::NotConnected(
                    "iMessage channel stopped before send".to_string(),
                ));
            }
            match parse_target(target) {
                Ok(IMessageTarget::ChatId { id }) => {
                    MessageSender::send_to_chat(&format!("chat_id:{id}"), &message.text)
                        .await
                        .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
                }
                Ok(IMessageTarget::ChatGuid { guid }) => {
                    MessageSender::send_to_chat(&guid, &message.text)
                        .await
                        .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
                }
                // Phone / Email / parse error: participant send (existing behavior)
                _ => {
                    MessageSender::send_text(target, &message.text)
                        .await
                        .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
                }
            }
        }

        // Send attachments
        for attachment in &message.attachments {
            if let Some(path) = &attachment.path {
                if !self.running.load(Ordering::SeqCst) {
                    return Err(ChannelError::NotConnected(
                        "iMessage channel stopped before attachment send".to_string(),
                    ));
                }
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
}

/// Factory for creating iMessage channels
pub struct IMessageChannelFactory;

impl IMessageChannelFactory {
    /// Create a new factory
    #[must_use]
    pub const fn new() -> Self {
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
        let config: IMessageConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid iMessage config: {e}")))?;

        if !config.enabled {
            return Err(ChannelError::ConfigError(
                "iMessage channel is disabled".to_string(),
            ));
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

    #[test]
    fn local_capabilities_are_honest() {
        let channel = IMessageChannel::new(IMessageConfig::default());
        let caps = &channel.info().capabilities;
        assert!(!caps.reactions, "AppleScript cannot send tapbacks");
        assert!(!caps.replies);
        assert!(!caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(caps.attachments);
    }

    #[test]
    fn group_chat_target_routes_to_chat_send() {
        // chat_id: prefix and chat_guid: must parse to chat targets, which the
        // send path must route via send_to_chat (not participant send_text).
        assert!(matches!(
            parse_target("chat_id:42").unwrap(),
            IMessageTarget::ChatId { .. }
        ));
        assert!(matches!(
            parse_target("chat_guid:iMessage;+;chat123").unwrap(),
            IMessageTarget::ChatGuid { .. }
        ));
        // a plain phone stays a Phone target (participant send_text)
        assert!(matches!(
            parse_target("+15551234567").unwrap(),
            IMessageTarget::Phone { .. }
        ));
    }
}
