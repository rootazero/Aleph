pub mod state;
pub mod ws_client;

pub use state::{AtomicRuntimeState, RuntimeState};
pub use ws_client::{run_ws_loop, WsLoopContext};

use crate::sync_primitives::Arc;

use tokio::sync::watch;

use crate::gateway::channel::{ChannelResult, InboundMessageSender};
use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::config::FeishuConfig;
use crate::gateway::interfaces::feishu::feishu_inbound::UserProfileCache;

pub struct FeishuRuntime {
    state: Arc<AtomicRuntimeState>,
    api: Arc<FeishuApi>,
    config: FeishuConfig,
    bot_open_id: String,
    user_cache: Arc<UserProfileCache>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl FeishuRuntime {
    pub fn new(api: Arc<FeishuApi>, config: FeishuConfig, bot_open_id: String) -> Self {
        Self {
            state: Arc::new(AtomicRuntimeState::new(RuntimeState::Disconnected)),
            api,
            config,
            bot_open_id,
            user_cache: Arc::new(UserProfileCache::new()),
            shutdown_tx: None,
        }
    }

    #[must_use]
    pub fn connection_state(&self) -> RuntimeState {
        self.state.get()
    }

    #[must_use]
    pub fn state_handle(&self) -> Arc<AtomicRuntimeState> {
        Arc::clone(&self.state)
    }

    pub async fn start(&mut self, sender: InboundMessageSender) -> ChannelResult<()> {
        self.state.set(RuntimeState::Connecting);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let ws_url = self.api.get_ws_endpoint().await.map_err(|e| {
            crate::gateway::channel::ChannelError::Internal(format!("WS endpoint failed: {e}"))
        })?;

        let ctx = WsLoopContext {
            initial_ws_url: ws_url,
            channel_id: crate::gateway::channel::ChannelId::new(&self.config.app_id),
            config: self.config.clone(),
            bot_open_id: self.bot_open_id.clone(),
            sender,
            state: Arc::clone(&self.state),
            shutdown_rx,
            api: Arc::clone(&self.api),
            user_cache: Arc::clone(&self.user_cache),
        };

        tokio::spawn(run_ws_loop(ctx));
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        self.state.set(RuntimeState::Disconnected);
    }
}
