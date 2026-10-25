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
    task: Option<tokio::task::JoinHandle<()>>,
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
            task: None,
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

    pub async fn start(
        &mut self,
        channel_id: crate::gateway::channel::ChannelId,
        sender: InboundMessageSender,
    ) -> ChannelResult<()> {
        self.state.set(RuntimeState::Connecting);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        let ws_url = self.api.get_ws_endpoint().await.map_err(|e| {
            crate::gateway::channel::ChannelError::Internal(format!("WS endpoint failed: {e}"))
        })?;

        let ctx = WsLoopContext {
            initial_ws_url: ws_url,
            channel_id,
            config: self.config.clone(),
            bot_open_id: self.bot_open_id.clone(),
            sender,
            state: Arc::clone(&self.state),
            shutdown_rx,
            api: Arc::clone(&self.api),
            user_cache: Arc::clone(&self.user_cache),
        };

        self.task = Some(tokio::spawn(run_ws_loop(ctx)));
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), tokio::task::JoinError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        let result = if let Some(task) = self.task.take() {
            task.abort();
            match task.await {
                Err(error) if !error.is_cancelled() => Err(error),
                _ => Ok(()),
            }
        } else {
            Ok(())
        };
        self.state.set(RuntimeState::Disconnected);
        result
    }
}

impl Drop for FeishuRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::feishu::auth::TokenManager;
    use futures_util::StreamExt;
    use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn stop_joins_the_websocket_loop_and_closes_its_socket() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let ws_url = format!("ws://{}", listener.local_addr().unwrap());
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(stream).await.unwrap()
        });
        let server = MockServer::start().await;
        Mock::given(path("/open-apis/auth/v3/app_access_token/internal"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "msg": "ok", "app_access_token": "token", "expire": 7200
            })))
            .mount(&server)
            .await;
        Mock::given(path("/open-apis/callback/ws/endpoint"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 0, "msg": "ok", "data": {"URL": ws_url}
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let api = Arc::new(FeishuApi::new(
            Arc::new(TokenManager::new(
                "id",
                "secret",
                &server.uri(),
                http.clone(),
            )),
            &server.uri(),
            http,
        ));
        let config = serde_json::from_value(serde_json::json!({
            "app_id": "id", "app_secret": "secret"
        }))
        .unwrap();
        let mut runtime = FeishuRuntime::new(api, config, "bot".into());
        runtime
            .start(
                crate::gateway::channel::ChannelId::new("ws-stop"),
                crate::gateway::channel::ChannelState::new(8).sender(),
            )
            .await
            .unwrap();
        let mut socket = tokio::time::timeout(std::time::Duration::from_secs(2), peer)
            .await
            .unwrap()
            .unwrap();
        let task = runtime.task.as_ref().unwrap().abort_handle();
        runtime.stop().await.unwrap();
        assert!(task.is_finished());
        assert_eq!(runtime.connection_state(), RuntimeState::Disconnected);
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("joined WS loop must close its socket");
        assert!(closed.is_none() || closed.unwrap().is_err());
    }

    #[tokio::test]
    async fn stop_reports_a_panicked_ws_task() {
        let http = reqwest::Client::new();
        let api = Arc::new(FeishuApi::new(
            Arc::new(TokenManager::new(
                "id",
                "secret",
                "http://127.0.0.1:1",
                http.clone(),
            )),
            "http://127.0.0.1:1",
            http,
        ));
        let config = serde_json::from_value(serde_json::json!({
            "app_id": "id", "app_secret": "secret"
        }))
        .unwrap();
        let mut runtime = FeishuRuntime::new(api, config, "bot".into());
        runtime.task = Some(tokio::spawn(async { panic!("simulated WS panic") }));
        tokio::task::yield_now().await;
        assert!(runtime.stop().await.unwrap_err().is_panic());
        assert_eq!(runtime.connection_state(), RuntimeState::Disconnected);
    }
}
