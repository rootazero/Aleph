//! Slack API Operations
//!
//! Low-level functions for interacting with the Slack Web API and Socket Mode.
//! These are separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::config::SlackConfig;

const SLACK_API_BASE: &str = "https://slack.com/api";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Slack message length limit (characters).
pub(crate) const SLACK_MSG_LIMIT: usize = 3000;

/// Slack message operations helper.
///
/// Provides methods for sending messages and interacting with the Slack REST API.
pub struct SlackMessageOps;

impl SlackMessageOps {
    /// Validate bot token via `auth.test` and return the bot user ID.
    pub async fn validate_bot_token(
        client: &reqwest::Client,
        bot_token: &str,
    ) -> Result<String, ChannelError> {
        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/auth.test"))
            .header("Authorization", format!("Bearer {bot_token}"))
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("auth.test request failed: {e}")))?
            .json()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("auth.test response parse failed: {e}")))?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown error");
            return Err(ChannelError::AuthFailed(format!(
                "Slack auth.test failed: {err}"
            )));
        }

        let user_id = resp["user_id"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        Ok(user_id)
    }

    /// Get Socket Mode WebSocket URL via `apps.connections.open`.
    pub async fn get_socket_mode_url(
        client: &reqwest::Client,
        app_token: &str,
    ) -> Result<String, ChannelError> {
        let resp: serde_json::Value = client
            .post(format!("{SLACK_API_BASE}/apps.connections.open"))
            .header("Authorization", format!("Bearer {app_token}"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("apps.connections.open request failed: {e}"))
            })?
            .json()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("apps.connections.open response parse failed: {e}"))
            })?;

        if resp["ok"].as_bool() != Some(true) {
            let err = resp["error"].as_str().unwrap_or("unknown error");
            return Err(ChannelError::Internal(format!(
                "Slack apps.connections.open failed: {err}"
            )));
        }

        resp["url"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                ChannelError::Internal(
                    "Missing 'url' in connections.open response".to_string(),
                )
            })
    }

    /// Send a message via `chat.postMessage`.
    ///
    /// Automatically splits long messages and formats using SlackMrkdwn.
    pub async fn send_message(
        client: &reqwest::Client,
        bot_token: &str,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<SendResult, ChannelError> {
        // Format text for Slack mrkdwn
        let formatted = MessageFormatter::format(text, MarkupFormat::SlackMrkdwn);
        let chunks = MessageFormatter::split(&formatted, SLACK_MSG_LIMIT);

        let mut last_result = None;

        for chunk in &chunks {
            let mut body = serde_json::json!({
                "channel": channel,
                "text": chunk,
            });

            if let Some(ts) = thread_ts {
                body["thread_ts"] = serde_json::Value::String(ts.to_string());
            }

            let resp: serde_json::Value = client
                .post(format!("{SLACK_API_BASE}/chat.postMessage"))
                .header("Authorization", format!("Bearer {bot_token}"))
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("chat.postMessage failed: {e}")))?
                .json()
                .await
                .map_err(|e| {
                    ChannelError::SendFailed(format!(
                        "chat.postMessage response parse failed: {e}"
                    ))
                })?;

            if resp["ok"].as_bool() != Some(true) {
                let err = resp["error"].as_str().unwrap_or("unknown");
                return Err(ChannelError::SendFailed(format!(
                    "Slack chat.postMessage failed: {err}"
                )));
            }

            let msg_ts = resp["ts"]
                .as_str()
                .unwrap_or("0")
                .to_string();

            last_result = Some(SendResult {
                message_id: MessageId::new(msg_ts),
                timestamp: Utc::now(),
            });
        }

        last_result.ok_or_else(|| ChannelError::SendFailed("No message chunks to send".to_string()))
    }

    /// Convert a Slack event payload to an `InboundMessage`.
    ///
    /// Returns `None` if the event should be ignored (bot's own message,
    /// filtered channel, non-message event, etc.).
    pub fn convert_event_to_inbound(
        event: &serde_json::Value,
        channel_id: &ChannelId,
        bot_user_id: &str,
        config: &SlackConfig,
    ) -> Option<InboundMessage> {
        let event_type = event["type"].as_str()?;
        if event_type != "message" {
            return None;
        }

        // Handle message_changed subtype: extract inner message
        let subtype = event["subtype"].as_str();
        let (msg_data, _is_edit) = match subtype {
            Some("message_changed") => match event.get("message") {
                Some(inner) => (inner, true),
                None => return None,
            },
            Some(_) => return None, // Skip other subtypes (joins, leaves, etc.)
            None => (event, false),
        };

        // Filter out bot messages
        if msg_data.get("bot_id").is_some() {
            return None;
        }

        let user_id = msg_data["user"]
            .as_str()
            .or_else(|| event["user"].as_str())?;

        // Filter out bot's own messages
        if user_id == bot_user_id {
            return None;
        }

        let slack_channel = event["channel"].as_str()?;

        // Filter by allowed channels
        if !config.is_channel_allowed(slack_channel) {
            return None;
        }

        // Check DM permission (DMs start with "D")
        let is_dm = slack_channel.starts_with('D');
        if is_dm && !config.dm_allowed {
            return None;
        }

        let text = msg_data["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return None;
        }

        // Normalize Slack mrkdwn to standard Markdown
        let normalized_text = MessageFormatter::normalize(text, MarkupFormat::SlackMrkdwn);

        let ts = msg_data["ts"]
            .as_str()
            .or_else(|| event["ts"].as_str())
            .unwrap_or("0");

        // Parse timestamp (Slack uses epoch.microseconds format)
        let timestamp = ts
            .split('.')
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|epoch| chrono::DateTime::from_timestamp(epoch, 0))
            .unwrap_or_else(Utc::now);

        // Extract thread_ts for reply threading
        let reply_to = event["thread_ts"]
            .as_str()
            .map(|ts| MessageId::new(ts.to_string()));

        Some(InboundMessage {
            id: MessageId::new(ts.to_string()),
            channel_id: channel_id.clone(),
            conversation_id: ConversationId::new(slack_channel.to_string()),
            sender_id: UserId::new(user_id.to_string()),
            sender_name: Some(user_id.to_string()), // Slack user IDs as display name
            text: normalized_text,
            attachments: Vec::new(), // TODO: extract Slack file attachments
            timestamp,
            reply_to,
            is_group: !is_dm,
            raw: Some(event.clone()),
        })
    }

    /// Run the Socket Mode WebSocket loop with reconnection and exponential backoff.
    ///
    /// This function runs indefinitely until a shutdown signal is received.
    /// It handles:
    /// - Getting a fresh WebSocket URL via `apps.connections.open`
    /// - Connecting with tokio-tungstenite
    /// - Processing events in a loop with `tokio::select!`
    /// - ACK-ing `events_api` envelopes
    /// - Reconnecting with exponential backoff on disconnect
    #[cfg(feature = "slack")]
    pub async fn run_socket_mode_loop(
        client: reqwest::Client,
        app_token: String,
        bot_user_id: Arc<RwLock<Option<String>>>,
        channel_id: ChannelId,
        config: SlackConfig,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        use futures_util::{SinkExt, StreamExt};

        let mut backoff = INITIAL_BACKOFF;

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            // Get a fresh WebSocket URL
            let ws_url = match Self::get_socket_mode_url(&client, &app_token).await {
                Ok(url) => url,
                Err(e) => {
                    tracing::warn!(
                        "Slack: failed to get WebSocket URL: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            tracing::info!("Connecting to Slack Socket Mode...");

            let ws_result = tokio_tungstenite::connect_async(&ws_url).await;
            let ws_stream = match ws_result {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!(
                        "Slack WebSocket connection failed: {e}, retrying in {backoff:?}"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            // Reset backoff on successful connection
            backoff = INITIAL_BACKOFF;
            tracing::info!("Slack Socket Mode connected");

            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            let should_reconnect = 'inner: loop {
                let msg = tokio::select! {
                    msg = ws_rx.next() => msg,
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            let _ = ws_tx.close().await;
                            return;
                        }
                        continue;
                    }
                };

                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        tracing::warn!("Slack WebSocket error: {e}");
                        break 'inner true;
                    }
                    None => {
                        tracing::info!("Slack WebSocket closed");
                        break 'inner true;
                    }
                };

                let text = match msg {
                    tokio_tungstenite::tungstenite::Message::Text(t) => t,
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        tracing::info!("Slack Socket Mode closed by server");
                        break 'inner true;
                    }
                    _ => continue,
                };

                let payload: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("Slack: failed to parse message: {e}");
                        continue;
                    }
                };

                let envelope_type = payload["type"].as_str().unwrap_or("");

                match envelope_type {
                    "hello" => {
                        tracing::debug!("Slack Socket Mode hello received");
                    }

                    "events_api" => {
                        // Acknowledge the envelope
                        let envelope_id = payload["envelope_id"].as_str().unwrap_or("");
                        if !envelope_id.is_empty() {
                            let ack = serde_json::json!({ "envelope_id": envelope_id });
                            if let Err(e) = ws_tx
                                .send(tokio_tungstenite::tungstenite::Message::Text(
                                    serde_json::to_string(&ack).unwrap().into(),
                                ))
                                .await
                            {
                                tracing::error!("Slack: failed to send ack: {e}");
                                break 'inner true;
                            }
                        }

                        // Extract and process the event
                        let event = &payload["payload"]["event"];
                        let bot_id_guard = bot_user_id.read().await;
                        let bot_id_str = bot_id_guard
                            .as_deref()
                            .unwrap_or("");

                        if let Some(inbound) = Self::convert_event_to_inbound(
                            event,
                            &channel_id,
                            bot_id_str,
                            &config,
                        ) {
                            tracing::debug!(
                                "Slack message from {}: {}",
                                inbound.sender_id.as_str(),
                                &inbound.text[..inbound.text.len().min(50)]
                            );
                            if inbound_tx.send(inbound).await.is_err() {
                                tracing::error!("Slack: inbound channel closed");
                                return;
                            }
                        }
                    }

                    "disconnect" => {
                        let reason = payload["reason"].as_str().unwrap_or("unknown");
                        tracing::info!("Slack disconnect request: {reason}");
                        break 'inner true;
                    }

                    _ => {
                        tracing::debug!("Slack envelope type: {envelope_type}");
                    }
                }
            };

            if !should_reconnect || *shutdown_rx.borrow() {
                break;
            }

            tracing::warn!("Slack: reconnecting in {backoff:?}");
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }

        tracing::info!("Slack Socket Mode loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_basic_message() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello agent!",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        )
        .unwrap();

        assert_eq!(msg.channel_id.as_str(), "slack");
        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.sender_id.as_str(), "U456");
        assert_eq!(msg.text, "Hello agent!");
        assert!(msg.is_group);
    }

    #[test]
    fn test_convert_filters_bot_messages() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Bot message",
            "ts": "1700000000.000100",
            "bot_id": "B999"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_filters_own_user() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "My message",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "U456", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_channel_filter() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        // Not in allowed channels
        let config = SlackConfig {
            allowed_channels: vec!["C111".to_string(), "C222".to_string()],
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_none());

        // In allowed channels
        let config = SlackConfig {
            allowed_channels: vec!["C789".to_string()],
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_some());
    }

    #[test]
    fn test_convert_skips_other_subtypes() {
        let event = serde_json::json!({
            "type": "message",
            "subtype": "channel_join",
            "user": "U456",
            "channel": "C789",
            "text": "joined",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_message_changed() {
        let event = serde_json::json!({
            "type": "message",
            "subtype": "message_changed",
            "channel": "C789",
            "message": {
                "user": "U456",
                "text": "Edited message text",
                "ts": "1700000000.000100"
            },
            "ts": "1700000001.000200"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        )
        .unwrap();

        assert_eq!(msg.conversation_id.as_str(), "C789");
        assert_eq!(msg.text, "Edited message text");
    }

    #[test]
    fn test_convert_non_message_event() {
        let event = serde_json::json!({
            "type": "reaction_added",
            "user": "U456",
            "reaction": "thumbsup"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_empty_text() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_dm_message() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "D12345",
            "text": "Private message",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");

        // DMs allowed
        let config = SlackConfig {
            dm_allowed: true,
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_some());
        assert!(!msg.unwrap().is_group);

        // DMs not allowed
        let config = SlackConfig {
            dm_allowed: false,
            ..Default::default()
        };
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        );
        assert!(msg.is_none());
    }

    #[test]
    fn test_convert_thread_reply() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Thread reply",
            "ts": "1700000002.000300",
            "thread_ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        )
        .unwrap();

        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "1700000000.000100");
    }

    #[test]
    fn test_convert_normalizes_mrkdwn() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "*bold text*",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        )
        .unwrap();

        // Slack *bold* normalizes to Markdown **bold**
        assert_eq!(msg.text, "**bold text**");
    }

    #[test]
    fn test_convert_timestamp_parsing() {
        let event = serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "Hello",
            "ts": "1700000000.000100"
        });

        let channel_id = ChannelId::new("slack");
        let config = SlackConfig::default();
        let msg = SlackMessageOps::convert_event_to_inbound(
            &event, &channel_id, "B123", &config,
        )
        .unwrap();

        assert_eq!(msg.timestamp.timestamp(), 1700000000);
    }
}
