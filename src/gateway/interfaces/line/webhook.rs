//! LINE Webhook Server
//!
//! Raw TCP socket webhook server with HMAC-SHA256 signature verification.
//! Follows the same pattern as Feishu's webhook implementation.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
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
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("LINE webhook server listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
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

    if expected.len() != sig_bytes.len() {
        return false;
    }
    expected.iter().zip(sig_bytes.iter()).all(|(a, b)| a == b)
}

async fn handle_connection(
    mut stream: TcpStream,
    ctx: WebhookContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; WEBHOOK_BODY_LIMIT];

    let bytes_read = stream.read(&mut buf).await?;
    if bytes_read == 0 {
        return Ok(());
    }

    let data = &buf[..bytes_read];

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

    let event: LineEvent = match serde_json::from_slice(req.body) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to parse LINE event: {}", e);
            let response = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response).await?;
            return Ok(());
        }
    };

    if let Err(e) = dispatch_event(&event, &ctx).await {
        tracing::warn!("Failed to dispatch LINE event: {}", e);
    }

    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    stream.write_all(response).await?;

    Ok(())
}

async fn dispatch_event(event: &LineEvent, ctx: &WebhookContext) -> Result<(), String> {
    let (source, reply_token, text) = match event {
        LineEvent::Message {
            reply_token,
            source,
            message,
        } => {
            let text = message.text().unwrap_or("").to_string();
            (source, Some(reply_token.clone()), text)
        }
        LineEvent::Follow {
            reply_token,
            source,
        } => (
            source,
            Some(reply_token.clone()),
            "[Follow event]".to_string(),
        ),
        LineEvent::Unfollow { source } => (source, None, "[Unfollow event]".to_string()),
        LineEvent::Join {
            reply_token,
            source,
        } => (
            source,
            Some(reply_token.clone()),
            "[Join event]".to_string(),
        ),
        LineEvent::Leave { source } => (source, None, "[Leave event]".to_string()),
        LineEvent::Postback {
            reply_token,
            source,
            postback,
        } => (
            source,
            Some(reply_token.clone()),
            format!("[Postback: {}]", postback.data),
        ),
        LineEvent::Beacon {
            reply_token,
            source,
            beacon,
        } => (
            source,
            Some(reply_token.clone()),
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
            .map(|s| ConversationId::new(s.clone()))
            .unwrap_or_else(|| ConversationId::new("unknown".to_string()))
    } else {
        source
            .user_id
            .as_ref()
            .map(|s| ConversationId::new(s.clone()))
            .unwrap_or_else(|| ConversationId::new("unknown".to_string()))
    };

    let user_id = source
        .user_id
        .as_ref()
        .map(|s| UserId::new(s.clone()))
        .unwrap_or_else(|| UserId::new("unknown".to_string()));

    let raw = reply_token.as_ref().map(|token| {
        serde_json::json!({ "reply_token": token })
            .to_string()
            .into()
    });

    let inbound = InboundMessage {
        id: MessageId::new(format!("line_{}", chrono::Utc::now().timestamp_millis())),
        channel_id: ctx.channel_id.clone(),
        conversation_id,
        sender_id: user_id,
        sender_name: None,
        text,
        attachments: Vec::new(),
        timestamp: chrono::Utc::now(),
        reply_to: None,
        is_group,
        raw,
        metadata: vec![],
    };

    ctx.sender
        .send(inbound)
        .map_err(|e| format!("Failed to send inbound message: {:?}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
