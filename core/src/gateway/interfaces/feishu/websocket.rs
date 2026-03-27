use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};

use chrono::Utc;
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch, RwLock};

use crate::gateway::channel::{
    ChannelId, ChannelStatus, ConversationId, InboundMessage, MessageId, UserId,
};
use super::events::{extract_text_content, mark_bot_mentions, parse_ws_frame};
use super::types::{ChatType, FeishuConfig, FeishuEvent, WsEndpointResponse};

const DEDUP_CAPACITY: usize = 1000;

// ── GroupSessionScope ──

/// Controls how conversation_id is computed for group messages.
#[derive(Debug, Clone, Copy, Default)]
pub enum GroupSessionScope {
    /// One session per group chat (default)
    #[default]
    Group,
    /// Separate sessions per topic thread within a group
    GroupTopic,
}

// ── MessageDedup ──

/// Ring-buffer deduplication for message IDs.
struct MessageDedup {
    seen: VecDeque<String>,
    capacity: usize,
}

impl MessageDedup {
    fn new() -> Self {
        Self {
            seen: VecDeque::with_capacity(DEDUP_CAPACITY),
            capacity: DEDUP_CAPACITY,
        }
    }

    /// Returns `true` if the key was already seen. Otherwise records it.
    fn is_duplicate(&mut self, key: &str) -> bool {
        if self.seen.iter().any(|id| id == key) {
            return true;
        }
        if self.seen.len() >= self.capacity {
            self.seen.pop_front();
        }
        self.seen.push_back(key.to_string());
        false
    }
}

// ── WsLoopContext ──

/// Bundles all parameters needed by the WebSocket reconnect loop.
pub struct WsLoopContext {
    /// HTTP client for re-fetching WS endpoints
    pub ws_http: reqwest::Client,
    /// Base URL (e.g. "https://open.feishu.cn")
    pub ws_base_url: String,
    /// Shared token state for Authorization header
    pub(super) ws_token: Arc<RwLock<super::client::TokenState>>,
    /// Initial WS URL obtained during start()
    pub initial_url: String,
    /// Channel configuration
    pub config: FeishuConfig,
    /// Sender half for inbound messages
    pub sender: mpsc::Sender<InboundMessage>,
    /// Channel identifier
    pub channel_id: ChannelId,
    /// Bot's own open_id for mention detection
    pub bot_open_id: String,
    /// Session scope strategy for groups
    pub group_session_scope: GroupSessionScope,
    /// Async handle to update channel status
    pub status_handle: Arc<RwLock<ChannelStatus>>,
    /// Shutdown signal receiver
    pub shutdown_rx: watch::Receiver<bool>,
}

// ── Main Loop ──

