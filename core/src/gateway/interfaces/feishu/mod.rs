pub mod config;
pub mod types;
pub mod events;
pub mod auth;
pub mod api;
pub mod client;
pub mod streaming;
pub mod dedup;
pub mod user_cache;
pub mod websocket;

use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use chrono::Utc;
use tokio::sync::watch;
use async_trait::async_trait;
use futures_util::StreamExt;

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelInfo, ChannelId,
    ChannelProvider, ChannelResult, ChannelState, ChannelStatus, ConversationId,
    InboundMessage, MessageId, OutboundMessage, SendResult, UserId,
};
use crate::thinker::interaction::{
    InteractionConstraints, InteractionManifest, InteractionParadigm,
};

pub use config::FeishuConfig;
use client::{FeishuClient, FeishuSendError};
use events::{extract_text_content, mark_bot_mentions, parse_ws_frame};
use types::{ChatType, FeishuEvent};

/// Determine if the response should use a Feishu card for markdown rendering.
fn should_use_card(text: &str, render_mode: &str) -> bool {
    match render_mode {
        "card" => true,
        "raw" => false,
        // "auto": use card for rich/long content
        _ => text.len() > 200
            || text.contains("```")
            || text.contains("|---|")
            || text.contains("|:--"),
    }
}

fn map_send_error(e: FeishuSendError) -> ChannelError {
    match e {
        FeishuSendError::RateLimited { retry_after_secs } => {
            ChannelError::RateLimited { retry_after_secs }
        }
        FeishuSendError::Other(msg) => ChannelError::SendFailed(msg),
    }
}

const DEDUP_CAPACITY: usize = 1000;

