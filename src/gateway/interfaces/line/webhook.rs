//! LINE Webhook Server
//!
//! Raw TCP socket webhook server with HMAC-SHA256 signature verification.
//! Follows the same pattern as Feishu's webhook implementation.

use crate::sync_primitives::Arc;
use base64::Engine as _;
use hmac::{digest::KeyInit, Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::gateway::channel::{
    ChannelId, ChannelStatus, ConversationId, InboundMessage, InboundMessageSender, MessageId,
    UserId,
};

use super::config::LineConfig;
use super::types::{LineEvent, LineSourceType};

type HmacSha256 = Hmac<Sha256>;

const WEBHOOK_BODY_LIMIT: usize = 64 * 1024;

pub struct WebhookContext {
    pub config: LineConfig,
    pub channel_id: ChannelId,
    pub sender: InboundMessageSender,
    pub status_handle: Arc<tokio::sync::RwLock<ChannelStatus>>,
}

impl Clone for WebhookContext {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            channel_id: self.channel_id.clone(),
            sender: self.sender.clone(),
            status_handle: self.status_handle.clone(),
        }
    }
}

/// Run the LINE webhook server.
pub async fn run_webhook_server(ctx: WebhookContext) {
    let addr = format!("{}:{}", ctx.config.webhook_host, ctx.config.webhook_port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("LINE webhook failed to bind {}: {}", addr, e);
            *ctx.status_handle.write().await = ChannelStatus::Error;
            return;
        }
    };
    tracing::info!("LINE webhook server listening on {}", addr);

    let semaphore = Arc::new(tokio::sync::Semaphore::new(100));

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let ctx = ctx.clone();
                let permit = match semaphore.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::warn!(
                            "LINE webhook connection limit reached, dropping connection"
                        );
                        continue;
                    }
                };
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) = handle_connection(stream, ctx).await {
                        tracing::warn!("LINE webhook connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                tracing::warn!("LINE webhook accept error: {}", e);
            }
        }
    }
}

struct ParsedRequest<'a> {
    path: &'a str,
    headers: Vec<(&'a str, &'a str)>,
    body: &'a [u8],
}

impl<'a> ParsedRequest<'a> {
    fn get_header(&self, name: &str) -> Option<&'a str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }
}

fn parse_http_request(data: &[u8]) -> Option<ParsedRequest<'_>> {
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

    let body = &data[body_start..];

    Some(ParsedRequest {
        path,
        headers,
        body,
    })
}

/// Verify HMAC-SHA256 signature from X-Line-Signature header.
fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();

    let sig_bytes = match base64::engine::general_purpose::STANDARD.decode(signature) {
        Ok(b) => b,
        Err(_) => return false,
    };

    crate::security::secret_equal_bytes(&expected, &sig_bytes)
}

/// Returns `true` once `data` holds the complete header section and, when a
/// Content-Length header is present, the complete body.
fn request_is_complete(data: &[u8]) -> bool {
    let Some(req) = parse_http_request(data) else {
        return false;
    };
    let expected = req
        .get_header("content-length")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    req.body.len() >= expected
}

async fn handle_connection(
    mut stream: TcpStream,
    ctx: WebhookContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Accumulate reads until the full request has arrived — TCP does not
    // preserve message boundaries and LINE signs the exact body bytes, so a
    // single read can truncate the body and fail signature verification.
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > WEBHOOK_BODY_LIMIT {
            let response = b"HTTP/1.1 413 Payload Too Large\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response).await?;
            return Ok(());
        }
        if request_is_complete(&buf) {
            break;
        }
    }
    if buf.is_empty() {
        return Ok(());
    }

    let data = &buf[..];

    let req = match parse_http_request(data) {
        Some(r) => r,
        None => return Ok(()),
    };

    if req.path == "/health" {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(response).await?;
        return Ok(());
    }

    if req.path != ctx.config.webhook_path {
        let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(response).await?;
        return Ok(());
    }

    let signature = match req.get_header("x-line-signature") {
        Some(s) => s,
        None => {
            let response = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response).await?;
            return Ok(());
        }
    };

    if !verify_signature(&ctx.config.channel_secret, req.body, signature) {
        tracing::warn!("LINE webhook signature verification failed");
        let response = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(response).await?;
        return Ok(());
    }
    // LINE delivers webhooks as an envelope: {"destination": ..., "events": [...]}.
    let payload: serde_json::Value = match serde_json::from_slice(req.body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Failed to parse LINE webhook body: {}", e);
            let response = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response).await?;
            return Ok(());
        }
    };

    let events = payload.get("events").and_then(|e| e.as_array()).cloned();
    if let Some(events) = events {
        for raw_event in events {
            match serde_json::from_value::<LineEvent>(raw_event.clone()) {
                Ok(event) => {
                    if let Err(e) = dispatch_event(&event, &ctx, &raw_event).await {
                        tracing::warn!("Failed to dispatch LINE event: {}", e);
                    }
                }
                // Unknown event types must not fail the rest of the batch.
                Err(e) => tracing::debug!("Skipping unparseable LINE event: {}", e),
            }
        }
    } else if let Ok(event) = serde_json::from_value::<LineEvent>(payload.clone()) {
        // Bare event body (non-envelope callers, e.g. local testing).
        if let Err(e) = dispatch_event(&event, &ctx, &payload).await {
            tracing::warn!("Failed to dispatch LINE event: {}", e);
        }
    } else {
        tracing::warn!("LINE webhook body has no events array");
    }

    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    stream.write_all(response).await?;

    Ok(())
}

