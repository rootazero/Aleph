//! Matrix API Operations
//!
//! Low-level functions for interacting with the Matrix Client-Server API v3.
//! These are separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use crate::sync_primitives::Arc;
use chrono::Utc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::config::MatrixConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Matrix message length limit (characters).
pub(crate) const MATRIX_MSG_LIMIT: usize = 65535;
/// Matrix media upload maximum size (100MB).
pub const MATRIX_MEDIA_MAX_SIZE: u64 = 100 * 1024 * 1024;

/// Matrix message operations helper.
///
/// Provides methods for sending messages and interacting with the Matrix Client-Server API v3.
pub struct MatrixMessageOps;

impl MatrixMessageOps {
    /// Validate access token via `/_matrix/client/v3/account/whoami` and return the user ID.
    pub async fn validate_token(
        client: &reqwest::Client,
        homeserver: &str,
        token: &str,
    ) -> Result<String, ChannelError> {
        let url = format!("{homeserver}/_matrix/client/v3/account/whoami");

        let resp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("whoami request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::AuthFailed(format!(
                "Matrix authentication failed ({status}): {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("whoami response parse failed: {e}")))?;

        let user_id = body["user_id"].as_str().unwrap_or("unknown").to_string();

        Ok(user_id)
    }

    /// Send a text message to a Matrix room.
    ///
    /// Uses `PUT /_matrix/client/v3/rooms/{room_id}/send/m.room.message/{txn_id}`.
    /// Formats the message body as `org.matrix.custom.html` using Markdown.
    /// Automatically splits long messages.
    pub async fn send_message(
        client: &reqwest::Client,
        homeserver: &str,
        token: &str,
        room_id: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> Result<SendResult, ChannelError> {
        // Format text as HTML for Matrix
        let formatted_body = MessageFormatter::format(text, MarkupFormat::Markdown);
        let chunks = MessageFormatter::split(&formatted_body, MATRIX_MSG_LIMIT);

        let mut last_result = None;

        for chunk in &chunks {
            let txn_id = uuid::Uuid::new_v4().to_string();
            let url = format!(
                "{homeserver}/_matrix/client/v3/rooms/{room_id}/send/m.room.message/{txn_id}"
            );

            let mut body = serde_json::json!({
                "msgtype": "m.text",
                "body": chunk,
                "format": "org.matrix.custom.html",
                "formatted_body": chunk,
            });

            // Add reply relation if provided
            if let Some(event_id) = reply_to {
                body["m.relates_to"] = serde_json::json!({
                    "m.in_reply_to": {
                        "event_id": event_id
                    }
                });
            }

            let resp = client
                .put(&url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Matrix send failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                return Err(ChannelError::SendFailed(format!(
                    "Matrix send failed ({status}): {resp_body}"
                )));
            }

            let resp_json: serde_json::Value = resp.json().await.map_err(|e| {
                ChannelError::SendFailed(format!("Matrix send response parse failed: {e}"))
            })?;

            let event_id = resp_json["event_id"]
                .as_str()
                .unwrap_or(&txn_id)
                .to_string();

            last_result = Some(SendResult {
                message_id: MessageId::new(event_id),
                timestamp: Utc::now(),
            });
        }

        last_result.ok_or_else(|| ChannelError::SendFailed("No message chunks to send".to_string()))
    }

    /// Send a reaction (annotation) to a Matrix message.
    ///
    /// Uses `PUT /_matrix/client/v3/rooms/{room_id}/send/m.reaction/{txn_id}`.
    /// Matrix reactions are annotations on existing events.
    pub async fn send_reaction(
        client: &reqwest::Client,
        homeserver: &str,
        token: &str,
        room_id: &str,
        event_id: &str,
        reaction: &str,
    ) -> Result<(), ChannelError> {
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "{homeserver}/_matrix/client/v3/rooms/{room_id}/send/m.reaction/{txn_id}"
        );

        let body = serde_json::json!({
            "m.relates_to": {
                "rel_type": "m.annotation",
                "event_id": event_id,
                "key": reaction
            }
        });

        let resp = client
            .put(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Matrix reaction failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::SendFailed(format!(
                "Matrix reaction failed ({status}): {resp_body}"
            )));
        }

        Ok(())
    }

