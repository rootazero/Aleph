use crate::sync_primitives::{Arc, Mutex as StdMutex};

use futures_util::StreamExt;
use tokio::sync::watch;

use crate::gateway::channel::{ChannelId, InboundMessageSender};
use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::config::FeishuConfig;
use crate::gateway::interfaces::feishu::feishu_inbound::events::parse_ws_frame;
use crate::gateway::interfaces::feishu::feishu_inbound::{
    map_event_to_inbound, InboundPolicy, MessageDedup, UserProfileCache,
};
use crate::gateway::interfaces::feishu::feishu_runtime::state::{AtomicRuntimeState, RuntimeState};
use crate::gateway::restart_backoff::RestartBackoff;

pub struct WsLoopContext {
    pub initial_ws_url: String,
    pub channel_id: ChannelId,
    pub config: FeishuConfig,
    pub bot_open_id: String,
    pub sender: InboundMessageSender,
    pub state: Arc<AtomicRuntimeState>,
    pub shutdown_rx: watch::Receiver<bool>,
    pub api: Arc<FeishuApi>,
    pub user_cache: Arc<UserProfileCache>,
}

pub async fn run_ws_loop(ctx: WsLoopContext) {
    let dedup = Arc::new(StdMutex::new(MessageDedup::new()));
    let policy = InboundPolicy::new(ctx.config.clone(), ctx.bot_open_id.clone());
    let mut shutdown_rx = ctx.shutdown_rx;
    let mut backoff = RestartBackoff::with_defaults();
    let mut current_url = ctx.initial_ws_url;

    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        tracing::info!(
            "Connecting to Feishu WebSocket: {}...",
            current_url.chars().take(60).collect::<String>()
        );

        match tokio_tungstenite::connect_async(&current_url).await {
            Ok((ws_stream, _)) => {
                backoff.reset();
                ctx.state.set(RuntimeState::Connected);
                tracing::info!("Feishu WebSocket connected");

                let (_, mut read) = ws_stream.split();

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    match parse_ws_frame(&text) {
                                        Ok(Some(event)) => {
                                            if let Some(inbound) = map_event_to_inbound(
                                                &event,
                                                &ctx.channel_id,
                                                &ctx.config,
                                                &ctx.bot_open_id,
                                                &ctx.user_cache,
                                                &ctx.api,
                                            ) {
                                                let mut dedup_guard = dedup.lock().unwrap_or_else(|e| e.into_inner());
                                                if !dedup_guard.is_duplicate(inbound.id.as_str()) {
                                                    drop(dedup_guard);
                                                    match policy.evaluate(&inbound) {
                                                        crate::gateway::interfaces::feishu::feishu_inbound::InboundPolicyResult::Accept => {
                                                            let _ = ctx.sender.send(inbound);
                                                        }
                                                        crate::gateway::interfaces::feishu::feishu_inbound::InboundPolicyResult::Block(reason) => {
                                                            tracing::debug!(reason, "Inbound message blocked by policy");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(e) => tracing::debug!("Failed to parse WS frame: {e}"),
                                    }
                                }
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(_))) => {}
                                Some(Ok(_)) => {}
                                Some(Err(e)) => {
                                    tracing::warn!("Feishu WS error: {e}");
                                    break;
                                }
                                None => {
                                    tracing::info!("Feishu WS stream ended");
                                    break;
                                }
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            tracing::info!("Feishu WS shutdown signal received");
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Feishu WS connection failed: {e}");
            }
        }

        ctx.state.set(RuntimeState::Error);
        if let Some(delay) = backoff.next_delay() {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = shutdown_rx.changed() => {
                    tracing::info!("Feishu WS shutdown during backoff");
                    return;
                }
            }
        }

        if let Ok(new_url) = ctx.api.get_ws_endpoint().await {
            current_url = new_url;
        }
    }

    ctx.state.set(RuntimeState::Disconnected);
}
