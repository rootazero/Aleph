pub mod access;
pub mod api;
pub mod config;
pub mod delivery;
pub mod error;
pub mod gateway;
pub mod handlers;
pub mod types;

pub use config::{QQAccountConfig, QQConfig, QQDmPolicy, QQGroupPolicy};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, MessageId, OutboundMessage, SendResult,
};
use crate::gateway::interfaces::qq::types::QQEvent;
use crate::sync_primitives::Arc;
use async_trait::async_trait;

pub struct QQChannel {
    info: ChannelInfo,
    channel_state: ChannelState,
    config: QQConfig,
    api_client: Option<Arc<api::QQApiClient>>,
    gateway_handle: Option<tokio::task::JoinHandle<()>>,
    event_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    reply_tracker: Arc<delivery::ReplyTracker>,
    test_mode: bool,
}

impl QQChannel {
    pub fn new(id: impl Into<String>, config: QQConfig) -> Self {
        Self::with_mode(id, config, false)
    }

    fn with_mode(id: impl Into<String>, config: QQConfig, test_mode: bool) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "QQ".to_string(),
            channel_type: "qq".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            channel_state: ChannelState::new(100),
            config,
            api_client: None,
            gateway_handle: None,
            event_handle: None,
            shutdown_tx: None,
            reply_tracker: Arc::new(delivery::ReplyTracker::new()),
            test_mode,
        }
    }

    pub fn for_test(id: impl Into<String>, config: QQConfig) -> Self {
        Self::with_mode(id, config, true)
    }

    const fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: true,
            images: true,
            audio: false,
            video: false,
            reactions: false,
            replies: true,
            editing: false,
            deletion: false,
            typing_indicator: false,
            read_receipts: false,
            rich_text: true,
            max_message_length: 4000,
            max_attachment_size: 8 * 1024 * 1024,
            stream_protocol: crate::gateway::channel::StreamProtocol::None,
        }
    }
}

#[async_trait]
impl Channel for QQChannel {
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
            tracing::info!("QQ channel started in test mode");
            return Ok(());
        }

        if self.config.accounts.is_empty() {
            return Err(ChannelError::ConfigError(
                "No QQ accounts configured".to_string(),
            ));
        }

        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;
        tracing::info!("Starting QQ channel...");

        // Only the first account is ever started. Saying so out loud matters:
        // a `[[channels.qq.accounts]]` block past the first is a config the
        // operator wrote, saw accepted, and would otherwise never learn was
        // dropped.
        if self.config.accounts.len() > 1 {
            let ignored: Vec<&str> = self.config.accounts[1..]
                .iter()
                .map(|a| a.id.as_str())
                .collect();
            tracing::warn!(
                "QQ channel starts one account; ignoring {} further account(s): {}",
                ignored.len(),
                ignored.join(", "),
            );
        }
        let account = &self.config.accounts[0];
        if !account.enabled {
            return Err(ChannelError::ConfigError(
                "QQ account is disabled".to_string(),
            ));
        }

        let api = Arc::new(api::QQApiClient::new(
            account.app_id.clone(),
            account.client_secret.clone(),
        ));

        let token = api
            .get_access_token()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("QQ auth failed: {e}")))?;

        tracing::info!("QQ bot authenticated for account {}", account.id);
        self.api_client = Some(api);

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<QQEvent>(100);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let gateway_ctx = gateway::GatewayContext {
            channel_id: self.info.id.clone(),
            token,
            event_tx,
            shutdown_rx,
        };

        let gateway_handle = tokio::spawn(gateway::run_gateway(gateway_ctx));

        let channel_id = self.info.id.clone();
        let inbound_tx = self.channel_state.sender();
        let access = access::AccessController::new(account.clone());

        let event_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let Some(inbound) = handlers::convert_event(&event, &channel_id) {
                    let group_openid = if inbound.is_group {
                        inbound.conversation_id.as_str().strip_prefix("group:")
                    } else {
                        None
                    };

                    if access.check(inbound.sender_id.as_str(), group_openid)
                        == access::AccessDecision::Allow
                    {
                        if let Err(e) = inbound_tx.send(inbound) {
                            tracing::error!("Failed to send inbound QQ message: {:?}", e);
                        }
                    }
                }
            }
        });

        self.gateway_handle = Some(gateway_handle);
        self.event_handle = Some(event_handle);
        self.shutdown_tx = Some(shutdown_tx);
        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        tracing::info!("Stopping QQ channel...");
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.event_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.gateway_handle.take() {
            handle.abort();
        }
        self.api_client = None;
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        if self.test_mode {
            return Ok(SendResult {
                message_id: MessageId::new(format!(
                    "qq-test-{}",
                    chrono::Utc::now().timestamp_millis()
                )),
                timestamp: chrono::Utc::now(),
            });
        }

        let api = self
            .api_client
            .as_ref()
            .ok_or_else(|| ChannelError::NotConnected("QQ API not initialized".to_string()))?;
        delivery::deliver_message(api, &message, &self.reply_tracker).await
    }
}

pub struct QQChannelFactory;

#[async_trait]
impl ChannelFactory for QQChannelFactory {
    fn channel_type(&self) -> &str {
        "qq"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        // `from_wire` is the one place the two accepted `[channels.qq]`
        // spellings collapse into `accounts`; see its docs.
        let qq_config = QQConfig::from_wire(config).map_err(ChannelError::ConfigError)?;
        Ok(Box::new(QQChannel::new("qq", qq_config)))
    }
}

fn qq_factory_creator(
    _config: crate::gateway::channel::ChannelConfig,
) -> ChannelResult<Arc<dyn ChannelFactory>> {
    Ok(Arc::new(QQChannelFactory))
}

pub fn register_with_plugin() {
    let _ = crate::gateway::interfaces::plugin::register("qq", qq_factory_creator);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities() {
        let caps = QQChannel::capabilities();
        assert!(caps.images);
        assert_eq!(caps.max_message_length, 4000);
    }
}
