//! LINE Channel Implementation
//!
//! Integrates with LINE Messaging API using webhook-based inbound
//! and HTTP API outbound.

pub mod config;
pub mod message_ops;
pub mod types;
pub mod webhook;

use crate::sync_primitives::Arc;
use async_trait::async_trait;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    PairingData, SendResult,
};

pub use config::{LineConfig, LineDmPolicy, LineGroupPolicy};

use message_ops::LineMessagingApi;
use webhook::WebhookContext;

pub struct LineChannel {
    info: ChannelInfo,
    config: LineConfig,
    channel_state: ChannelState,
    api: Option<Arc<LineMessagingApi>>,
    webhook_handle: Option<tokio::task::JoinHandle<()>>,
    /// Optional custom API base URL for testing (e.g. mock server)
    api_base: Option<String>,
}

impl LineChannel {
    pub fn new(id: impl Into<String>, config: LineConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "LINE".to_string(),
            channel_type: "line".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            api: None,
            webhook_handle: None,
            api_base: None,
        }
    }

    /// Create a LINE channel configured for testing against a mock API server.
    pub fn for_test(
        id: impl Into<String>,
        config: LineConfig,
        api_base: impl Into<String>,
    ) -> Self {
        let mut channel = Self::new(id, config);
        channel.api_base = Some(api_base.into());
        channel
    }

    const fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: true,
            replies: true,
            editing: false,
            deletion: true,
            typing_indicator: true,
            read_receipts: false,
            rich_text: true,
            max_message_length: 5000,
            max_attachment_size: 50 * 1024 * 1024,
            // NOT EditBased: `edit` below returns `UnsupportedFeature`
            // unconditionally, so the two declarations contradicted each other
            // and this one won — LINE was forced into edit-based streaming and
            // every reply past the first flush was silently discarded.
            stream_protocol: crate::gateway::channel::StreamProtocol::None,
        }
    }
}

