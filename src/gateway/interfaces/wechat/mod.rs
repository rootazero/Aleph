//! `WeChat` Channel Implementation
//!
//! iLink Bot API integration for `WeChat` messaging.

pub mod api;
pub mod auth;
pub mod config;
pub mod inbound;
pub mod media;
pub mod outbound;
pub mod runtime;
pub mod sync_buf;
pub mod types;

use crate::sync_primitives::Arc;
use async_trait::async_trait;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendResult,
};

pub use config::WeChatConfig;

pub struct WeChatChannel {
    info: ChannelInfo,
    config: WeChatConfig,
    channel_state: ChannelState,
    runtime: Option<Arc<runtime::WeChatRuntime>>,
    test_mode: bool,
}

impl WeChatChannel {
    pub fn new(id: impl Into<String>, config: WeChatConfig) -> Self {
        Self::with_mode(id, config, false)
    }

    fn with_mode(id: impl Into<String>, config: WeChatConfig, test_mode: bool) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "WeChat".to_string(),
            channel_type: "wechat".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            runtime: None,
            test_mode,
        }
    }

    pub fn for_test(id: impl Into<String>, config: WeChatConfig) -> Self {
        Self::with_mode(id, config, true)
    }

    const fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: true,
            video: true,
            reactions: false,
            replies: false,
            editing: false,
            deletion: false,
            typing_indicator: true,
            read_receipts: false,
            rich_text: false,
            max_message_length: 4000,
            max_attachment_size: 10 * 1024 * 1024,
            // NOT EditBased: `edit` below returns `UnsupportedFeature`
            // unconditionally. Same contradiction as LINE — see that channel.
            stream_protocol: crate::gateway::channel::StreamProtocol::None,
        }
    }
}

#[async_trait]
impl Channel for WeChatChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    async fn start(&mut self) -> ChannelResult<()> {
        self.config.validate().map_err(ChannelError::ConfigError)?;

        if self.test_mode {
            self.channel_state
                .set_status(ChannelStatus::Connected)
                .await;
            tracing::info!("WeChat channel started in test mode");
            return Ok(());
        }

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;

        tracing::info!("Starting WeChat channel...");

        let runtime = Arc::new(runtime::WeChatRuntime::new(
            self.config.clone(),
            auth::ContextTokenStore::new(&self.config.data_dir),
        ));

        self.runtime = Some(runtime.clone());

        // `WeChatRuntime::start` runs the long-poll loop until `stop()` flips
        // the running flag, so it must run on its own task — awaiting it here
        // would block `start()` forever and leave the channel unusable.
        let sender = self.channel_state.sender();
        let poll_runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            poll_runtime.start(sender).await;
        });

        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;

        tracing::info!("WeChat channel started");
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping WeChat channel...");

        if let Some(runtime) = self.runtime.take() {
            runtime.stop().await;
        }

        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.test_mode {
            return Ok(SendResult {
                message_id: MessageId::new(format!(
                    "wechat-test-{}",
                    chrono::Utc::now().timestamp_millis()
                )),
                timestamp: chrono::Utc::now(),
            });
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("Runtime not initialized".to_string()))?;

        let context_token = runtime
            .get_context_token(&self.config.account_id, message.conversation_id.as_str())
            .await;

        let text = outbound::markdown::markdown_to_wechat(&message.text);
        let chunks = crate::gateway::channel_chunking::WhatsAppChunker::chunk_by_length(
            &text,
            self.info.capabilities.max_message_length,
        );

        for chunk in &chunks {
            let payload = outbound::mapper::build_send_payload(
                &self.config.account_id,
                message.conversation_id.as_str(),
                &self.config.account_id,
                chunk,
                context_token.as_deref(),
            );

            runtime
                .send_message(&self.config.token, payload)
                .await
                .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        }

        Ok(SendResult {
            message_id: MessageId::new(""),
            timestamp: chrono::Utc::now(),
        })
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("Runtime not initialized".to_string()))?;

        runtime
            .send_typing(&self.config.token, conversation_id.as_str())
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;

        Ok(())
    }

    async fn react(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _reaction: &str,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature(
            "WeChat reactions not yet implemented".to_string(),
        ))
    }

    async fn edit(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _new_text: &str,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature(
            "WeChat does not support message editing".to_string(),
        ))
    }

    async fn delete(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature(
            "WeChat does not support message deletion".to_string(),
        ))
    }
}

/// Factory for creating `WeChat` channels.
pub struct WeChatChannelFactory;

#[async_trait]
impl ChannelFactory for WeChatChannelFactory {
    fn channel_type(&self) -> &str {
        "wechat"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: WeChatConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid WeChat config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(WeChatChannel::new("wechat", config)))
    }
}

fn wechat_factory_creator(
    _config: crate::gateway::channel::ChannelConfig,
) -> crate::gateway::channel::ChannelResult<Arc<dyn ChannelFactory>> {
    Ok(Arc::new(WeChatChannelFactory))
}

pub fn register_with_plugin() {
    let _ = crate::gateway::interfaces::plugin::register("wechat", wechat_factory_creator);
}
