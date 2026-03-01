//! Matrix API Operations
//!
//! Low-level functions for interacting with the Matrix Client-Server API v3.
//! These are separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::config::MatrixConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Matrix message length limit (characters).
pub(crate) const MATRIX_MSG_LIMIT: usize = 65535;

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

        let user_id = body["user_id"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

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

            let resp_json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| {
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

        last_result
            .ok_or_else(|| ChannelError::SendFailed("No message chunks to send".to_string()))
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
        let url = format!(
            "{homeserver}/_matrix/client/v3/rooms/{room_id}/typing/{user_id}"
        );

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
            .map_err(|e| {
                ChannelError::Internal(format!("Matrix typing indicator failed: {e}"))
            })?;

        if !resp.status().is_success() {
            tracing::warn!(
                "Matrix typing indicator returned {}",
                resp.status()
            );
        }

        Ok(())
    }

    /// Convert a Matrix room event to an `InboundMessage`.
    ///
    /// Returns `None` if the event should be ignored (own message,
    /// filtered room, non-message event, etc.).
    pub fn convert_room_event(
        event: &serde_json::Value,
        room_id: &str,
        channel_id: &ChannelId,
        own_user_id: &str,
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

        let content = &event["content"];
        let body = content["body"].as_str().unwrap_or("");
        if body.is_empty() {
            return None;
        }

        let event_id = event["event_id"]
            .as_str()
            .unwrap_or("")
            .to_string();

        // Parse timestamp from origin_server_ts (milliseconds since epoch)
        let timestamp = event["origin_server_ts"]
            .as_i64()
            .and_then(|ms| chrono::DateTime::from_timestamp(ms / 1000, ((ms % 1000) * 1_000_000) as u32))
            .unwrap_or_else(Utc::now);

        // Extract reply-to from m.relates_to.m.in_reply_to.event_id
        let reply_to = content["m.relates_to"]["m.in_reply_to"]["event_id"]
            .as_str()
            .map(|id| MessageId::new(id.to_string()));

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
    #[cfg(feature = "matrix")]
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
            let own_user_id = user_id
                .read()
                .await
                .clone()
                .unwrap_or_default();

            // Process room events from rooms.join
            if let Some(rooms) = body["rooms"]["join"].as_object() {
                for (room_id, room_data) in rooms {
                    // Filter by allowed rooms
                    if !config.is_room_allowed(room_id) {
                        continue;
                    }

                    if let Some(events) = room_data["timeline"]["events"].as_array() {
                        for event in events {
                            if let Some(inbound) = Self::convert_room_event(
                                event,
                                room_id,
                                &channel_id,
                                &own_user_id,
                            ) {
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
        )
        .unwrap();

        assert_eq!(
            msg.reply_to.as_ref().unwrap().as_str(),
            "$original_event"
        );
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
        )
        .unwrap();

        assert_eq!(
            msg.sender_name.as_deref(),
            Some("@alice:matrix.org")
        );
    }
}
