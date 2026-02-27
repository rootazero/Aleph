//! Mattermost API Operations
//!
//! Low-level functions for interacting with the Mattermost REST API v4 and
//! WebSocket event stream. Separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use std::time::Duration;

use super::config::MattermostConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Mattermost message length limit (characters).
pub(crate) const MATTERMOST_MSG_LIMIT: usize = 16383;

/// Mattermost message operations helper.
///
/// Provides methods for sending messages and interacting with the Mattermost REST API v4.
pub struct MattermostMessageOps;

impl MattermostMessageOps {
    /// Get current user info via `GET /api/v4/users/me`.
    ///
    /// Returns `(user_id, username)` on success.
    pub async fn get_me(
        client: &reqwest::Client,
        server: &str,
        token: &str,
    ) -> Result<(String, String), ChannelError> {
        let url = format!("{}/api/v4/users/me", server.trim_end_matches('/'));

        let resp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("users/me request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::AuthFailed(format!(
                "Mattermost auth failed {status}: {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("users/me response parse failed: {e}")))?;

        let user_id = body["id"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        let username = body["username"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok((user_id, username))
    }

    /// Send a message via `POST /api/v4/posts`.
    ///
    /// Automatically splits long messages and formats using standard Markdown.
    /// Optionally specify `root_id` for threaded replies.
    pub async fn send_message(
        client: &reqwest::Client,
        server: &str,
        token: &str,
        channel_id: &str,
        text: &str,
        root_id: Option<&str>,
    ) -> Result<SendResult, ChannelError> {
        let base = server.trim_end_matches('/');
        let url = format!("{base}/api/v4/posts");

        // Mattermost natively supports standard Markdown, so format as Markdown
        let formatted = MessageFormatter::format(text, MarkupFormat::Markdown);
        let chunks = MessageFormatter::split(&formatted, MATTERMOST_MSG_LIMIT);

        let mut last_result = None;

        for chunk in &chunks {
            let mut body = serde_json::json!({
                "channel_id": channel_id,
                "message": chunk,
            });

            if let Some(rid) = root_id {
                if !rid.is_empty() {
                    body["root_id"] = serde_json::Value::String(rid.to_string());
                }
            }

            let resp = client
                .post(&url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("posts request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                return Err(ChannelError::SendFailed(format!(
                    "Mattermost posts failed {status}: {resp_body}"
                )));
            }

            let resp_json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| {
                    ChannelError::SendFailed(format!("posts response parse failed: {e}"))
                })?;

            let post_id = resp_json["id"]
                .as_str()
                .unwrap_or("")
                .to_string();

            last_result = Some(SendResult {
                message_id: MessageId::new(post_id),
                timestamp: Utc::now(),
            });
        }

        last_result
            .ok_or_else(|| ChannelError::SendFailed("No message chunks to send".to_string()))
    }

    /// Send a typing indicator via `POST /api/v4/users/me/typing`.
    pub async fn send_typing(
        client: &reqwest::Client,
        server: &str,
        token: &str,
        channel_id: &str,
    ) -> Result<(), ChannelError> {
        let base = server.trim_end_matches('/');
        let url = format!("{base}/api/v4/users/me/typing");

        let body = serde_json::json!({
            "channel_id": channel_id,
        });

        // Best-effort: typing indicator failure is not fatal
        let _ = client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await;

        Ok(())
    }

