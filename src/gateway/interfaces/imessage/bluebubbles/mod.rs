//! BlueBubbles iMessage transport (REST + webhook).
//!
//! Pure HTTP — compiles and runs on any OS, unlike the macOS-only local path.

pub mod config;

pub use config::BlueBubblesConfig;

use async_trait::async_trait;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelId, ChannelInfo, ChannelResult,
    ChannelState, ChannelStatus, OutboundMessage, SendResult,
};

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
    config: BlueBubblesConfig,
    channel_state: ChannelState,
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
        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
        }
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

    async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
        Err(ChannelError::UnsupportedFeature(
            "BlueBubbles send not yet implemented".to_string(),
        ))
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