#[async_trait]
impl Channel for LineChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn get_pairing_data(&self) -> ChannelResult<PairingData> {
        Err(ChannelError::UnsupportedFeature(
            "LINE does not support pairing codes".to_string(),
        ))
    }

    async fn list_active_pairing_codes(&self) -> ChannelResult<Vec<(String, u64)>> {
        Ok(Vec::new())
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.config.validate().map_err(ChannelError::ConfigError)?;

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;
        tracing::info!("Starting LINE channel...");

        let api = if let Some(ref base) = self.api_base {
            Arc::new(LineMessagingApi::with_base(
                self.config.channel_access_token.clone(),
                base.clone(),
            ))
        } else {
            Arc::new(LineMessagingApi::new(
                self.config.channel_access_token.clone(),
            ))
        };
        self.api = Some(api);

        if self.api_base.is_none() {
            let webhook_ctx = WebhookContext {
                config: self.config.clone(),
                channel_id: self.info.id.clone(),
                sender: self.channel_state.sender(),
                status_handle: self.channel_state.status_handle(),
            };

            self.webhook_handle = Some(tokio::spawn(webhook::run_webhook_server(webhook_ctx)));

            tracing::info!(
                "LINE channel started on {}:{}",
                self.config.webhook_host,
                self.config.webhook_port
            );
        }

        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping LINE channel...");

        if let Some(handle) = self.webhook_handle.take() {
            handle.abort();
        }

        self.api = None;
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("API not initialized".to_string()))?;

        let chat_id = message.conversation_id.as_str();

        let msg_id = match message
            .metadata
            .get("line::message_type")
            .map(|s| s.as_str())
        {
            Some("flex") => {
                let alt_text = message
                    .metadata
                    .get("line::flex_alt_text")
                    .cloned()
                    .unwrap_or_else(|| "Flex message".to_string());
                let contents_json =
                    message.metadata.get("line::flex_contents").ok_or_else(|| {
                        ChannelError::ConfigError(
                            "flex_contents required for flex messages".to_string(),
                        )
                    })?;
                let contents: message_ops::FlexBubbleContents = serde_json::from_str(contents_json)
                    .map_err(|e| {
                        ChannelError::ConfigError(format!("Invalid flex_contents: {e}"))
                    })?;
                api.push_flex(chat_id, &alt_text, contents)
                    .await
                    .map_err(ChannelError::SendFailed)?
            }
            Some("template") => {
                let alt_text = message
                    .metadata
                    .get("line::template_alt_text")
                    .cloned()
                    .unwrap_or_else(|| "Template message".to_string());
                let template_json = message.metadata.get("line::template").ok_or_else(|| {
                    ChannelError::ConfigError("template required for template messages".to_string())
                })?;
                let template: message_ops::TemplatePayload = serde_json::from_str(template_json)
                    .map_err(|e| ChannelError::ConfigError(format!("Invalid template: {e}")))?;
                api.push_template(chat_id, &alt_text, template)
                    .await
                    .map_err(ChannelError::SendFailed)?
            }
            Some("image") => {
                let original_url = message
                    .metadata
                    .get("line::image_url")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "image_url required for image messages".to_string(),
                        )
                    })?
                    .clone();
                let preview_url = message
                    .metadata
                    .get("line::preview_url")
                    .cloned()
                    .unwrap_or_else(|| original_url.clone());
                api.push_image(chat_id, &original_url, &preview_url)
                    .await
                    .map_err(ChannelError::SendFailed)?
            }
            Some("video") => {
                let video_url = message
                    .metadata
                    .get("line::video_url")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "video_url required for video messages".to_string(),
                        )
                    })?
                    .clone();
                let preview_url = message
                    .metadata
                    .get("line::preview_url")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "preview_url required for video messages".to_string(),
                        )
                    })?
                    .clone();
                let duration = message
                    .metadata
                    .get("line::duration")
                    .and_then(|s| s.parse::<u64>().ok());
                api.push_video(chat_id, &video_url, &preview_url, duration)
                    .await
                    .map_err(ChannelError::SendFailed)?
            }
            Some("audio") => {
                let audio_url = message
                    .metadata
                    .get("line::audio_url")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "audio_url required for audio messages".to_string(),
                        )
                    })?
                    .clone();
                let duration = message
                    .metadata
                    .get("line::duration")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "duration required for audio messages".to_string(),
                        )
                    })?
                    .parse::<u64>()
                    .map_err(|e| ChannelError::ConfigError(format!("Invalid duration: {e}")))?;
                api.push_audio(chat_id, &audio_url, duration)
                    .await
                    .map_err(ChannelError::SendFailed)?
            }
            Some("location") => {
                let title = message
                    .metadata
                    .get("line::location_title")
                    .cloned()
                    .unwrap_or_default();
                let address = message
                    .metadata
                    .get("line::location_address")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "location_address required for location messages".to_string(),
                        )
                    })?
                    .clone();
                let latitude: f64 = message
                    .metadata
                    .get("line::latitude")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "latitude required for location messages".to_string(),
                        )
                    })?
                    .parse()
                    .map_err(|e| ChannelError::ConfigError(format!("Invalid latitude: {e}")))?;
                let longitude: f64 = message
                    .metadata
                    .get("line::longitude")
                    .ok_or_else(|| {
                        ChannelError::ConfigError(
                            "longitude required for location messages".to_string(),
                        )
                    })?
                    .parse()
                    .map_err(|e| ChannelError::ConfigError(format!("Invalid longitude: {e}")))?;
                api.push_location(chat_id, &title, &address, latitude, longitude)
                    .await
                    .map_err(ChannelError::SendFailed)?
            }
            _ => {
                if let Some(quick_reply_json) = message.metadata.get("line::quick_reply_actions") {
                    let actions: Vec<message_ops::QuickReplyAction> =
                        serde_json::from_str(quick_reply_json).map_err(|e| {
                            ChannelError::ConfigError(format!("Invalid quick_reply_actions: {e}"))
                        })?;
                    api.push_quick_reply(chat_id, &message.text, actions)
                        .await
                        .map_err(ChannelError::SendFailed)?
                } else {
                    api.push_text(chat_id, &message.text)
                        .await
                        .map_err(ChannelError::SendFailed)?
                }
            }
        };

        Ok(SendResult {
            message_id: MessageId::new(msg_id),
            timestamp: chrono::Utc::now(),
        })
    }

    async fn send_typing(&self, _conversation_id: &ConversationId) -> ChannelResult<()> {
        Ok(())
    }

    async fn react(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _reaction: &str,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature(
            "LINE reactions not yet implemented".to_string(),
        ))
    }

    async fn edit(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _new_text: &str,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature(
            "LINE does not support message editing".to_string(),
        ))
    }

    async fn delete(
        &self,
        _conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        let api = self
            .api
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("API not initialized".to_string()))?;

        api.delete_message(message_id.as_str())
            .await
            .map_err(ChannelError::SendFailed)?;
        Ok(())
    }
}

/// Factory for creating LINE channels.
pub struct LineChannelFactory;

#[async_trait]
impl ChannelFactory for LineChannelFactory {
    fn channel_type(&self) -> &str {
        "line"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: LineConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid LINE config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(LineChannel::new("line", config)))
    }
}

fn line_factory_creator(
    _config: crate::gateway::channel::ChannelConfig,
) -> crate::gateway::channel::ChannelResult<crate::sync_primitives::Arc<dyn ChannelFactory>> {
    Ok(crate::sync_primitives::Arc::new(LineChannelFactory))
}

pub fn register_with_plugin() {
    let _ = crate::gateway::interfaces::plugin::register("line", line_factory_creator);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_capabilities() {
        let caps = LineChannel::capabilities();
        assert!(caps.attachments);
        assert!(caps.images);
        assert!(caps.audio);
        assert!(caps.video);
        assert!(caps.reactions);
        assert!(caps.replies);
        assert!(!caps.editing);
        assert!(caps.deletion);
        assert!(caps.typing_indicator);
        assert!(!caps.read_receipts);
        assert!(caps.rich_text);
        assert_eq!(caps.max_message_length, 5000);
    }

    #[test]
    fn test_channel_creation() {
        let config = LineConfig {
            channel_access_token: "test_token".to_string(),
            channel_secret: "test_secret".to_string(),
            ..Default::default()
        };
        let channel = LineChannel::new("line-test", config);
        assert_eq!(channel.info().id.as_str(), "line-test");
        assert_eq!(channel.info().channel_type, "line");
    }

    #[test]
    fn test_factory_channel_type() {
        let factory = LineChannelFactory;
        assert_eq!(factory.channel_type(), "line");
    }
}
