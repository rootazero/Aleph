//! BlueBubbles iMessage transport (REST + webhook).
//!
//! Pure HTTP — compiles and runs on any OS, unlike the macOS-only local path.

pub mod api;
pub mod config;
pub mod outbound;

pub use config::BlueBubblesConfig;

use std::sync::Arc;

use async_trait::async_trait;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelId, ChannelInfo, ChannelResult,
    ChannelState, ChannelStatus, MessageId, OutboundMessage, SendResult,
};

use api::{BlueBubblesApi, LruGuidCache, ServerCaps};

/// Honest capabilities for the BlueBubbles transport.
#[must_use]
pub fn bluebubbles_capabilities() -> ChannelCapabilities {
    ChannelCapabilities {
        attachments: true,
        images: true,
        audio: true,
        video: true,
        reactions: true,
        replies: true,
        editing: false,
        deletion: false,
        typing_indicator: true,
        read_receipts: true,
        rich_text: false,
        max_message_length: 4000,
        max_attachment_size: 100 * 1024 * 1024,
        stream_protocol: Default::default(),
    }
}

/// iMessage channel backed by a BlueBubbles server.
pub struct BlueBubblesChannel {
    info: ChannelInfo,
    #[allow(dead_code)] // self.config not read until Task 11 (webhook lifecycle)
    config: BlueBubblesConfig,
    channel_state: ChannelState,
    #[allow(dead_code)] // consumed in Task 12 (catch-up poll)
    offset_tracker: Option<
        Arc<crate::gateway::interfaces::telegram::offset::OffsetTracker>,
    >,
    api: BlueBubblesApi,
    guid_cache: Arc<tokio::sync::Mutex<LruGuidCache>>,
    server_caps: Arc<tokio::sync::RwLock<ServerCaps>>,
}

impl BlueBubblesChannel {
    #[must_use]
    pub fn new(config: BlueBubblesConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new("imessage"),
            name: "iMessage (BlueBubbles)".to_string(),
            channel_type: "imessage".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: bluebubbles_capabilities(),
        };
        let api = BlueBubblesApi::new(config.server_url.clone(), config.password.clone());
        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            offset_tracker: None,
            api,
            guid_cache: Arc::new(tokio::sync::Mutex::new(LruGuidCache::new(500))),
            server_caps: Arc::new(tokio::sync::RwLock::new(ServerCaps::default())),
        }
    }

    pub fn set_offset_tracker(
        &mut self,
        tracker: Arc<crate::gateway::interfaces::telegram::offset::OffsetTracker>,
    ) {
        self.offset_tracker = Some(tracker);
    }
}

#[async_trait]
impl Channel for BlueBubblesChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        // Filled in Tasks 10-12 (webhook + poll). Skeleton marks connected.
        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let target = message.conversation_id.as_str();
        let guid = self
            .api
            .resolve_chat_guid(target, &self.guid_cache)
            .await
            .ok_or_else(|| ChannelError::SendFailed(format!("chat not found: {target}")))?;
        let private_api = self.server_caps.read().await.private_api;
        let reply = message.reply_to.as_ref().map(|m| m.as_str());
        let mut last = String::from("ok");
        if !message.text.is_empty() {
            for chunk in outbound::text::split_into_bubbles(&message.text, 4000) {
                last = self
                    .api
                    .send_text_chunk(&guid, &chunk, reply, private_api)
                    .await
                    .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
            }
        }
        Ok(SendResult { message_id: MessageId::new(last), timestamp: chrono::Utc::now() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BlueBubblesConfig {
        BlueBubblesConfig {
            server_url: "http://localhost:1234".into(),
            password: "pw".into(),
            webhook_host: "127.0.0.1".into(),
            webhook_port: 8645,
            webhook_path: "/bluebubbles-webhook".into(),
            poll_interval_secs: 30,
            send_read_receipts: true,
            require_mention: false,
            mention_patterns: vec![],
        }
    }

    #[test]
    fn reports_imessage_channel_type_and_honest_caps() {
        let ch = BlueBubblesChannel::new(cfg());
        assert_eq!(ch.info().id.as_str(), "imessage");
        assert_eq!(ch.info().channel_type, "imessage");
        let caps = &ch.info().capabilities;
        assert!(caps.reactions, "BlueBubbles supports tapbacks");
        assert!(caps.replies);
        assert!(caps.typing_indicator);
        assert!(caps.read_receipts);
    }
}