pub struct FeishuChannel {
    info: ChannelInfo,
    config: FeishuConfig,
    channel_state: ChannelState,
    client: Option<FeishuClient>,
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl FeishuChannel {
    pub fn new(id: impl Into<String>, config: FeishuConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "Feishu".to_string(),
            channel_type: "feishu".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::capabilities(),
        };

        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            client: None,
            shutdown_tx: None,
        }
    }

    fn capabilities() -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: true,
            audio: false,
            video: false,
            reactions: true,
            replies: true,
            editing: true,
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

    async fn start(&mut self) -> ChannelResult<()> {
        self.channel_state.set_status(ChannelStatus::Connecting).await;

        let client = FeishuClient::new(&self.config);
        client.refresh_token().await
            .map_err(|e| ChannelError::AuthFailed(format!("Token acquisition failed: {e}")))?;

        let bot_info = client.get_bot_info().await
            .map_err(|e| ChannelError::AuthFailed(format!("Bot info failed: {e}")))?;
        tracing::info!("Feishu bot connected: {:?}", bot_info.app_name);

        let bot_open_id = client.bot_open_id().await.unwrap_or_default();

        let ws_url = client.get_ws_endpoint().await
            .map_err(|e| ChannelError::Internal(format!("WS endpoint failed: {e}")))?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        client.spawn_token_refresh(shutdown_rx.clone());

        let sender = self.channel_state.sender();
        let channel_id = self.info.id.clone();
        let config = self.config.clone();
        let status_handle = self.channel_state.status_handle();
        let dedup = std::sync::Arc::new(StdMutex::new(VecDeque::<String>::with_capacity(DEDUP_CAPACITY)));
        let (ws_http, ws_base_url, ws_token) = client.ws_reconnect_handle();

        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            let mut backoff_secs: u64 = 1;
            let mut current_url = ws_url;

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }

                tracing::info!("Connecting to Feishu WebSocket: {}...", current_url.get(..60).unwrap_or(&current_url));

                match tokio_tungstenite::connect_async(&current_url).await {
                    Ok((ws_stream, _)) => {
                        backoff_secs = 1;
                        *status_handle.write().await = ChannelStatus::Connected;
                        tracing::info!("Feishu WebSocket connected");

                        let (_, mut read) = ws_stream.split();

                        loop {
                            tokio::select! {
                                msg = read.next() => {
                                    match msg {
                                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                            match parse_ws_frame(&text) {
                                                Ok(Some(FeishuEvent::MessageReceive {
                                                    message_id, chat_id, chat_type, sender_id,
                                                    sender_name, message_type, content, mut mentions,
                                                    parent_id, ..
                                                })) => {
                                                    {
                                                        let mut seen = dedup.lock().unwrap_or_else(|e| e.into_inner());
                                                        if seen.iter().any(|id| id == &message_id) {
                                                            continue;
                                                        }
                                                        if seen.len() >= DEDUP_CAPACITY {
                                                            seen.pop_front();
                                                        }
                                                        seen.push_back(message_id.clone());
                                                    }

                                                    mark_bot_mentions(&mut mentions, &bot_open_id);

                                                    if chat_type == ChatType::Group && config.require_mention {
                                                        let bot_mentioned = mentions.iter().any(|m| m.is_bot);
                                                        if !bot_mentioned {
                                                            continue;
                                                        }
                                                    }

                                                    if chat_type == ChatType::Group && !config.groups_allowed {
                                                        continue;
                                                    }

                                                    if chat_type == ChatType::P2p && !config.dm_allowed {
                                                        continue;
                                                    }

                                                    let extracted_text = match message_type.as_str() {
                                                        "text" => {
                                                            match extract_text_content(&content, &mentions) {
                                                                Some(t) => t,
                                                                None => continue,
                                                            }
                                                        }
                                                        "image" => "[Image]".to_string(),
                                                        other => {
                                                            tracing::debug!("Skipping unsupported message type: {other}");
                                                            continue;
                                                        }
                                                    };

                                                    let inbound = InboundMessage {
                                                        id: MessageId::new(&message_id),
                                                        channel_id: channel_id.clone(),
                                                        conversation_id: ConversationId::new(&chat_id),
                                                        sender_id: UserId::new(&sender_id),
                                                        sender_name,
                                                        text: extracted_text,
                                                        attachments: Vec::new(),
                                                        timestamp: Utc::now(),
                                                        reply_to: parent_id.map(MessageId::new),
                                                        is_group: chat_type == ChatType::Group,
                                                        raw: None,
                                                    };

                                                    if sender.send(inbound).await.is_err() {
                                                        tracing::warn!("Feishu inbound channel closed");
                                                        return;
                                                    }
                                                }
                                                Ok(Some(FeishuEvent::CardAction { .. })) => {
                                                    tracing::debug!("Received card action event (not yet handled)");
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

                *status_handle.write().await = ChannelStatus::Error;
                tracing::info!("Reconnecting in {backoff_secs}s...");

                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                    _ = shutdown_rx.changed() => {
                        tracing::info!("Feishu WS shutdown during backoff");
                        return;
                    }
                }

                backoff_secs = (backoff_secs * 2).min(60);

                // Re-fetch WS endpoint URL (old URLs may be expired)
                let token = ws_token.read().await.access_token.clone();
                let url = format!("{}/open-apis/callback/ws/endpoint", ws_base_url);
                match ws_http.post(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json; charset=utf-8")
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if let Ok(ws_resp) = resp.json::<types::WsEndpointResponse>().await {
                            if ws_resp.code == 0 {
                                if let Some(data) = ws_resp.data {
                                    current_url = data.url;
                                    tracing::debug!("Re-fetched WS endpoint URL");
                                }
                            }
                        }
                    }
                    Err(e) => tracing::warn!("Failed to re-fetch WS endpoint: {e}"),
                }
            }
        });

        self.client = Some(client);
        self.channel_state.set_status(ChannelStatus::Connected).await;

        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
        self.client = None;
        self.channel_state.set_status(ChannelStatus::Disconnected).await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let client = self.client.as_ref()
            .ok_or_else(|| ChannelError::NotConnected("Client not initialized".to_string()))?;

        let chat_id = message.conversation_id.as_str();
        let reply_to = message.reply_to.as_ref().map(|id| id.as_str());

        let has_image = message.attachments.iter().any(|a| a.mime_type.starts_with("image/"));

        let msg_id = if has_image {
            if let Some(attachment) = message.attachments.iter().find(|a| a.mime_type.starts_with("image/")) {
                let image_data = attachment.data.clone()
                    .ok_or_else(|| ChannelError::SendFailed("Image attachment has no data".to_string()))?;
                let filename = attachment.filename.as_deref().unwrap_or("image.png");
                let image_key = client.upload_image(image_data, filename).await
                    .map_err(ChannelError::SendFailed)?;
                client.send_image(chat_id, &image_key, reply_to).await
                    .map_err(map_send_error)?
            } else {
                unreachable!()
            }
        } else {
            if message.text.is_empty() {
                return Err(ChannelError::SendFailed("Empty message".to_string()));
            }
            if should_use_card(&message.text, &self.config.render_mode) {
                client.send_card(chat_id, &message.text, reply_to).await
                    .map_err(map_send_error)?
            } else {
                client.send_text(chat_id, &message.text, reply_to).await
                    .map_err(map_send_error)?
            }
        };

        if has_image && !message.text.is_empty() {
            let _ = client.send_text(chat_id, &message.text, reply_to).await;
        }

        Ok(SendResult {
            message_id: MessageId::new(msg_id),
            timestamp: Utc::now(),
        })
    }
}

impl ChannelProvider for FeishuChannel {
    fn interaction_manifest(&self) -> InteractionManifest {
        InteractionManifest::new(InteractionParadigm::Messaging)
            .with_constraints(
                InteractionConstraints::new()
                    .max_output_chars(4096)
                    .supports_streaming(self.config.streaming)
                    .prefer_compact(false),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::should_use_card;

    #[test]
    fn test_should_use_card_auto_plain() {
        assert!(!should_use_card("Hello world", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_code_block() {
        assert!(should_use_card("Here is code:\n```rust\nfn main() {}\n```", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_table() {
        assert!(should_use_card("| A |---|B |", "auto"));
    }

    #[test]
    fn test_should_use_card_auto_long() {
        let long_text = "a".repeat(201);
        assert!(should_use_card(&long_text, "auto"));
    }

    #[test]
    fn test_should_use_card_forced() {
        assert!(should_use_card("Hi", "card"));
    }

    #[test]
    fn test_should_use_card_raw() {
        assert!(!should_use_card("```code```", "raw"));
    }
}
