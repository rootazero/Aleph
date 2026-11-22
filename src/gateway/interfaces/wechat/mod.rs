//! `WeChat` Channel Implementation
//!
//! iLink Bot API integration for `WeChat` messaging.

pub mod api;
pub mod auth;
pub mod config;
pub mod inbound;
pub mod outbound;
pub mod runtime;
pub mod sync_buf;
pub mod types;

use std::time::Duration;

use crate::sync_primitives::Arc;
use async_trait::async_trait;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendOperation, SendResult,
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
            attachments: false,
            images: false,
            audio: false,
            video: false,
            reactions: false,
            replies: false,
            editing: false,
            deletion: false,
            typing_indicator: true,
            read_receipts: false,
            rich_text: false,
            polls: false,
            group_icons: false,
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
            self.info.id.clone(),
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

    fn send(&self, message: OutboundMessage) -> SendOperation {
        let runtime = self.runtime.clone();
        let test_mode = self.test_mode;
        let account_id = self.config.account_id.clone();
        let token = self.config.token.clone();
        let max_message_length = self.info.capabilities.max_message_length;
        let split_multiline = self.config.split_multiline_messages;
        // `validate` rejects negative/non-finite delays; clamp anyway so a
        // programmatic config cannot panic here.
        let chunk_delay = Duration::from_secs_f64(self.config.send_chunk_delay_seconds.max(0.0));
        let chunk_retries = self.config.send_chunk_retries;
        Box::pin(async move {
            if test_mode {
                return Ok(SendResult {
                    message_id: MessageId::new(format!(
                        "wechat-test-{}",
                        chrono::Utc::now().timestamp_millis()
                    )),
                    timestamp: chrono::Utc::now(),
                });
            }

            let runtime = runtime
                .as_ref()
                .ok_or_else(|| ChannelError::NotConnected("Runtime not initialized".to_string()))?;

            let context_token = runtime
                .get_context_token(&account_id, message.conversation_id.as_str())
                .await;

            let text = outbound::markdown::markdown_to_wechat(&message.text);
            let mut chunks = if split_multiline {
                // One message per non-empty line, still bounded by the length
                // limit for lines that are themselves too long.
                text.lines()
                    .filter(|line| !line.trim().is_empty())
                    .flat_map(|line| {
                        crate::gateway::channel_chunking::WhatsAppChunker::chunk_by_length(
                            line,
                            max_message_length,
                        )
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            if chunks.is_empty() {
                chunks = crate::gateway::channel_chunking::WhatsAppChunker::chunk_by_length(
                    &text,
                    max_message_length,
                );
            }

            for (index, chunk) in chunks.iter().enumerate() {
                // Space out consecutive chunks so the platform does not fold
                // them into one another or trip its burst limit.
                if index > 0 && !chunk_delay.is_zero() {
                    tokio::time::sleep(chunk_delay).await;
                }

                let mut attempt = 0_u32;
                loop {
                    let payload = outbound::mapper::build_send_payload(
                        &account_id,
                        message.conversation_id.as_str(),
                        &account_id,
                        chunk,
                        context_token.as_deref(),
                    );

                    match runtime.send_message(&token, payload).await {
                        Ok(()) => break,
                        Err(e) if attempt < chunk_retries => {
                            attempt += 1;
                            tracing::warn!(
                                attempt,
                                max_retries = chunk_retries,
                                error = %e,
                                "WeChat chunk send failed; retrying"
                            );
                            if !chunk_delay.is_zero() {
                                tokio::time::sleep(chunk_delay).await;
                            }
                        }
                        Err(e) => return Err(ChannelError::SendFailed(e.to_string())),
                    }
                }
            }

            // The reply has been delivered, so the "typing…" hint set for this
            // turn no longer describes anything the user can see coming.
            let _ = runtime
                .stop_typing(&token, message.conversation_id.as_str())
                .await;

            Ok(SendResult {
                message_id: MessageId::new(""),
                timestamp: chrono::Utc::now(),
            })
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
        self.create_with_id("wechat", config).await
    }

    async fn create_with_id(
        &self,
        id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<Box<dyn Channel>> {
        let config: WeChatConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid WeChat config: {e}")))?;

        config.validate().map_err(ChannelError::ConfigError)?;

        Ok(Box::new(WeChatChannel::new(id, config)))
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
