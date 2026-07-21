pub(crate) mod api;
pub(crate) mod auth;
pub(crate) mod config;
pub(crate) mod message_ops;
pub(crate) mod types;

pub mod feishu_inbound;
pub mod feishu_outbound;
pub mod feishu_policy;
pub mod feishu_runtime;

use crate::sync_primitives::{Arc, Mutex as StdMutex};
use async_trait::async_trait;
use tokio::sync::watch;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, ConversationId, MessageId, OutboundMessage,
    SendResult,
};

use api::FeishuApi;
use auth::TokenManager;
pub use config::FeishuConfig;
use feishu_inbound::{InboundPolicy, MessageDedup, UserProfileCache};
use feishu_outbound::FeishuSender;
use feishu_runtime::FeishuRuntime;
use message_ops::MessageOps;

pub struct FeishuChannel {
    info: ChannelInfo,
    config: FeishuConfig,
    channel_state: ChannelState,
    api: Option<Arc<FeishuApi>>,
    runtime: Option<FeishuRuntime>,
    message_ops: Option<MessageOps>,
    shutdown_tx: Option<watch::Sender<bool>>,
    webhook_handle: Option<tokio::task::JoinHandle<()>>,
    test_mode: bool,
}

impl FeishuChannel {
    pub fn new(id: impl Into<String>, config: FeishuConfig) -> Result<Self, ChannelError> {
        Self::with_mode(id, config, false)
    }

    fn with_mode(
        id: impl Into<String>,
        config: FeishuConfig,
        test_mode: bool,
    ) -> Result<Self, ChannelError> {
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
            runtime: None,
            message_ops: None,
            shutdown_tx: None,
            webhook_handle: None,
            test_mode,
        })
    }

    pub fn for_test(id: impl Into<String>, config: FeishuConfig) -> Result<Self, ChannelError> {
        Self::with_mode(id, config, true)
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: true,
            audio: false,
            video: false,
            reactions: true,
            replies: true,
            editing: false,
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

    fn status(&self) -> ChannelStatus {
        if self.test_mode {
            return self.channel_state.status();
        }
        match self.runtime.as_ref().map(|r| r.connection_state()) {
            Some(feishu_runtime::RuntimeState::Connected) => ChannelStatus::Connected,
            Some(feishu_runtime::RuntimeState::Connecting) => ChannelStatus::Connecting,
            Some(feishu_runtime::RuntimeState::Error) => ChannelStatus::Error,
            _ => ChannelStatus::Disconnected,
        }
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if self.test_mode {
            self.channel_state
                .set_status(ChannelStatus::Connected)
                .await;
            tracing::info!("Feishu channel started in test mode");
            return Ok(());
        }

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;

        let http = reqwest::Client::new();
        let base_url = self.config.base_url();

        let auth = Arc::new(TokenManager::new(
            &self.config.app_id,
            &self.config.app_secret,
            &base_url,
            http.clone(),
        ));
        auth.refresh_token()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Token acquisition failed: {e}")))?;

        let api = Arc::new(FeishuApi::new(auth.clone(), &base_url, http.clone()));

        let bot_info = api
            .get_bot_info()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Bot info failed: {e}")))?;
        tracing::info!("Feishu bot connected: {:?}", bot_info.app_name);

        let bot_open_id = api.bot_open_id().await.unwrap_or_default();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        auth.spawn_token_refresh(shutdown_rx);

        if self.config.is_webhook_mode() {
            tracing::info!(
                "Starting Feishu webhook server on {}:{}{}",
                self.config.webhook_host,
                self.config.webhook_port,
                self.config.webhook_path
            );

            use crate::gateway::interfaces::feishu::feishu_inbound::webhook_server::{
                run_webhook_server, WebhookState,
            };

            let webhook_state = WebhookState {
                config: self.config.clone(),
                channel_id: self.info.id.clone(),
                bot_open_id: bot_open_id.clone(),
                sender: self.channel_state.sender(),
                api: api.clone(),
                user_cache: Arc::new(UserProfileCache::new()),
                dedup: Arc::new(StdMutex::new(MessageDedup::new())),
                policy: Arc::new(InboundPolicy::new(self.config.clone(), bot_open_id)),
            };
            self.webhook_handle = Some(tokio::spawn(run_webhook_server(webhook_state)));
        } else {
            let mut runtime = FeishuRuntime::new(api.clone(), self.config.clone(), bot_open_id);
            runtime.start(self.channel_state.sender()).await?;
            self.runtime = Some(runtime);
        }

        self.api = Some(api.clone());
        self.message_ops = Some(MessageOps::new(api));
        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.webhook_handle.take() {
            handle.abort();
        }
        if let Some(mut runtime) = self.runtime.take() {
            runtime.stop().await;
        }
        self.api = None;
        self.message_ops = None;
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.test_mode {
            return Ok(SendResult {
                message_id: MessageId::new(format!(
                    "feishu-test-{}",
                    chrono::Utc::now().timestamp_millis()
                )),
                timestamp: chrono::Utc::now(),
            });
        }

        let api = self
            .api
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("API not initialized".to_string()))?;
        FeishuSender::send_message(api, message, &self.config).await
    }

    async fn react(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _reaction: &str,
    ) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let ops = self
            .message_ops
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("API not initialized".to_string()))?;
        ops.react(_conversation_id, _message_id, _reaction).await
    }

    async fn edit(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let ops = self
            .message_ops
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("API not initialized".to_string()))?;
        ops.edit(conversation_id, message_id, new_text).await
    }

    async fn delete(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
    ) -> ChannelResult<()> {
        if self.test_mode {
            return Ok(());
        }

        let ops = self
            .message_ops
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("API not initialized".to_string()))?;
        ops.delete(conversation_id, message_id).await
    }
}