    /// Delete a message in a Matrix room via redaction.
    ///
    /// Uses `PUT /_matrix/client/v3/rooms/{room_id}/redact/{event_id}/{txn_id}`.
    pub async fn delete_message(
        client: &reqwest::Client,
        homeserver: &str,
        token: &str,
        room_id: &str,
        event_id: &str,
    ) -> Result<(), ChannelError> {
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "{homeserver}/_matrix/client/v3/rooms/{room_id}/redact/{event_id}/{txn_id}"
        );

        let body = serde_json::json!({});

        let resp = client
            .put(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Matrix delete failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::SendFailed(format!(
                "Matrix delete failed ({status}): {resp_body}"
            )));
        }

        Ok(())
    }

    /// Edit an existing message in a Matrix room.
    ///
    /// Uses `PUT /_matrix/client/v3/rooms/{room_id}/send/m.room.message/{txn_id}` with
    /// `m.relates_to` containing `rel_type: "m.replace"` to signal an edit.
    /// Formats the new text body as `org.matrix.custom.html` using Markdown.
    pub async fn edit_message(
        client: &reqwest::Client,
        homeserver: &str,
        token: &str,
        room_id: &str,
        event_id: &str,
        new_text: &str,
    ) -> Result<SendResult, ChannelError> {
        let formatted_body = MessageFormatter::format(new_text, MarkupFormat::Markdown);
        let chunks = MessageFormatter::split(&formatted_body, MATRIX_MSG_LIMIT);

        if chunks.is_empty() {
            return Err(ChannelError::SendFailed("No message content to send".to_string()));
        }

        let chunk = &chunks[0];
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "{homeserver}/_matrix/client/v3/rooms/{room_id}/send/m.room.message/{txn_id}"
        );

        let body = serde_json::json!({
            "msgtype": "m.text",
            "body": format!("* {}", chunk),
            "format": "org.matrix.custom.html",
            "formatted_body": chunk,
            "m.relates_to": {
                "rel_type": "m.replace",
                "event_id": event_id
            }
        });

        let resp = client
            .put(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Matrix edit failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::SendFailed(format!(
                "Matrix edit failed ({status}): {resp_body}"
            )));
        }

        let resp_json: serde_json::Value = resp.json().await.map_err(|e| {
            ChannelError::SendFailed(format!("Matrix edit response parse failed: {e}"))
        })?;

        let event_id = resp_json["event_id"]
            .as_str()
            .unwrap_or(&txn_id)
            .to_string();

        Ok(SendResult {
            message_id: MessageId::new(event_id),
            timestamp: Utc::now(),
        })
    }

    /// Upload media to Matrix Content Repository.
    ///
    /// Uses `POST /_matrix/media/v3/upload`.
    /// Returns the `mxc://` URI for the uploaded content.
    pub async fn upload_media(
        client: &reqwest::Client,
        homeserver: &str,
        token: &str,
        content: Vec<u8>,
        mime_type: &str,
        filename: Option<&str>,
    ) -> Result<String, ChannelError> {
        let url = format!("{homeserver}/_matrix/media/v3/upload");

        let mut req = client.post(&url).bearer_auth(token);

        if let Some(name) = filename {
            req = req.query(&[("filename", name)]);
        }

        let resp = req
            .header("Content-Type", mime_type)
            .body(content)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Matrix media upload failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::SendFailed(format!(
                "Matrix media upload failed ({status}): {resp_body}"
            )));
        }

        let resp_json: serde_json::Value = resp.json().await.map_err(|e| {
            ChannelError::SendFailed(format!("Matrix media upload response parse failed: {e}"))
        })?;

        let mxc_uri = resp_json["content_uri"]
            .as_str()
            .ok_or_else(|| ChannelError::SendFailed("No content_uri in media upload response".to_string()))?;

        Ok(mxc_uri.to_string())
    }

    /// Download media from Matrix Content Repository.
    ///
    /// Uses `GET /_matrix/media/v3/download/{serverName}/{mediaId}`.
    /// Returns the media content and Content-Type header.
    pub async fn download_media(
        client: &reqwest::Client,
        homeserver: &str,
        mxc_uri: &str,
    ) -> Result<(Vec<u8>, String), ChannelError> {
        // Parse mxc://serverName/mediaId
        let mxc_uri = mxc_uri.trim();
        if !mxc_uri.starts_with("mxc://") {
            return Err(ChannelError::ReceiveFailed(
                "Invalid mxc:// URI format".to_string(),
            ));
        }

        let path = &mxc_uri[6..];
        let (server_name, media_id) = path
            .split_once('/')
            .ok_or_else(|| ChannelError::ReceiveFailed("Invalid mxc:// URI path".to_string()))?;

        let encoded_server = percent_encoding::percent_encode(
            server_name.as_bytes(),
            percent_encoding::NON_ALPHANUMERIC,
        );
        let encoded_media = percent_encoding::percent_encode(
            media_id.as_bytes(),
            percent_encoding::NON_ALPHANUMERIC,
        );

        let url = format!(
            "{homeserver}/_matrix/media/v3/download/{}/{}",
            encoded_server, encoded_media
        );

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ChannelError::ReceiveFailed(format!("Matrix media download failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::ReceiveFailed(format!(
                "Matrix media download failed ({status}): {resp_body}"
            )));
        }

        let content_type = resp
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        if let Some(len) = resp
            .headers()
            .get("Content-Length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
        {
            if len > MATRIX_MEDIA_MAX_SIZE {
                return Err(ChannelError::ReceiveFailed(format!(
                    "Media file too large: {} bytes (max {})",
                    len, MATRIX_MEDIA_MAX_SIZE
                )));
            }
        }

        let bytes = resp.bytes().await.map_err(|e| {
            ChannelError::ReceiveFailed(format!("Matrix media download body read failed: {e}"))
        })?;

        if bytes.len() as u64 > MATRIX_MEDIA_MAX_SIZE {
            return Err(ChannelError::ReceiveFailed(format!(
                "Media file too large: {} bytes (max {})",
                bytes.len(), MATRIX_MEDIA_MAX_SIZE
            )));
        }

        Ok((bytes.to_vec(), content_type))
    }

    /// Send a typing indicator to a Matrix room.
    ///
    /// Uses `PUT /_matrix/client/v3/rooms/{room_id}/typing/{user_id}`.
    pub async fn send_typing(
        client: &reqwest::Client,
        homeserver: &str,
        token: &str,
        room_id: &str,
        user_id: &str,
        typing: bool,
    ) -> Result<(), ChannelError> {
        let url = format!("{homeserver}/_matrix/client/v3/rooms/{room_id}/typing/{user_id}");

        let body = if typing {
            serde_json::json!({
                "typing": true,
                "timeout": 10000,
            })
        } else {
            serde_json::json!({
                "typing": false,
            })
        };

        let resp = client
            .put(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Matrix typing indicator failed: {e}")))?;

        if !resp.status().is_success() {
            tracing::warn!("Matrix typing indicator returned {}", resp.status());
        }

        Ok(())
    }

    /// Convert a Matrix room event to an `InboundMessage`.
    ///
    /// Returns `None` if the event should be ignored (own message,
    /// filtered room, non-message event, policy-gated, etc.).
    pub fn convert_room_event(
        event: &serde_json::Value,
        room_id: &str,
        channel_id: &ChannelId,
        own_user_id: &str,
        config: Option<&MatrixConfig>,
    ) -> Option<InboundMessage> {
        // Only process m.room.message events
        let event_type = event["type"].as_str()?;
        if event_type != "m.room.message" {
            return None;
        }

        let sender = event["sender"].as_str()?;

        // Skip own messages
        if sender == own_user_id {
            return None;
        }

        // Apply user allowlist policy
        if let Some(cfg) = config {
            if !cfg.is_user_allowed(sender) {
                return None;
            }
        }

        let content = &event["content"];
        let body = content["body"].as_str().unwrap_or("");
        if body.is_empty() {
            return None;
        }

        // Apply mention gating policy
        if let Some(cfg) = config {
            if !cfg.check_mention(body, own_user_id) {
                return None;
            }
        }

        let event_id = event["event_id"].as_str().unwrap_or("").to_string();

        // Parse timestamp from origin_server_ts (milliseconds since epoch)
        let timestamp = event["origin_server_ts"]
            .as_i64()
            .and_then(|ms| {
                chrono::DateTime::from_timestamp(ms / 1000, ((ms % 1000) * 1_000_000) as u32)
            })
            .unwrap_or_else(Utc::now);

        // Extract reply-to and thread root from m.relates_to
        let relates_to = content["m.relates_to"].as_object();
        let reply_to = relates_to
            .and_then(|r| r.get("m.in_reply_to"))
            .and_then(|ir| ir.get("event_id"))
            .and_then(|eid| eid.as_str())
            .map(|id| MessageId::new(id.to_string()));

        // Extract thread root from m.relates_to when rel_type is "m.thread"
        // This is stored in metadata for downstream processing
        let thread_root = relates_to
            .and_then(|r| r.get("rel_type"))
            .and_then(|rt| if rt.as_str()? == "m.thread" { Some(()) } else { None })
            .and_then(|_| relates_to)
            .and_then(|r| r.get("event_id"))
            .and_then(|eid| eid.as_str())
            .map(|id| MessageId::new(id.to_string()));

        let mut metadata = Vec::new();
        if let Some(root) = thread_root {
            metadata.push(crate::gateway::channel::MessageMeta::ThreadRoot(root));
        }

        // Matrix rooms are always group conversations
        let is_group = true;

        Some(InboundMessage {
            id: MessageId::new(event_id),
            channel_id: channel_id.clone(),
            conversation_id: ConversationId::new(room_id.to_string()),
            sender_id: UserId::new(sender.to_string()),
            sender_name: Some(sender.to_string()),
            text: body.to_string(),
            attachments: Vec::new(),
            timestamp,
            reply_to,
            is_group,
            raw: Some(event.clone()),
            metadata,
        })
    }

    /// Run the /sync long-polling loop.
    ///
    /// This function runs indefinitely until a shutdown signal is received.
    /// It handles:
    /// - Building /sync URL with timeout and since token
    /// - Long-polling with bearer auth
    /// - Parsing `rooms.join.{room_id}.timeline.events`
    /// - Filtering by allowed_rooms
    /// - Processing m.room.message events
    /// - Updating since_token from next_batch
    /// - Exponential backoff on errors
    pub async fn run_sync_loop(
        client: reqwest::Client,
        config: MatrixConfig,
        user_id: Arc<RwLock<Option<String>>>,
        since_token: Arc<RwLock<Option<String>>>,
        channel_id: ChannelId,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut backoff = INITIAL_BACKOFF;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            // Build /sync URL
            let since = since_token.read().await.clone();
            let mut url = format!(
                "{}/_matrix/client/v3/sync?timeout={}&filter={{\"room\":{{\"timeline\":{{\"limit\":10}}}}}}",
                config.homeserver_url, config.sync_timeout_ms
            );
            if let Some(ref token) = since {
                url.push_str(&format!("&since={token}"));
            }

            let resp = tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("Matrix sync loop shutting down");
                        break;
                    }
                    continue;
                }
                result = client.get(&url).bearer_auth(&config.access_token).send() => {
                    match result {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!("Matrix sync error: {e}, retrying in {backoff:?}");
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                            continue;
                        }
                    }
                }
            };

            if !resp.status().is_success() {
                tracing::warn!("Matrix sync returned {}", resp.status());
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }

            // Reset backoff on success
            backoff = INITIAL_BACKOFF;

            let body: serde_json::Value = match resp.json().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("Matrix sync parse error: {e}");
                    continue;
                }
            };

            // Update since token from next_batch
            if let Some(next) = body["next_batch"].as_str() {
                *since_token.write().await = Some(next.to_string());
            }

            // Get own user ID for filtering
            let own_user_id = user_id.read().await.clone().unwrap_or_default();

            // Process room events from rooms.join
            if let Some(rooms) = body["rooms"]["join"].as_object() {
                for (room_id, room_data) in rooms {
                    // Filter by allowed rooms
                    if !config.is_room_allowed(room_id) {
                        continue;
                    }

                    if let Some(events) = room_data["timeline"]["events"].as_array() {
                        for event in events {
                            if let Some(inbound) =
                                Self::convert_room_event(event, room_id, &channel_id, &own_user_id, Some(&config))
                            {
                                tracing::debug!(
                                    "Matrix message from {} in {}: {}",
                                    inbound.sender_id.as_str(),
                                    room_id,
                                    &inbound.text[..inbound.text.len().min(50)]
                                );
                                if inbound_tx.send(inbound).await.is_err() {
                                    tracing::error!("Matrix: inbound channel closed");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }

        tracing::info!("Matrix sync loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_basic_message() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$event123",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello from Matrix!"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.channel_id.as_str(), "matrix");
        assert_eq!(msg.conversation_id.as_str(), "!room1:matrix.org");
        assert_eq!(msg.sender_id.as_str(), "@user:matrix.org");
        assert_eq!(msg.text, "Hello from Matrix!");
        assert_eq!(msg.id.as_str(), "$event123");
        assert!(msg.is_group);
    }

    #[test]
    fn test_convert_filters_own_messages() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@bot:matrix.org",
            "event_id": "$event456",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "My own message"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_skips_non_message_events() {
        let event = serde_json::json!({
            "type": "m.room.member",
            "sender": "@user:matrix.org",
            "event_id": "$event789",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "membership": "join"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_skips_empty_body() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$event000",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": ""
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_with_reply_to() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$reply_event",
            "origin_server_ts": 1700000001000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "This is a reply",
                "m.relates_to": {
                    "m.in_reply_to": {
                        "event_id": "$original_event"
                    }
                }
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "$original_event");
    }

    #[test]
    fn test_convert_timestamp_parsing() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$ts_event",
            "origin_server_ts": 1700000000123_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Timestamp test"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.timestamp.timestamp(), 1700000000);
    }

    #[test]
    fn test_convert_no_sender() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "event_id": "$no_sender",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "No sender"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_preserves_raw_event() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$raw_test",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Raw test"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert!(msg.raw.is_some());
        assert_eq!(msg.raw.unwrap()["event_id"].as_str().unwrap(), "$raw_test");
    }

    #[test]
    fn test_convert_sender_name_is_sender_id() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@alice:matrix.org",
            "event_id": "$name_test",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Name test"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        assert_eq!(msg.sender_name.as_deref(), Some("@alice:matrix.org"));
    }

    #[test]
    fn test_convert_thread_root_extraction() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$thread_reply",
            "origin_server_ts": 1700000002000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Reply in thread",
                "m.relates_to": {
                    "rel_type": "m.thread",
                    "event_id": "$thread_root"
                }
            }
        });

        let channel_id = ChannelId::new("matrix");
        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            None,
        )
        .unwrap();

        let has_thread_root = msg.metadata.iter().any(|m| {
            matches!(m, crate::gateway::channel::MessageMeta::ThreadRoot(id) 
                if id.as_str() == "$thread_root")
        });
        assert!(has_thread_root, "Expected ThreadRoot metadata with $thread_root");
    }

    #[test]
    fn test_convert_policy_user_allowlist() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@stranger:matrix.org",
            "event_id": "$event_blocked",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello from stranger!"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let config = MatrixConfig {
            allowed_users: vec!["@allowed:matrix.org".to_string()],
            ..Default::default()
        };

        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        );
        assert!(msg.is_none(), "Stranger should be blocked by allowlist");
    }

    #[test]
    fn test_convert_policy_user_allowlist_allowed() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@allowed:matrix.org",
            "event_id": "$event_allowed",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello from allowed user!"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let config = MatrixConfig {
            allowed_users: vec!["@allowed:matrix.org".to_string()],
            ..Default::default()
        };

        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        );
        assert!(msg.is_some(), "Allowed user should pass");
    }

    #[test]
    fn test_convert_policy_mention_gating() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$event_no_mention",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello without mention"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let config = MatrixConfig {
            mention_gating: true,
            ..Default::default()
        };

        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        );
        assert!(msg.is_none(), "Message without mention should be blocked when gating enabled");
    }

    #[test]
    fn test_convert_policy_mention_gating_pass() {
        let event = serde_json::json!({
            "type": "m.room.message",
            "sender": "@user:matrix.org",
            "event_id": "$event_with_mention",
            "origin_server_ts": 1700000000000_i64,
            "content": {
                "msgtype": "m.text",
                "body": "Hello @bot:matrix.org how are you?"
            }
        });

        let channel_id = ChannelId::new("matrix");
        let config = MatrixConfig {
            mention_gating: true,
            ..Default::default()
        };

        let msg = MatrixMessageOps::convert_room_event(
            &event,
            "!room1:matrix.org",
            &channel_id,
            "@bot:matrix.org",
            Some(&config),
        );
        assert!(msg.is_some(), "Message with mention should pass when gating enabled");
    }
}
