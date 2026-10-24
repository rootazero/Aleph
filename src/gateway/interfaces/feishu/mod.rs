pub(crate) mod api;
pub(crate) mod api_handle;
pub(crate) mod auth;
pub(crate) mod config;
pub(crate) mod envelope;
pub(crate) mod message_ops;
pub(crate) mod types;

pub mod feishu_inbound;
pub mod feishu_outbound;
pub mod feishu_policy;
pub mod feishu_runtime;

use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
};
use crate::sync_primitives::{Arc, Mutex as StdMutex};
use async_trait::async_trait;
use tokio::sync::watch;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
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

#[derive(Debug, thiserror::Error)]
enum WebhookShutdownResidual {
    #[error("webhook serve failed: {0}")]
    Serve(#[source] std::io::Error),
    #[error("webhook join failed; accepted handlers may remain: {0}")]
    Join(#[source] tokio::task::JoinError),
}

pub struct FeishuChannel {
    info: ChannelInfo,
    config: FeishuConfig,
    channel_state: ChannelState,
    api: Option<Arc<FeishuApi>>,
    runtime: Option<FeishuRuntime>,
    message_ops: Option<MessageOps>,
    shutdown_tx: Option<watch::Sender<bool>>,
    webhook_handle: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    // Retain failure evidence after taking the task; repeated stop must not
    // turn an unproven handler join into a successful disposal.
    webhook_shutdown_residual: Option<WebhookShutdownResidual>,
    test_mode: bool,
    provider: ProviderHandle,
    refresh_handle: Option<tokio::task::JoinHandle<()>>,
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

        let mut lifecycle = ProviderLifecycle::new(
            ProviderId::new(format!("gateway.feishu:{}", info.id)).expect("nonempty provider id"),
            ProviderTransport::InProcess,
        );
        lifecycle
            .transition(ProviderState::Cold)
            .expect("discovery admission");
        Ok(Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            api: None,
            runtime: None,
            message_ops: None,
            shutdown_tx: None,
            webhook_handle: None,
            webhook_shutdown_residual: None,
            test_mode,
            provider: ProviderHandle::new(lifecycle),
            refresh_handle: None,
        })
    }

    pub fn for_test(id: impl Into<String>, config: FeishuConfig) -> Result<Self, ChannelError> {
        Self::with_mode(id, config, true)
    }

    pub(crate) fn revoke(&self) {
        if matches!(
            self.provider.state(),
            ProviderState::Starting | ProviderState::Ready
        ) {
            let _ = self.provider.transition(ProviderState::Stopping);
        }
        api_handle::withdraw(
            self.info.id.as_str(),
            &self.provider,
            self.provider.generation(),
        );
    }

