//! BlueBubbles iMessage transport (REST + webhook).
//!
//! Pure HTTP — compiles and runs on any OS, unlike the macOS-only local path.

pub mod api;
pub mod config;
pub mod inbound;
pub mod outbound;

pub use config::BlueBubblesConfig;

use crate::sync_primitives::{Arc, AtomicBool, Mutex as StdMutex, Ordering};

use async_trait::async_trait;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelId, ChannelInfo, ChannelResult,
    ChannelState, ChannelStatus, MessageId, OutboundMessage, SendResult,
};

use api::{BlueBubblesApi, LruGuidCache, ServerCaps};
use inbound::dedup::BbDedup;

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
    #[allow(dead_code)] // consumed in Task 12 (catch-up poll)
    offset_tracker: Option<
        Arc<crate::gateway::interfaces::telegram::offset::OffsetTracker>,
    >,
    api: BlueBubblesApi,
    guid_cache: Arc<tokio::sync::Mutex<LruGuidCache>>,
    server_caps: Arc<tokio::sync::RwLock<ServerCaps>>,
    running: Arc<AtomicBool>,
    dedup: Arc<StdMutex<BbDedup>>,
    webhook_handle: Option<tokio::task::JoinHandle<()>>,
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
            running: Arc::new(AtomicBool::new(false)),
            dedup: Arc::new(StdMutex::new(BbDedup::new())),
            webhook_handle: None,
        }
    }

    pub fn set_offset_tracker(
        &mut self,
        tracker: Arc<crate::gateway::interfaces::telegram::offset::OffsetTracker>,
    ) {
        self.offset_tracker = Some(tracker);
    }
}

/// Compile mention wake-word patterns; skip and warn on invalid regex.
fn compile_patterns(raw: &[String]) -> Vec<regex::Regex> {
    raw.iter()
        .filter_map(|p| match regex::Regex::new(p) {
            Ok(re) => Some(re),
            Err(e) => {
                tracing::warn!("invalid mention pattern {:?}: {e}", p);
                None
            }
        })
        .collect()
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
        self.channel_state.set_status(ChannelStatus::Connecting).await;

        if self.api.ping().await.is_err() {
            self.channel_state.set_status(ChannelStatus::Error).await;
            return Err(ChannelError::NotConnected("BlueBubbles ping failed".into()));
        }
        *self.server_caps.write().await = self.api.server_caps().await;

        let patterns = compile_patterns(&self.config.mention_patterns);
        let state = inbound::webhook_server::WebhookState {
            password: self.config.password.clone(),
            require_mention: self.config.require_mention,
            mention_patterns: Arc::new(patterns),
            sender: self.channel_state.sender(),
            api: Arc::new(self.api.clone()),
            dedup: self.dedup.clone(),
        };
        let (host, port, path) = (
            self.config.webhook_host.clone(),
            self.config.webhook_port,
            self.config.webhook_path.clone(),
        );
        self.webhook_handle =
            Some(tokio::spawn(inbound::webhook_server::run_webhook_server(state, host, port, path)));

        let cb = api::webhook_callback_url(
            &self.config.webhook_host,
            self.config.webhook_port,
            &self.config.webhook_path,
            &self.config.password,
        );
        self.api.register_webhook(&cb).await;

        self.running.store(true, Ordering::SeqCst);
        self.channel_state.set_status(ChannelStatus::Connected).await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        let cb = api::webhook_callback_url(
            &self.config.webhook_host,
            self.config.webhook_port,
            &self.config.webhook_path,
            &self.config.password,
        );
        self.api.unregister_matching(&cb).await;
        if let Some(h) = self.webhook_handle.take() {
            h.abort();
        }
        self.running.store(false, Ordering::SeqCst);
        self.channel_state.set_status(ChannelStatus::Disconnected).await;
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
        for attachment in &message.attachments {
            if let Some(path) = &attachment.path {
                let is_audio = attachment.mime_type.starts_with("audio/");
                last = self
                    .api
                    .send_attachment(&guid, std::path::Path::new(path), is_audio)
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