    /// Convert a Mattermost WebSocket `posted` event to an `InboundMessage`.
    ///
    /// Returns `None` if the event should be ignored (bot's own message,
    /// filtered channel, non-posted event, empty text, etc.).
    ///
    /// Mattermost WebSocket event format:
    /// ```json
    /// {
    ///   "event": "posted",
    ///   "data": {
    ///     "post": "{\"id\":\"...\",\"channel_id\":\"...\",\"message\":\"...\",\"user_id\":\"...\",\"root_id\":\"...\"}",
    ///     "channel_type": "O|D|G",
    ///     "sender_name": "alice"
    ///   }
    /// }
    /// ```
    ///
    /// Note: `data.post` is a JSON string inside JSON, requiring a second parse.
    pub fn convert_posted_event(
        event: &serde_json::Value,
        channel_id: &ChannelId,
        own_user_id: &str,
        config: &MattermostConfig,
    ) -> Option<InboundMessage> {
        let event_type = event["event"].as_str().unwrap_or("");
        if event_type != "posted" {
            return None;
        }

        // The `data.post` field is a JSON string that needs a second parse
        let post_str = event["data"]["post"].as_str()?;
        let post: serde_json::Value = serde_json::from_str(post_str).ok()?;

        let user_id = post["user_id"].as_str().unwrap_or("");
        let mm_channel_id = post["channel_id"].as_str().unwrap_or("");
        let message = post["message"].as_str().unwrap_or("");
        let post_id = post["id"].as_str().unwrap_or("").to_string();

        // Skip own messages
        if user_id == own_user_id {
            return None;
        }

        // Filter by allowed channels
        if !config.is_channel_allowed(mm_channel_id) {
            return None;
        }

        if message.is_empty() {
            return None;
        }

        // Determine if group conversation from channel_type
        // "D" = direct message, "O" = open channel, "G" = group message, "P" = private channel
        let channel_type = event["data"]["channel_type"].as_str().unwrap_or("");
        let is_group = channel_type != "D";

        // Extract thread root id
        let root_id = post["root_id"].as_str().unwrap_or("");
        let reply_to = if root_id.is_empty() {
            None
        } else {
            Some(MessageId::new(root_id.to_string()))
        };

        // Sender display name from event data
        let sender_name = event["data"]["sender_name"]
            .as_str()
            .unwrap_or(user_id);

        // Parse create_at timestamp (milliseconds since epoch)
        let timestamp = post["create_at"]
            .as_i64()
            .and_then(|ms| chrono::DateTime::from_timestamp(ms / 1000, ((ms % 1000) * 1_000_000) as u32))
            .unwrap_or_else(Utc::now);

        Some(InboundMessage {
            id: MessageId::new(post_id),
            channel_id: channel_id.clone(),
            conversation_id: ConversationId::new(mm_channel_id.to_string()),
            sender_id: UserId::new(user_id.to_string()),
            sender_name: Some(sender_name.to_string()),
            text: message.to_string(),
            attachments: Vec::new(), // TODO: extract Mattermost file attachments
            timestamp,
            reply_to,
            is_group,
            raw: Some(event.clone()),
        })
    }

