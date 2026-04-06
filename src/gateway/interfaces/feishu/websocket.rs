use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use chrono::Utc;
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};

use crate::gateway::channel::{
    ChannelId, ChannelStatus, ConversationId, InboundMessage, MessageId, UserId,
};

use super::api::FeishuApi;
use super::config::{FeishuConfig, GroupSessionScope};
use super::dedup::MessageDedup;
use super::events::{
    extract_text_content, mark_bot_mentions, parse_merge_forward_content, parse_ws_frame,
};
use super::types::{ChatType, FeishuEvent};
use super::user_cache::{UserProfile, UserProfileCache};

/// All context needed by the WS loop, passed as a single struct.
pub(super) struct WsLoopContext {
    pub(super) initial_ws_url: String,
    pub(super) channel_id: ChannelId,
    pub(super) config: FeishuConfig,
    pub(super) bot_open_id: String,
    pub(super) sender: mpsc::Sender<InboundMessage>,
    pub(super) status_handle: Arc<tokio::sync::RwLock<ChannelStatus>>,
    pub(super) shutdown_rx: watch::Receiver<bool>,
    pub(super) api: Arc<FeishuApi>,
    pub(super) user_cache: Arc<UserProfileCache>,
}

/// Run the WebSocket event loop with automatic reconnection.
pub(super) async fn run_ws_loop(ctx: WsLoopContext) {
    let dedup = Arc::new(StdMutex::new(MessageDedup::new()));
    let mut shutdown_rx = ctx.shutdown_rx;
    let mut backoff_secs: u64 = 1;
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
                backoff_secs = 1;
                *ctx.status_handle.write().await = ChannelStatus::Connected;
                tracing::info!("Feishu WebSocket connected");

                let (_, mut read) = ws_stream.split();

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                    handle_text_frame(
                                        &text,
                                        &dedup,
                                        &ctx.config,
                                        &ctx.bot_open_id,
                                        &ctx.channel_id,
                                        &ctx.sender,
                                        &ctx.user_cache,
                                        &ctx.api,
                                    ).await;
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

        *ctx.status_handle.write().await = ChannelStatus::Error;
        tracing::info!("Reconnecting in {backoff_secs}s...");

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = shutdown_rx.changed() => {
                tracing::info!("Feishu WS shutdown during backoff");
                return;
            }
        }

        backoff_secs = (backoff_secs * 2).min(60);

        // Re-fetch WS endpoint URL via FeishuApi (old URLs may be expired)
        match ctx.api.get_ws_endpoint().await {
            Ok(url) => {
                current_url = url;
                tracing::debug!("Re-fetched WS endpoint URL");
            }
            Err(e) => tracing::warn!("Failed to re-fetch WS endpoint: {e}"),
        }
    }
}

