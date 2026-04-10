use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::gateway::channel::{
    ChannelId, ChannelStatus, ConversationId, InboundMessage, InboundMessageSender, MessageId,
    UserId,
};

use super::api::FeishuApi;
use super::config::FeishuConfig;
use super::dedup::MessageDedup;
use super::events::{extract_text_content, mark_bot_mentions, parse_ws_frame};
use super::types::{ChatType, FeishuEvent};
use super::user_cache::{UserProfile, UserProfileCache};

type HmacSha256 = Hmac<Sha256>;

const WEBHOOK_BODY_LIMIT: usize = 64 * 1024;

pub struct WebhookContext {
    pub config: FeishuConfig,
    pub channel_id: ChannelId,
    pub bot_open_id: String,
    pub sender: InboundMessageSender,
    pub status_handle: Arc<tokio::sync::RwLock<ChannelStatus>>,
    pub api: Arc<FeishuApi>,
    pub user_cache: Arc<UserProfileCache>,
    pub dedup: Arc<StdMutex<MessageDedup>>,
}

impl Clone for WebhookContext {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            channel_id: self.channel_id.clone(),
            bot_open_id: self.bot_open_id.clone(),
            sender: self.sender.clone(),
            status_handle: self.status_handle.clone(),
            api: self.api.clone(),
            user_cache: self.user_cache.clone(),
            dedup: self.dedup.clone(),
        }
    }
}

pub async fn run_webhook_server(ctx: WebhookContext) {
    let addr = format!("{}:{}", ctx.config.webhook_host, ctx.config.webhook_port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("Feishu webhook server listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, ctx).await {
                        tracing::warn!("Webhook connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::warn!("Webhook accept error: {}", e);
            }
        }
    }
}

struct ParsedRequest<'a> {
    path: &'a str,
    headers: Vec<(&'a str, &'a str)>,
    body_start: usize,
}

fn parse_http_request(data: &[u8]) -> Option<ParsedRequest<'_>> {
    // Find body start by looking for \r\n\r\n sequence
    let body_separator = b"\r\n\r\n";
    let body_start = data
        .windows(4)
        .position(|window| window == body_separator)?
        + 4;

    let headers_section = &data[..body_start - 4];
    let headers_str = std::str::from_utf8(headers_section).ok()?;

    let mut lines = headers_str.split("\r\n");
    let request_line = lines.next()?;

    let parts: Vec<&str> = request_line.split(' ').collect();
    if parts.len() < 2 {
        return None;
    }

    let path = parts[1];

    let mut headers = Vec::new();
    for line in lines {
        if let Some(colon_pos) = line.find(':') {
            let name = line[..colon_pos].trim();
            let value = line[colon_pos + 1..].trim();
            headers.push((name, value));
        }
    }

    Some(ParsedRequest {
        path,
        headers,
        body_start,
    })
}

async fn handle_connection(
    mut stream: TcpStream,
    ctx: WebhookContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; WEBHOOK_BODY_LIMIT];

    loop {
        let bytes_read = stream.read(&mut buf).await?;
        if bytes_read == 0 {
            break;
        }

        let data = &buf[..bytes_read];

        let req = match parse_http_request(data) {
            Some(r) => r,
            None => continue,
        };

        if req.path == "/health" {
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response).await?;
            continue;
        }

        if req.path == "/verify" {
            let body = r#"{"challenge":""}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await?;
            continue;
        }

        if req.path != ctx.config.webhook_path {
            let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response).await?;
            continue;
        }

        let signature = req
            .headers
            .iter()
            .find(|(n, _)| *n == "x-lark-signature")
            .map(|(_, v)| *v);

        let timestamp = req
            .headers
            .iter()
            .find(|(n, _)| *n == "x-lark-request-timestamp")
            .map(|(_, v)| *v);

        let nonce = req
            .headers
            .iter()
            .find(|(n, _)| *n == "x-lark-request-nonce")
            .map(|(_, v)| *v);

        if let (Some(token), Some(sig), Some(ts), Some(n)) = (
            ctx.config.verification_token.as_deref(),
            signature,
            timestamp,
            nonce,
        ) {
            if !verify_signature(token, sig, ts, n, &ctx.config.encrypt_key) {
                let response = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
                stream.write_all(response).await?;
                continue;
            }
        }

        if req.body_start > WEBHOOK_BODY_LIMIT {
            let response = b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response).await?;
            continue;
        }

        let body_buf = &data[req.body_start..];

        let raw = match std::str::from_utf8(body_buf) {
            Ok(s) => s,
            Err(_) => {
                let response = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                stream.write_all(response).await?;
                continue;
            }
        };

        let event = match parse_ws_frame(raw) {
            Ok(Some(e)) => e,
            Ok(None) => {
                let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                stream.write_all(response).await?;
                continue;
            }
            Err(e) => {
                tracing::warn!("Failed to parse webhook payload: {}", e);
                let response = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                stream.write_all(response).await?;
                continue;
            }
        };

        let response = process_event(&ctx, event).await;
        stream.write_all(response.as_bytes()).await?;
    }

    Ok(())
}