    /// Run the WebSocket event loop with reconnection and exponential backoff.
    ///
    /// This function runs indefinitely until a shutdown signal is received.
    /// It handles:
    /// 1. Building the WS URL from the server URL
    /// 2. Connecting with tokio-tungstenite
    /// 3. Sending the authentication challenge
    /// 4. Processing events in a select! loop (message vs shutdown)
    /// 5. For "posted" events: parsing `data.post` (JSON string inside JSON)
    /// 6. Skipping own messages and filtered channels
    /// 7. Reconnecting with exponential backoff on disconnect
    #[cfg(feature = "mattermost")]
    pub async fn run_ws_loop(
        _client: reqwest::Client,
        config: MattermostConfig,
        user_id: String,
        channel_id: ChannelId,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use futures_util::{SinkExt, StreamExt};

        let mut backoff = INITIAL_BACKOFF;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let ws_url = config.ws_url();
            tracing::info!("Connecting to Mattermost WebSocket at {ws_url}...");

            let ws_result = tokio_tungstenite::connect_async(&ws_url).await;
            let ws_stream = match ws_result {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!(
                        "Mattermost WebSocket connection failed: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            // Reset backoff on successful connection
            backoff = INITIAL_BACKOFF;
            tracing::info!("Mattermost WebSocket connected");

            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            // Send authentication challenge
            let auth_msg = serde_json::json!({
                "seq": 1,
                "action": "authentication_challenge",
                "data": {
                    "token": config.bot_token
                }
            });

            if let Err(e) = ws_tx
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::to_string(&auth_msg).unwrap().into(),
                ))
                .await
            {
                tracing::warn!("Mattermost WebSocket auth send failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }

            // Inner message loop
            let should_reconnect = 'inner: loop {
                let msg = tokio::select! {
                    msg = ws_rx.next() => msg,
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("Mattermost channel shutting down");
                            let _ = ws_tx.close().await;
                            return;
                        }
                        continue;
                    }
                };

                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::warn!("Mattermost WebSocket error: {e}");
                        break 'inner true;
                    }
                    None => {
                        tracing::info!("Mattermost WebSocket closed");
                        break 'inner true;
                    }
                };

                let text = match msg {
                    tokio_tungstenite::tungstenite::Message::Text(t) => t,
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        tracing::info!("Mattermost WebSocket closed by server");
                        break 'inner true;
                    }
                    _ => continue,
                };

                let payload: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("Mattermost: failed to parse message: {e}");
                        continue;
                    }
                };

                // Check for auth response (status field present)
                if payload.get("status").is_some() {
                    let status = payload["status"].as_str().unwrap_or("");
                    if status == "OK" {
                        tracing::debug!("Mattermost WebSocket authentication successful");
                    } else {
                        tracing::warn!("Mattermost WebSocket auth response: {status}");
                    }
                    continue;
                }

                // Parse posted events
                if let Some(inbound) = Self::convert_posted_event(
                    &payload,
                    &channel_id,
                    &user_id,
                    &config,
                ) {
                    tracing::debug!(
                        "Mattermost message from {}: {}",
                        inbound.sender_name.as_deref().unwrap_or("?"),
                        &inbound.text[..inbound.text.len().min(50)]
                    );
                    if inbound_tx.send(inbound).await.is_err() {
                        tracing::error!("Mattermost: inbound channel closed");
                        return;
                    }
                }
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("Mattermost: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }

        tracing::info!("Mattermost WebSocket loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_posted_event(post: &serde_json::Value, channel_type: &str, sender_name: &str) -> serde_json::Value {
        serde_json::json!({
            "event": "posted",
            "data": {
                "post": serde_json::to_string(post).unwrap(),
                "channel_type": channel_type,
                "sender_name": sender_name
            }
        })
    }

    #[test]
    fn test_convert_basic_message() {
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "Hello from Mattermost!",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "O", "alice");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        )
        .unwrap();

        assert_eq!(msg.channel_id.as_str(), "mattermost");
        assert_eq!(msg.conversation_id.as_str(), "ch-789");
        assert_eq!(msg.sender_id.as_str(), "user-456");
        assert_eq!(msg.sender_name.as_deref(), Some("alice"));
        assert_eq!(msg.text, "Hello from Mattermost!");
        assert!(msg.is_group);
        assert!(msg.reply_to.is_none());
        assert_eq!(msg.id.as_str(), "post-1");
    }

    #[test]
    fn test_convert_dm_message() {
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "DM message",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "D", "bob");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        )
        .unwrap();

        assert!(!msg.is_group);
    }

    #[test]
    fn test_convert_threaded_reply() {
        let post = serde_json::json!({
            "id": "post-2",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "Thread reply",
            "root_id": "post-1",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "O", "alice");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        )
        .unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "post-1");
    }

    #[test]
    fn test_convert_skips_own_message() {
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "bot-123",
            "channel_id": "ch-789",
            "message": "Bot's own message",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "O", "aleph-bot");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_channel_filter() {
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "Hello",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "O", "alice");
        let channel_id = ChannelId::new("mattermost");

        // Not in allowed channels
        let config = MattermostConfig {
            allowed_channels: vec!["ch-111".to_string(), "ch-222".to_string()],
            ..Default::default()
        };
        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        );
        assert!(msg.is_none());

        // In allowed channels
        let config = MattermostConfig {
            allowed_channels: vec!["ch-789".to_string()],
            ..Default::default()
        };
        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        );
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_non_posted_event() {
        let event = serde_json::json!({
            "event": "typing",
            "data": {}
        });

        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_empty_message() {
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "O", "alice");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_timestamp_parsing() {
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "Hello",
            "root_id": "",
            "create_at": 1700000000123_i64
        });

        let event = make_posted_event(&post, "O", "alice");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        )
        .unwrap();

        assert_eq!(msg.timestamp.timestamp(), 1700000000);
    }

    #[test]
    fn test_convert_group_channel_type() {
        // "G" = group message channel
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "Group message",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "G", "alice");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        )
        .unwrap();

        assert!(msg.is_group);
    }

    #[test]
    fn test_convert_private_channel_type() {
        // "P" = private channel
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "Private channel message",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "P", "alice");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        )
        .unwrap();

        assert!(msg.is_group);
    }

    #[test]
    fn test_convert_preserves_raw_event() {
        let post = serde_json::json!({
            "id": "post-1",
            "user_id": "user-456",
            "channel_id": "ch-789",
            "message": "Test",
            "root_id": "",
            "create_at": 1700000000000_i64
        });

        let event = make_posted_event(&post, "O", "alice");
        let channel_id = ChannelId::new("mattermost");
        let config = MattermostConfig::default();

        let msg = MattermostMessageOps::convert_posted_event(
            &event, &channel_id, "bot-123", &config,
        )
        .unwrap();

        assert!(msg.raw.is_some());
        assert_eq!(msg.raw.unwrap()["event"].as_str().unwrap(), "posted");
    }
}