async fn handle_text_frame(
    text: &str,
    dedup: &Arc<StdMutex<MessageDedup>>,
    config: &FeishuConfig,
    bot_open_id: &str,
    channel_id: &ChannelId,
    sender: &mpsc::Sender<InboundMessage>,
    user_cache: &UserProfileCache,
    api: &FeishuApi,
) {
    match parse_ws_frame(text) {
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
            ..
        })) => {
            // Dedup check
            {
                let mut seen = dedup.lock().unwrap_or_else(|e| e.into_inner());
                if seen.is_duplicate(&message_id) {
                    return;
                }
            }

            mark_bot_mentions(&mut mentions, bot_open_id);

            if chat_type == ChatType::Group && config.require_mention {
                let bot_mentioned = mentions.iter().any(|m| m.is_bot);
                if !bot_mentioned {
                    return;
                }
            }

            if chat_type == ChatType::Group && !config.is_group_allowed(&chat_id) {
                return;
            }

            if chat_type == ChatType::P2p && !config.dm_allowed {
                return;
            }

            let extracted_text = match message_type.as_str() {
                "text" => match extract_text_content(&content, &mentions) {
                    Some(t) => t,
                    None => return,
                },
                "image" => "[Image]".to_string(),
                "merge_forward" => match api.get_message(&message_id).await {
                    Ok(msg) => {
                        let items_content = msg.content.unwrap_or_default();
                        parse_merge_forward_content(&items_content)
                    }
                    Err(e) => {
                        tracing::debug!("merge_forward fetch failed: {}", e);
                        "[Merged and Forwarded Message - fetch error]".to_string()
                    }
                },
                other => {
                    tracing::debug!("Skipping unsupported message type: {other}");
                    return;
                }
            };

            // Resolve sender name: try user_cache first, then API on miss
            let resolved_name = if let Some(name) = sender_name.clone() {
                Some(name)
            } else {
                user_cache.get_name(&sender_id, api).await
            };

            // Cache the sender profile if we have a name
            if let Some(ref name) = resolved_name {
                user_cache.insert(UserProfile {
                    open_id: sender_id.clone(),
                    name: Some(name.clone()),
                });
            }

            // Determine conversation_id based on group_session_scope
            let conversation_id = if chat_type == ChatType::Group {
                match config.group_session_scope {
                    GroupSessionScope::Group => chat_id.clone(),
                    GroupSessionScope::User => format!("{}:{}", chat_id, sender_id),
                    GroupSessionScope::Thread => {
                        if let Some(ref root) = parent_id {
                            format!("{}:{}", chat_id, root)
                        } else {
                            format!("{}:{}", chat_id, message_id)
                        }
                    }
                }
            } else {
                chat_id.clone()
            };

            let inbound = InboundMessage {
                id: MessageId::new(&message_id),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(&conversation_id),
                sender_id: UserId::new(&sender_id),
                sender_name: resolved_name,
                text: extracted_text,
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: parent_id.map(MessageId::new),
                is_group: chat_type == ChatType::Group,
                raw: None,
                metadata: vec![],
            };

            if sender.send(inbound).await.is_err() {
                tracing::warn!("Feishu inbound channel closed");
            }
        }
        Ok(Some(FeishuEvent::CardAction { .. })) => {
            tracing::debug!("Feishu card action (not handled in websocket)");
        }
        Ok(Some(FeishuEvent::BotAdded {
            chat_id,
            operator_id,
            ..
        })) => {
            tracing::info!("Feishu bot added to chat {chat_id} by {operator_id}");
        }
        Ok(Some(FeishuEvent::BotRemoved {
            chat_id,
            operator_id,
        })) => {
            tracing::info!("Feishu bot removed from chat {chat_id} by {operator_id:?}");
        }
        Ok(Some(FeishuEvent::ReactionCreated {
            message_id,
            chat_id,
            emoji,
            operator_id,
            ..
        })) => {
            if !config.reaction_notifications {
                return;
            }
            if operator_id == bot_open_id {
                return;
            }
            let original_msg = match api.get_message(&message_id).await {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::debug!("Failed to fetch reaction target message: {}", e);
                    return;
                }
            };
            let is_bot_message = original_msg
                .sender
                .as_ref()
                .and_then(|s| s.sender_id.as_ref())
                .and_then(|id| id.open_id.as_ref())
                .map(|id| id == bot_open_id)
                .unwrap_or(false);
            if !is_bot_message {
                tracing::debug!(
                    "Ignoring reaction on non-bot message {} by {}",
                    message_id,
                    operator_id
                );
                return;
            }
            let synthetic_id = format!("{}:reaction:{}", message_id, emoji);
            let content = format!("[reacted with {} to message {}]", emoji, message_id);
            let resolved_chat_id = chat_id
                .clone()
                .unwrap_or_else(|| format!("p2p:{}", operator_id));
            let conversation_id = resolved_chat_id.clone();
            let inbound = InboundMessage {
                id: MessageId::new(&synthetic_id),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(&conversation_id),
                sender_id: UserId::new(&operator_id),
                sender_name: None,
                text: content,
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: Some(MessageId::new(&message_id)),
                is_group: !resolved_chat_id.starts_with("p2p:"),
                raw: None,
                metadata: vec![],
            };
            if sender.send(inbound).await.is_err() {
                tracing::warn!("Feishu inbound channel closed");
            }
        }
        Ok(Some(FeishuEvent::ReactionDeleted {
            message_id,
            emoji,
            operator_id,
            ..
        })) => {
            tracing::debug!("Feishu reaction deleted: {emoji} on {message_id} by {operator_id}");
        }
        Ok(Some(FeishuEvent::BotMenu {
            event_key,
            operator_id,
            ..
        })) => {
            tracing::info!("Feishu bot menu event: {event_key} by {operator_id}");
        }
        Ok(Some(FeishuEvent::DriveComment {
            event_id,
            file_token,
            ..
        })) => {
            tracing::info!("Feishu drive comment: {event_id} on file {file_token}");
        }
        Ok(Some(FeishuEvent::Unknown(t))) => {
            tracing::debug!("Unknown Feishu event: {t}");
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!("Failed to parse Feishu WS frame: {e}");
        }
    }
}