async fn process_event(ctx: &WebhookContext, event: FeishuEvent) -> String {
    match event {
        FeishuEvent::MessageReceive {
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
        } => {
            {
                let mut seen = ctx.dedup.lock().unwrap_or_else(|e| e.into_inner());
                if seen.is_duplicate(&message_id) {
                    return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
                }
            }

            mark_bot_mentions(&mut mentions, &ctx.bot_open_id);

            if chat_type == ChatType::Group && ctx.config.require_mention {
                let bot_mentioned = mentions.iter().any(|m| m.is_bot);
                if !bot_mentioned {
                    return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
                }
            }

            if chat_type == ChatType::Group && !ctx.config.is_group_allowed(&chat_id) {
                return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
            }

            if chat_type == ChatType::P2p && !ctx.config.dm_allowed {
                return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
            }

            let extracted_text = match message_type.as_str() {
                "text" => match extract_text_content(&content, &mentions) {
                    Some(t) => t,
                    None => {
                        return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
                    }
                },
                "image" => "[Image]".to_string(),
                other => {
                    tracing::debug!("Skipping unsupported message type: {}", other);
                    return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
                }
            };

            let resolved_name =
                sender_name.or_else(|| ctx.user_cache.get(&sender_id).and_then(|p| p.name.clone()));

            if let Some(ref name) = resolved_name {
                ctx.user_cache.insert(UserProfile {
                    open_id: sender_id.clone(),
                    name: Some(name.clone()),
                });
            }

            let conversation_id = if chat_type == ChatType::Group {
                match ctx.config.group_session_scope {
                    super::config::GroupSessionScope::Group => chat_id.clone(),
                    super::config::GroupSessionScope::User => {
                        format!("{}:{}", chat_id, sender_id)
                    }
                    super::config::GroupSessionScope::Thread => {
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
                channel_id: ctx.channel_id.clone(),
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

            if ctx.sender.send(inbound).is_err() {
                tracing::warn!("Feishu inbound channel closed");
            }

            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::BotAdded { chat_id, .. } => {
            tracing::info!("Feishu bot added to chat {}", chat_id);
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::BotRemoved { chat_id, .. } => {
            tracing::info!("Feishu bot removed from chat {}", chat_id);
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::ReactionCreated {
            message_id,
            chat_id,
            emoji,
            operator_id,
        } => {
            if !ctx.config.reaction_notifications {
                return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
            }
            if operator_id == ctx.bot_open_id {
                return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
            }
            let original_msg = match ctx.api.get_message(&message_id).await {
                Ok(msg) => msg,
                Err(e) => {
                    tracing::debug!("Failed to fetch reaction target message: {}", e);
                    return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
                }
            };
            let is_bot_message = original_msg
                .sender
                .as_ref()
                .and_then(|s| s.sender_id.as_ref())
                .and_then(|id| id.open_id.as_ref())
                .map(|id| id.as_str() == ctx.bot_open_id.as_str())
                .unwrap_or(false);
            if !is_bot_message {
                tracing::debug!(
                    "Ignoring reaction on non-bot message {} by {}",
                    message_id,
                    operator_id
                );
                return "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string();
            }
            let synthetic_id = format!("{}:reaction:{}", message_id, emoji);
            let content = format!("[reacted with {} to message {}]", emoji, message_id);
            let resolved_chat_id = chat_id
                .clone()
                .unwrap_or_else(|| format!("p2p:{}", operator_id));
            let conversation_id = resolved_chat_id.clone();
            let inbound = InboundMessage {
                id: MessageId::new(&synthetic_id),
                channel_id: ctx.channel_id.clone(),
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
            if ctx.sender.send(inbound).is_err() {
                tracing::warn!("Feishu inbound channel closed");
            }
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::ReactionDeleted {
            message_id, emoji, ..
        } => {
            tracing::debug!("Feishu reaction deleted: {} on {}", emoji, message_id);
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::BotMenu {
            event_key,
            operator_id,
            ..
        } => {
            tracing::info!("Feishu bot menu event: {} by {}", event_key, operator_id);
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::DriveComment {
            event_id,
            file_token,
            ..
        } => {
            tracing::info!("Feishu drive comment: {} on file {}", event_id, file_token);
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::CardAction { .. } => {
            tracing::debug!("Feishu card action (not handled in webhook)");
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
        FeishuEvent::Unknown(t) => {
            tracing::debug!("Unknown Feishu webhook event: {}", t);
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
        }
    }
}

fn verify_signature(
    token: &str,
    signature: &str,
    timestamp: &str,
    nonce: &str,
    encrypt_key: &Option<String>,
) -> bool {
    let key = match encrypt_key.as_deref() {
        Some(k) => k,
        None => return true,
    };

    let payload = format!("{}{}{}{}", timestamp, nonce, key, token);

    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
    mac.update(payload.as_bytes());
    let result = hex::encode(mac.finalize().into_bytes());

    signature == result
}