    async fn cleanup(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(task) = self.webhook_handle.take() {
            self.webhook_shutdown_residual = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(WebhookShutdownResidual::Serve(error)),
                Err(error) => Some(WebhookShutdownResidual::Join(error)),
            };
        }
        if let Some(task) = self.refresh_handle.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(mut runtime) = self.runtime.take() {
            runtime.stop().await;
        }
        if let Some(api) = &self.api {
            api.drain().await;
        }
        self.message_ops = None;
        self.api = None;
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
    }

    fn admit_call(&self) -> ChannelResult<()> {
        self.provider
            .acquire_call(self.provider.generation())
            .map_err(|e| ChannelError::NotConnected(e.to_string()))
    }
    async fn start_runtime(&mut self, generation: u64) -> ChannelResult<()> {
        if self.test_mode {
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

        let api = Arc::new(
            FeishuApi::new(auth.clone(), &base_url, http.clone())
                .with_provider(self.provider.clone(), generation),
        );
        self.api = Some(api.clone());
        let bot_info = api
            .get_bot_info()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Bot info failed: {e}")))?;
        tracing::info!("Feishu bot connected: {:?}", bot_info.app_name);

        let bot_open_id = api.bot_open_id().await.unwrap_or_default();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        self.refresh_handle = Some(auth.spawn_token_refresh(shutdown_rx.clone()));

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

            let listener = tokio::net::TcpListener::bind((
                self.config.webhook_host.as_str(),
                self.config.webhook_port,
            ))
            .await
            .map_err(|e| ChannelError::Internal(format!("Feishu webhook bind failed: {e}")))?;
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
            self.webhook_handle = Some(tokio::spawn(run_webhook_server(
                webhook_state,
                listener,
                shutdown_rx,
            )));
        } else {
            let mut runtime = FeishuRuntime::new(api.clone(), self.config.clone(), bot_open_id);
            runtime
                .start(
                    ChannelId::new(&self.config.app_id),
                    self.channel_state.sender(),
                )
                .await?;
            self.runtime = Some(runtime);
        }

        self.api = Some(api.clone());
        self.message_ops = Some(MessageOps::new(api));
        Ok(())
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: true,
            audio: false,
            video: false,
            reactions: true,
            replies: true,
            polls: false,
            group_icons: false,
            // Truthful despite the `Channel::edit` override below: that
            // override delegates to `MessageOps::edit`, which refuses
            // unconditionally. The override's *existence* is not evidence of
            // the capability — read what it returns. (Feishu's real streaming
            // runs through the orchestrated card emitter, not `Channel::edit`.)
            editing: false,
            deletion: false,
            // Feishu has no typing endpoint: the streaming emitter *emulates*
            // one by reacting to the inbound message
            // (`feishu_outbound/reactions.rs`), which needs a message_id that
            // `Channel::send_typing(&conversation_id)` does not carry. So the
            // trait method is genuinely unavailable here, and declaring `true`
            // only bought a fake success from its default body. The emulated
            // indicator is unaffected — it runs off `[channels.feishu]
            // typing_indicator`, a different knob.
            typing_indicator: false,
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
        match self.provider.state() {
            ProviderState::Ready => self.channel_state.status(),
            ProviderState::Starting => ChannelStatus::Connecting,
            _ => ChannelStatus::Disconnected,
        }
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if let Some(residual) = &self.webhook_shutdown_residual {
            return Err(ChannelError::Internal(residual.to_string()));
        }
        match self.provider.state() {
            ProviderState::Ready => return Ok(()),
            ProviderState::Disabled => {
                return Err(ChannelError::NotConnected(
                    "Feishu provider disabled/retired".into(),
                ))
            }
            _ => {}
        }
        self.provider
            .transition(ProviderState::Starting)
            .map_err(|e| ChannelError::Internal(e.to_string()))?;
        let generation = self.provider.generation();
        let result = self.start_runtime(generation).await.and_then(|()| {
            self.provider
                .with_ready_commit(generation, |_| {
                    if let Some(api) = &self.api {
                        api_handle::publish(
                            self.info.id.as_str(),
                            api,
                            &self.config,
                            &self.provider,
                            generation,
                        );
                    }
                    Ok::<_, crate::capability::provider::ProviderCallError>(())
                })
                .map_err(|e| ChannelError::NotConnected(e.to_string()))
        });
        if let Err(error) = result {
            self.revoke();
            self.cleanup().await;
            let _ = self.provider.transition(ProviderState::Failed);
            return Err(match &self.webhook_shutdown_residual {
                Some(residual) => ChannelError::Internal(format!("{error}; {residual}")),
                None => error,
            });
        }
        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        self.revoke();
        self.cleanup().await;
        if let Some(residual) = &self.webhook_shutdown_residual {
            if self.provider.state() == ProviderState::Stopping {
                let _ = self.provider.transition(ProviderState::Failed);
            }
            return Err(ChannelError::Internal(residual.to_string()));
        }
        if matches!(
            self.provider.state(),
            ProviderState::Stopping | ProviderState::Failed
        ) {
            self.provider
                .transition(ProviderState::Stopped)
                .map_err(|e| ChannelError::Internal(e.to_string()))?;
        }
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        self.admit_call()?;
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
        self.admit_call()?;
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
        self.admit_call()?;
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
        self.admit_call()?;
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

/// Factory for creating Feishu/Lark channels.
///
/// Written 2026-08-18, four months after the adapter. The back-fill that made
/// the ten pre-table adapters reachable (`643c47bf9`) swept for
/// `impl ChannelFactory`, and feishu — a complete adapter that simply never
/// had a factory struct — was structurally invisible to that sweep. So
/// `[channels.feishu]` resolved to `None` in `create_channel_from_config` from
/// the day the table landed while the Panel shipped a full Feishu settings
/// card, and no test could see it: every feishu test constructs
/// `FeishuChannel::for_test` directly, which never consults the table.
///
/// `FeishuChannel::new` already runs `FeishuConfig::validate`, so there is no
/// second validate call here — a second one would be a second answer to
/// "is this config usable".
pub struct FeishuChannelFactory;

#[async_trait]
impl ChannelFactory for FeishuChannelFactory {
    fn channel_type(&self) -> &str {
        "feishu"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: FeishuConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid Feishu config: {e}")))?;

        Ok(Box::new(FeishuChannel::new("feishu", config)?))
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::gateway::interfaces::feishu::api_handle;

    fn test_config() -> FeishuConfig {
        serde_json::from_value(serde_json::json!({
            "app_id": "test-app",
            "app_secret": "test-secret"
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn start_stop_cycles_provider_and_leaves_no_stale_publication() {
        let mut channel = FeishuChannel::for_test("feishu-lifecycle", test_config()).unwrap();
        assert_eq!(channel.provider.state(), ProviderState::Cold);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);

        channel.start().await.unwrap();
        assert_eq!(channel.provider.state(), ProviderState::Ready);
        assert_eq!(channel.status(), ChannelStatus::Connected);

        channel.stop().await.unwrap();
        assert_eq!(channel.provider.state(), ProviderState::Stopped);
        assert_eq!(channel.status(), ChannelStatus::Disconnected);

        // The registry's restart path is stop→start on the same channel; the
        // provider machine must allow Stopped→Starting.
        channel.start().await.unwrap();
        assert_eq!(channel.status(), ChannelStatus::Connected);
        channel.stop().await.unwrap();

        // Test mode never publishes, and the read side must not fabricate one.
        assert!(api_handle::get("feishu-lifecycle").is_none());
    }

    #[tokio::test]
    async fn disabled_provider_refuses_start() {
        let mut channel = FeishuChannel::for_test("feishu-disabled", test_config()).unwrap();
        channel
            .provider
            .transition(ProviderState::Disabled)
            .unwrap();
        let err = channel.start().await.unwrap_err();
        assert!(matches!(err, ChannelError::NotConnected(_)));
        assert_eq!(channel.status(), ChannelStatus::Disconnected);
    }

    #[tokio::test]
    async fn failed_webhook_join_is_retained_and_blocks_restart() {
        let mut channel =
            FeishuChannel::for_test("feishu-webhook-residual", test_config()).unwrap();
        channel.webhook_handle = Some(tokio::spawn(async {
            panic!("simulated webhook task failure");
            #[allow(unreachable_code)]
            Ok::<(), std::io::Error>(())
        }));

        channel.cleanup().await;

        assert!(channel.webhook_shutdown_residual.is_some());
        let error = channel.start().await.unwrap_err();
        assert!(matches!(error, ChannelError::Internal(_)));
        assert_eq!(channel.provider.state(), ProviderState::Cold);
    }

    #[tokio::test]
    async fn send_without_ready_provider_is_refused() {
        let channel = FeishuChannel::for_test("feishu-cold", test_config()).unwrap();
        let err = channel
            .send(OutboundMessage::text("ou_1", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::NotConnected(_)));
    }

    #[tokio::test]
    async fn generation_fences_stale_admission_after_restart() {
        let mut channel = FeishuChannel::for_test("feishu-gen", test_config()).unwrap();
        channel.start().await.unwrap();
        let first = channel.provider.generation();
        channel.stop().await.unwrap();
        channel.start().await.unwrap();
        let second = channel.provider.generation();
        assert!(second > first, "restart must allocate a new generation");
        // A caller holding the previous generation is refused.
        assert!(channel.provider.acquire_call(first).is_err());
        channel.stop().await.unwrap();
    }
}