/// Run the WebSocket reconnect loop. Spawned as a tokio task from `FeishuChannel::start()`.
pub async fn run_ws_loop(mut ctx: WsLoopContext) {
    let dedup = Arc::new(StdMutex::new(MessageDedup::new()));
    let mut backoff_secs: u64 = 1;
    let mut current_url = ctx.initial_url.clone();

    loop {
        if *ctx.shutdown_rx.borrow() {
            break;
        }

        tracing::info!(
            "Connecting to Feishu WebSocket: {}...",
            current_url.get(..60).unwrap_or(&current_url)
        );

        match tokio_tungstenite::connect_async(&current_url).await {
            Ok((ws_stream, _)) => {
                backoff_secs = 1;
                *ctx.status_handle.write().await = ChannelStatus::Connected;
                tracing::info!("Feishu WebSocket connected");

                let (_, mut read) = ws_stream.split();

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    if let Some(inbound) = process_ws_message(&text, &ctx, &dedup).await {
                                        if ctx.sender.send(inbound).await.is_err() {
                                            tracing::warn!("Feishu inbound channel closed");
                                            return;
                                        }
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
                        _ = ctx.shutdown_rx.changed() => {
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

        *ctx.status_handle.write().await = ChannelStatus::Error;
        tracing::info!("Reconnecting in {backoff_secs}s...");

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = ctx.shutdown_rx.changed() => {
                tracing::info!("Feishu WS shutdown during backoff");
                return;
            }
        }

        backoff_secs = (backoff_secs * 2).min(60);

        // Re-fetch WS endpoint URL (old URLs may be expired)
        match refresh_ws_endpoint(&ctx.ws_http, &ctx.ws_base_url, &ctx.ws_token).await {
            Ok(url) => {
                current_url = url;
                tracing::debug!("Re-fetched WS endpoint URL");
            }
            Err(e) => tracing::warn!("Failed to re-fetch WS endpoint: {e}"),
        }
    }
}

// ── Single Message Processing ──

/// Process a single WS text frame. Returns an `InboundMessage` if it should be forwarded.
async fn process_ws_message(
    raw: &str,
    ctx: &WsLoopContext,
    dedup: &Arc<StdMutex<MessageDedup>>,
) -> Option<InboundMessage> {
    match parse_ws_frame(raw) {
        Ok(Some(FeishuEvent::MessageReceive {
            message_id,
            chat_id,
            chat_type,
            sender_id,
            sender_name,
            message_type,
            content,
            mut mentions,
            parent_id,
        })) => {
            // Dedup check
            {
                let mut guard = dedup.lock().unwrap_or_else(|e| e.into_inner());
                if guard.is_duplicate(&message_id) {
                    return None;
                }
            }

            mark_bot_mentions(&mut mentions, &ctx.bot_open_id);

            // Group mention filter
            if chat_type == ChatType::Group && ctx.config.require_mention {
                let bot_mentioned = mentions.iter().any(|m| m.is_bot);
                if !bot_mentioned {
                    return None;
                }
            }

            // Group/DM access filters
            if chat_type == ChatType::Group && !ctx.config.groups_allowed {
                return None;
            }
            if chat_type == ChatType::P2p && !ctx.config.dm_allowed {
                return None;
            }

            // Extract text
            let extracted_text = match message_type.as_str() {
                "text" => extract_text_content(&content, &mentions)?,
                "image" => "[Image]".to_string(),
                other => {
                    tracing::debug!("Skipping unsupported message type: {other}");
                    return None;
                }
            };

            // Compute conversation_id based on session scope
            let conversation_id = compute_conversation_id(
                &chat_id,
                chat_type,
                parent_id.as_deref(),
                ctx.group_session_scope,
            );

            Some(InboundMessage {
                id: MessageId::new(&message_id),
                channel_id: ctx.channel_id.clone(),
                conversation_id: ConversationId::new(conversation_id),
                sender_id: UserId::new(&sender_id),
                sender_name,
                text: extracted_text,
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: parent_id.map(MessageId::new),
                is_group: chat_type == ChatType::Group,
                raw: None,
            })
        }
        Ok(Some(FeishuEvent::Unknown(t))) => {
            tracing::debug!("Unknown Feishu event: {t}");
            None
        }
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("Failed to parse Feishu WS frame: {e}");
            None
        }
    }
}

// ── Helpers ──

/// Compute conversation_id based on GroupSessionScope.
fn compute_conversation_id(
    chat_id: &str,
    chat_type: ChatType,
    parent_id: Option<&str>,
    scope: GroupSessionScope,
) -> String {
    match scope {
        GroupSessionScope::Group => chat_id.to_string(),
        GroupSessionScope::GroupTopic => {
            if chat_type == ChatType::Group {
                if let Some(topic_id) = parent_id {
                    return format!("{chat_id}:topic:{topic_id}");
                }
            }
            chat_id.to_string()
        }
    }
}

/// Re-fetch the WebSocket endpoint URL using the current token.
async fn refresh_ws_endpoint(
    http: &reqwest::Client,
    base_url: &str,
    token_state: &Arc<RwLock<super::client::TokenState>>,
) -> Result<String, String> {
    let token = token_state.read().await.access_token.clone();
    let url = format!("{base_url}/open-apis/callback/ws/endpoint");

    let resp = http
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| format!("WS endpoint request failed: {e}"))?;

    let ws_resp: WsEndpointResponse = resp
        .json()
        .await
        .map_err(|e| format!("WS endpoint response parse failed: {e}"))?;

    if ws_resp.code != 0 {
        return Err(format!(
            "WS endpoint error: code={}, msg={}",
            ws_resp.code, ws_resp.msg
        ));
    }

    ws_resp
        .data
        .map(|d| d.url)
        .ok_or_else(|| "No data in WS endpoint response".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_new_key() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg_001"));
    }

    #[test]
    fn test_dedup_duplicate_key() {
        let mut dedup = MessageDedup::new();
        assert!(!dedup.is_duplicate("msg_001"));
        assert!(dedup.is_duplicate("msg_001"));
    }

    #[test]
    fn test_dedup_capacity_eviction() {
        let mut dedup = MessageDedup::new();
        // Fill to capacity
        for i in 0..DEDUP_CAPACITY {
            assert!(!dedup.is_duplicate(&format!("msg_{i}")));
        }
        // All slots full — adding a new key evicts the oldest (msg_0)
        assert!(!dedup.is_duplicate("msg_new"));
        // msg_0 was evicted, so it should no longer be a duplicate
        assert!(!dedup.is_duplicate("msg_0"));
        // msg_1 was evicted by the msg_0 re-insertion above
        assert!(!dedup.is_duplicate("msg_1"));
    }

    #[test]
    fn test_conversation_id_group_scope() {
        let id = compute_conversation_id("oc_chat1", ChatType::Group, Some("msg_root"), GroupSessionScope::Group);
        assert_eq!(id, "oc_chat1");
    }

    #[test]
    fn test_conversation_id_group_topic_with_parent() {
        let id = compute_conversation_id("oc_chat1", ChatType::Group, Some("msg_root"), GroupSessionScope::GroupTopic);
        assert_eq!(id, "oc_chat1:topic:msg_root");
    }

    #[test]
    fn test_conversation_id_group_topic_without_parent() {
        let id = compute_conversation_id("oc_chat1", ChatType::Group, None, GroupSessionScope::GroupTopic);
        assert_eq!(id, "oc_chat1");
    }

    #[test]
    fn test_conversation_id_p2p_topic_scope() {
        // P2P messages always use chat_id regardless of scope
        let id = compute_conversation_id("oc_p2p1", ChatType::P2p, Some("msg_root"), GroupSessionScope::GroupTopic);
        assert_eq!(id, "oc_p2p1");
    }
}