async fn download_media(
    message: &super::types::LineMessage,
    config: &LineConfig,
) -> Result<Vec<crate::gateway::channel::Attachment>, String> {
    use super::types::LineMessage;
    use crate::gateway::channel::Attachment;
    let (mime, filename, size) = match message {
        LineMessage::Image(m) => {
            if let Some(url) = &m.content_provider.original_content_url {
                return Ok(vec![Attachment {
                    id: m.id.clone(),
                    mime_type: "image/jpeg".into(),
                    filename: None,
                    size: None,
                    url: Some(url.clone()),
                    path: None,
                    data: None,
                    is_voice_note: false,
                    is_video_note: false,
                }]);
            }
            ("image/jpeg", None, None)
        }
        LineMessage::Video(_) => ("video/mp4", None, None),
        LineMessage::Audio(_) => ("audio/mp4", None, None),
        LineMessage::File(m) => (
            "application/octet-stream",
            Some(m.file_name.clone()),
            Some(m.file_size),
        ),
        _ => return Ok(Vec::new()),
    };
    const LIMIT: usize = 50 * 1024 * 1024;
    let mut response = reqwest::Client::new()
        .get(format!(
            "https://api-data.line.me/v2/bot/message/{}/content",
            message.id()
        ))
        .bearer_auth(&config.channel_access_token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    if response.content_length().is_some_and(|n| n > LIMIT as u64) {
        return Err("LINE media exceeds 50 MiB".into());
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(mime)
        .to_owned();
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if chunk.len() > LIMIT - data.len() {
            return Err("LINE media exceeds 50 MiB".into());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(vec![Attachment {
        id: message.id().into(),
        mime_type: mime,
        filename,
        size: size.or(Some(data.len() as u64)),
        url: None,
        path: None,
        data: Some(data),
        is_voice_note: false,
        is_video_note: false,
    }])
}

async fn dispatch_event(
    event: &LineEvent,
    ctx: &WebhookContext,
    raw_event: &serde_json::Value,
) -> Result<(), String> {
    let (source, reply_token, text) = match event {
        LineEvent::Message {
            reply_token,
            source,
            message,
        } => {
            use super::types::LineMessage;
            let text = match message {
                LineMessage::Text(m) => m.text.clone(),
                LineMessage::Location(m) => format!(
                    "Location: {} — {} ({}, {})",
                    m.title, m.address, m.latitude, m.longitude
                ),
                LineMessage::Sticker(m) => format!("Sticker: {}/{}", m.package_id, m.sticker_id),
                LineMessage::Unknown => return Err("Unsupported LINE message type".into()),
                _ => "[LINE attachment]".into(),
            };
            (source, reply_token.clone(), text)
        }
        LineEvent::Follow {
            reply_token,
            source,
        } => (source, reply_token.clone(), "[Follow event]".to_string()),
        LineEvent::Unfollow { source } => (source, None, "[Unfollow event]".to_string()),
        LineEvent::Join {
            reply_token,
            source,
        } => (source, reply_token.clone(), "[Join event]".to_string()),
        LineEvent::Leave { source } => (source, None, "[Leave event]".to_string()),
        LineEvent::Postback {
            reply_token,
            source,
            postback,
        } => (
            source,
            reply_token.clone(),
            format!("[Postback: {}]", postback.data),
        ),
        LineEvent::Beacon {
            reply_token,
            source,
            beacon,
        } => (
            source,
            reply_token.clone(),
            format!("[Beacon: {}]", beacon.hwid),
        ),
        LineEvent::AccountLink { .. } => return Ok(()),
    };

    let is_group = source.source_type != LineSourceType::User;
    let conversation_id = if is_group {
        source
            .group_id
            .as_ref()
            .or(source.room_id.as_ref())
            .map_or_else(
                || ConversationId::new("unknown".to_string()),
                |s| ConversationId::new(s.clone()),
            )
    } else {
        source.user_id.as_ref().map_or_else(
            || ConversationId::new("unknown".to_string()),
            |s| ConversationId::new(s.clone()),
        )
    };

    let user_id = source.user_id.as_ref().map_or_else(
        || UserId::new("unknown".to_string()),
        |s| UserId::new(s.clone()),
    );

    let mut raw = raw_event.clone();
    if let Some(token) = reply_token {
        raw["reply_token"] = token.into();
    }
    let (id, attachments) = if let LineEvent::Message { message, .. } = event {
        (
            MessageId::new(message.id()),
            download_media(message, &ctx.config).await?,
        )
    } else {
        (MessageId::new(uuid::Uuid::new_v4().to_string()), Vec::new())
    };
    let inbound = InboundMessage {
        id,
        channel_id: ctx.channel_id.clone(),
        conversation_id,
        sender_id: user_id,
        sender_name: None,
        text,
        attachments,
        timestamp: chrono::Utc::now(),
        reply_to: None,
        is_group,
        raw: Some(raw),
        metadata: vec![],
    };

    ctx.sender
        .send(inbound)
        .map_err(|e| format!("Failed to send inbound message: {e:?}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inbound_media_preserves_real_identity_raw_and_resolvable_url() {
        let state = crate::gateway::channel::ChannelState::new(2);
        let mut rx = state.inbound_subscribe();
        let ctx = WebhookContext {
            config: LineConfig::default(),
            channel_id: ChannelId::new("line-instance"),
            sender: state.sender(),
            status_handle: Arc::new(tokio::sync::RwLock::new(ChannelStatus::Connected)),
        };
        let raw = serde_json::json!({"type":"message", "replyToken":"reply", "source":{"type":"user", "userId":"U1"},
            "message":{"type":"image", "id":"actual-id", "contentProvider":{"type":"external", "originalContentUrl":"https://example.org/image.jpg", "previewImageUrl":"https://example.org/preview.jpg"}}});
        let event: LineEvent = serde_json::from_value(raw.clone()).unwrap();
        dispatch_event(&event, &ctx, &raw).await.unwrap();
        let inbound = rx.recv().await.unwrap();
        assert_eq!(inbound.id.as_str(), "actual-id");
        assert!(!inbound.text.is_empty());
        assert_eq!(
            inbound.attachments[0].url.as_deref(),
            Some("https://example.org/image.jpg")
        );
        assert_eq!(inbound.raw.as_ref().unwrap()["message"], raw["message"]);
        assert_eq!(inbound.raw.as_ref().unwrap()["reply_token"], "reply");
        let unknown = serde_json::json!({"type":"message", "source":{"type":"user", "userId":"U1"}, "message":{"type":"future"}});
        let event: LineEvent = serde_json::from_value(unknown.clone()).unwrap();
        assert!(dispatch_event(&event, &ctx, &unknown).await.is_err());
    }

    fn compute_hmac_sha256(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let result = mac.finalize();
        base64::engine::general_purpose::STANDARD.encode(result.into_bytes())
    }

    #[test]
    fn test_verify_signature_valid() {
        let secret = "test_secret";
        let body = b"{\"type\":\"message\"}";
        let signature = compute_hmac_sha256(secret, body);
        assert!(verify_signature(secret, body, &signature));
    }

    #[test]
    fn test_verify_signature_invalid() {
        let secret = "test_secret";
        let body = b"{\"type\":\"message\"}";
        let wrong_sig = base64::engine::general_purpose::STANDARD.encode(b"wrong_signature");
        assert!(!verify_signature(secret, body, &wrong_sig));
    }

    #[test]
    fn test_verify_signature_tampered_body() {
        let secret = "test_secret";
        let original_body = b"{\"type\":\"message\"}";
        let tampered_body = b"{\"type\":\"unfollow\"}";
        let signature = compute_hmac_sha256(secret, original_body);
        assert!(!verify_signature(secret, tampered_body, &signature));
    }

    #[test]
    fn test_parse_http_request_simple() {
        let request = b"POST /line/webhook HTTP/1.1\r\nHost: localhost\r\nContent-Length: 13\r\nX-Line-Signature: abc123\r\n\r\n{\"type\":\"test\"}";
        let parsed = parse_http_request(request).unwrap();
        assert_eq!(parsed.path, "/line/webhook");
        assert_eq!(parsed.get_header("content-length").unwrap(), "13");
        assert_eq!(parsed.get_header("x-line-signature").unwrap(), "abc123");
        assert_eq!(parsed.body, b"{\"type\":\"test\"}");
    }

    #[test]
    fn test_parse_http_request_health() {
        let request = b"GET /health HTTP/1.1\r\n\r\n";
        let parsed = parse_http_request(request).unwrap();
        assert_eq!(parsed.path, "/health");
    }

    #[test]
    fn test_parse_http_request_headers_case_insensitive() {
        let request = b"POST /webhook HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\r\nbody";
        let parsed = parse_http_request(request).unwrap();
        assert_eq!(parsed.get_header("host").unwrap(), "localhost");
        assert_eq!(parsed.get_header("HOST").unwrap(), "localhost");
        assert_eq!(
            parsed.get_header("Content-Type").unwrap(),
            "application/json"
        );
    }
}
