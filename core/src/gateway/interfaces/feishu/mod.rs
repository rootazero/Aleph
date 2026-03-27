pub mod config;
pub mod types;
pub mod events;
pub mod auth;
pub mod api;
pub mod streaming;
pub mod dedup;
pub mod user_cache;
pub mod websocket;

use std::sync::Arc;
use tokio::sync::watch;
use async_trait::async_trait;
use chrono::Utc;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelInfo, ChannelId,
    ChannelProvider, ChannelResult, ChannelState, ChannelStatus,
    MessageId, OutboundMessage, SendResult,
};
use crate::thinker::interaction::{
    InteractionConstraints, InteractionManifest, InteractionParadigm,
};

pub use config::FeishuConfig;
use api::{FeishuApi, FeishuSendError};
use auth::TokenManager;
use user_cache::UserProfileCache;
use websocket::WsLoopContext;

/// Determine if the response should use a Feishu card for markdown rendering.
fn should_use_card(text: &str, render_mode: &str) -> bool {
    match render_mode {
        "card" => true,
        "raw" => false,
        // "auto": use card for rich/long content
        _ => text.len() > 200
            || text.contains("```")
            || text.contains("|---|")
            || text.contains("|:--"),
    }
}

fn map_send_error(e: FeishuSendError) -> ChannelError {
    match e {
        FeishuSendError::RateLimited { retry_after_secs } => {
            ChannelError::RateLimited { retry_after_secs }
        }
        FeishuSendError::Other(msg) => ChannelError::SendFailed(msg),
    }
}

pub struct FeishuChannel {
    info: ChannelInfo,
    config: FeishuConfig,
    channel_state: ChannelState,
    api: Option<Arc<FeishuApi>>,
    user_cache: Option<Arc<UserProfileCache>>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl FeishuChannel {
    pub fn new(id: impl Into<String>, config: FeishuConfig) -> Result<Self, ChannelError> {
        config.validate()?;

        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Feishu".to_string(),
            channel_type: "feishu".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Ok(Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            api: None,
            user_cache: None,
            shutdown_tx: None,
        })
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: true,
            audio: false,
            video: false,
            reactions: true,
            replies: true,
            editing: true,
            deletion: false,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true,
            max_message_length: 4096,
            max_attachment_size: 20 * 1024 * 1024,
            stream_protocol: Default::default(),
        }
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.channel_state.set_status(ChannelStatus::Connecting).await;

        let http = reqwest::Client::new();
        let base_url = self.config.base_url();

        let auth = Arc::new(TokenManager::new(
            &self.config.app_id,
            &self.config.app_secret,
            &base_url,
            http.clone(),
        ));
        auth.refresh_token().await
            .map_err(|e| ChannelError::AuthFailed(format!("Token acquisition failed: {e}")))?;

        let api = Arc::new(FeishuApi::new(auth.clone(), &base_url, http.clone()));

        let bot_info = api.get_bot_info().await
            .map_err(|e| ChannelError::AuthFailed(format!("Bot info failed: {e}")))?;
        tracing::info!("Feishu bot connected: {:?}", bot_info.app_name);

        let bot_open_id = api.bot_open_id().await.unwrap_or_default();

        let ws_url = api.get_ws_endpoint().await
            .map_err(|e| ChannelError::Internal(format!("WS endpoint failed: {e}")))?;

        let user_cache = Arc::new(UserProfileCache::new());

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        auth.spawn_token_refresh(shutdown_rx.clone());

        let ws_ctx = WsLoopContext {
            initial_ws_url: ws_url,
            channel_id: self.info.id.clone(),
            config: self.config.clone(),
            bot_open_id,
            sender: self.channel_state.sender(),
            status_handle: self.channel_state.status_handle(),
            shutdown_rx,
            ws_http: http,
            ws_base_url: base_url,
            ws_token: auth.token_state(),
            user_cache: user_cache.clone(),
        };

        tokio::spawn(websocket::run_ws_loop(ws_ctx));

        self.api = Some(api);
        self.user_cache = Some(user_cache);
        self.channel_state.set_status(ChannelStatus::Connected).await;

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        self.api = None;
        self.user_cache = None;
        self.channel_state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let api = self.api.as_ref()
            .ok_or_else(|| ChannelError::NotConnected("API not initialized".to_string()))?;

        let chat_id = message.conversation_id.as_str();
        let reply_to = message.reply_to.as_ref().map(|id| id.as_str());

        let has_image = message.attachments.iter().any(|a| a.mime_type.starts_with("image/"));

        let msg_id = if has_image {
            if let Some(attachment) = message.attachments.iter().find(|a| a.mime_type.starts_with("image/")) {
                let image_data = attachment.data.clone()
                    .ok_or_else(|| ChannelError::SendFailed("Image attachment has no data".to_string()))?;
                let filename = attachment.filename.as_deref().unwrap_or("image.png");
                let image_key = api.upload_image(image_data, filename).await
                    .map_err(ChannelError::SendFailed)?;
                api.send_image(chat_id, &image_key, reply_to).await
                    .map_err(map_send_error)?
            } else {
                unreachable!()
            }
        } else {
            if message.text.is_empty() {
                return Err(ChannelError::SendFailed("Empty message".to_string()));
            }
            if should_use_card(&message.text, &self.config.render_mode) {
                api.send_card(chat_id, &message.text, reply_to).await
                    .map_err(map_send_error)?
            } else {
                api.send_text(chat_id, &message.text, reply_to).await
                    .map_err(map_send_error)?
            }
        };

        if has_image && !message.text.is_empty() {
            let _ = api.send_text(chat_id, &message.text, reply_to).await;
        }

        Ok(SendResult {
            message_id: MessageId::new(msg_id),
            timestamp: Utc::now(),
        })
    }
}

impl ChannelProvider for FeishuChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest::new(InteractionParadigm::Messaging)
            .with_constraints(
                InteractionConstraints::new()
                    .max_output_chars(4096)
                    .supports_streaming(self.config.streaming)
                    .prefer_compact(false),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::should_use_card;

    #[test]
    fn test_should_use_card_auto_plain() {
        assert!(!should_use_card("Hello world", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_code_block() {
        assert!(should_use_card("Here is code:\n```rust\nfn main() {}\n```", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_table() {
        assert!(should_use_card("| A |---|B |", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_long() {
        let long_text = "a".repeat(201);
        assert!(should_use_card(&long_text, "auto"));
    }

    #[test]
    fn test_should_use_card_forced() {
        assert!(should_use_card("Hi", "card"));
    }

    #[test]
    fn test_should_use_card_raw() {
        assert!(!should_use_card("```code```", "raw"));
    }
}
