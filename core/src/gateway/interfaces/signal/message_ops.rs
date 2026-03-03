//! Signal API Operations
//!
//! Low-level functions for interacting with the signal-cli REST API.
//! These are separated from the channel struct for testability.

use crate::gateway::channel::{
    ChannelError, ChannelId, ConversationId, InboundMessage, MessageId, SendResult, UserId,
};
use crate::gateway::formatter::{MarkupFormat, MessageFormatter};
use chrono::Utc;
use std::time::Duration;

use super::config::SignalConfig;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Signal message length limit (characters).
pub(crate) const SIGNAL_MSG_LIMIT: usize = 65535;

/// Signal message operations helper.
///
/// Provides methods for sending/receiving messages via the signal-cli REST API.
pub struct SignalMessageOps;

impl SignalMessageOps {
    /// Poll for new messages from signal-cli REST API.
    ///
    /// Uses `GET /v1/receive/{phone_number}` to fetch pending messages.
    pub async fn poll_messages(
        client: &reqwest::Client,
        api_url: &str,
        phone: &str,
    ) -> Result<Vec<serde_json::Value>, ChannelError> {
        let url = format!("{api_url}/v1/receive/{phone}");

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ChannelError::ReceiveFailed(format!("Signal poll failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::ReceiveFailed(format!(
                "Signal poll error ({status}): {body}"
            )));
        }

        let messages: Vec<serde_json::Value> = resp.json().await.map_err(|e| {
            ChannelError::ReceiveFailed(format!("Signal poll parse error: {e}"))
        })?;

        Ok(messages)
    }

    /// Send a message via signal-cli REST API.
    ///
    /// Uses `POST /v2/send` with the bot's phone number as sender.
    /// Formats text as plain text since Signal doesn't support rich text.
    /// Automatically splits long messages.
    pub async fn send_message(
        client: &reqwest::Client,
        api_url: &str,
        phone: &str,
        to: &str,
        text: &str,
    ) -> Result<SendResult, ChannelError> {
        // Format as plain text (Signal doesn't support rich text)
        let formatted = MessageFormatter::format(text, MarkupFormat::PlainText);
        let chunks = MessageFormatter::split(&formatted, SIGNAL_MSG_LIMIT);

        let mut last_result = None;

        for chunk in &chunks {
            let url = format!("{api_url}/v2/send");

            let body = serde_json::json!({
                "message": chunk,
                "number": phone,
                "recipients": [to],
            });

            let resp = client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(format!("Signal send failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                return Err(ChannelError::SendFailed(format!(
                    "Signal send failed ({status}): {resp_body}"
                )));
            }

            // signal-cli returns a timestamp as the message ID
            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            let timestamp = resp_json["timestamp"]
                .as_u64()
                .or_else(|| resp_json["timestamps"].as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_u64()))
                .unwrap_or_else(|| Utc::now().timestamp_millis() as u64);

            last_result = Some(SendResult {
                message_id: MessageId::new(timestamp.to_string()),
                timestamp: Utc::now(),
            });
        }

        last_result
            .ok_or_else(|| ChannelError::SendFailed("No message chunks to send".to_string()))
    }

    /// Convert a signal-cli message envelope JSON to an `InboundMessage`.
    ///
    /// Returns `None` if the message should be ignored (own message,
    /// filtered user, non-data message, etc.).
    ///
    /// Expected format from signal-cli REST API:
    /// ```json
    /// {
    ///   "envelope": {
    ///     "source": "+1234567890",
    ///     "sourceName": "John",
    ///     "timestamp": 1234567890000,
    ///     "dataMessage": {
    ///       "message": "Hello",
    ///       "timestamp": 1234567890000,
    ///       "groupInfo": { "groupId": "abc123" }
    ///     }
    ///   }
    /// }
    /// ```
    pub fn convert_message(
        msg: &serde_json::Value,
        channel_id: &ChannelId,
        own_phone: &str,
        config: &SignalConfig,
    ) -> Option<InboundMessage> {
        let envelope = msg.get("envelope").unwrap_or(msg);

        let source = envelope["source"].as_str().unwrap_or("").to_string();

        // Skip empty source or own messages
        if source.is_empty() || source == own_phone {
            return None;
        }

        // Check allowed users
        if !config.is_user_allowed(&source) {
            return None;
        }

        // Extract text from dataMessage
        let data_message = &envelope["dataMessage"];
        let text = data_message["message"].as_str().unwrap_or("");

        if text.is_empty() {
            return None;
        }

        let source_name = envelope["sourceName"]
            .as_str()
            .unwrap_or(&source)
            .to_string();

        // Extract message timestamp
        let timestamp_ms = data_message["timestamp"]
            .as_i64()
            .or_else(|| envelope["timestamp"].as_i64())
            .unwrap_or(0);

        let timestamp = chrono::DateTime::from_timestamp(
            timestamp_ms / 1000,
            ((timestamp_ms % 1000) * 1_000_000) as u32,
        )
        .unwrap_or_else(Utc::now);

        // Determine if group message and extract conversation ID
        let group_id = data_message["groupInfo"]["groupId"].as_str();
        let is_group = group_id.is_some();
        let conversation_id = group_id
            .unwrap_or(&source)
            .to_string();

        Some(InboundMessage {
            id: MessageId::new(timestamp_ms.to_string()),
            channel_id: channel_id.clone(),
            conversation_id: ConversationId::new(conversation_id),
            sender_id: UserId::new(source.clone()),
            sender_name: Some(source_name),
            text: text.to_string(),
            attachments: Vec::new(),
            timestamp,
            reply_to: None,
            is_group,
            raw: Some(msg.clone()),
        })
    }

    /// Run the polling loop for receiving messages from signal-cli.
    ///
    /// Polls `GET /v1/receive/{phone}` at the configured interval.
    /// Handles:
    /// - Periodic polling with configurable interval
    /// - Message filtering by allowed users
    /// - Graceful shutdown via watch channel
    /// - Exponential backoff on errors
    pub async fn run_poll_loop(
        client: reqwest::Client,
        config: SignalConfig,
        channel_id: ChannelId,
        inbound_tx: tokio::sync::mpsc::Sender<InboundMessage>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let poll_interval = Duration::from_secs(config.poll_interval_secs);
        let mut backoff = INITIAL_BACKOFF;

        tracing::info!(
            "Signal poll loop started (polling {} every {:?})",
            config.api_url,
            poll_interval
        );

        loop {
            // Wait for poll interval or shutdown
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("Signal poll loop shutting down");
                        break;
                    }
                    continue;
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }

            // Check shutdown before polling
            if *shutdown_rx.borrow() {
                break;
            }

            // Poll for new messages
            match Self::poll_messages(&client, &config.api_url, &config.phone_number).await {
                Ok(messages) => {
                    // Reset backoff on success
                    backoff = INITIAL_BACKOFF;

                    for msg in &messages {
                        if let Some(inbound) = Self::convert_message(
                            msg,
                            &channel_id,
                            &config.phone_number,
                            &config,
                        ) {
                            tracing::debug!(
                                "Signal message from {}: {}",
                                inbound.sender_id.as_str(),
                                &inbound.text[..inbound.text.len().min(50)]
                            );
                            if inbound_tx.send(inbound).await.is_err() {
                                tracing::error!("Signal: inbound channel closed");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Signal poll error: {e}, retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }

        tracing::info!("Signal poll loop stopped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_basic_message() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "Hello from Signal!",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let inbound = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        )
        .unwrap();

        assert_eq!(inbound.channel_id.as_str(), "signal");
        assert_eq!(inbound.sender_id.as_str(), "+9876543210");
        assert_eq!(inbound.sender_name.as_deref(), Some("Alice"));
        assert_eq!(inbound.text, "Hello from Signal!");
        assert_eq!(inbound.id.as_str(), "1700000000000");
        assert!(!inbound.is_group);
        assert_eq!(inbound.conversation_id.as_str(), "+9876543210");
    }

    #[test]
    fn test_convert_group_message() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "sourceName": "Bob",
                "timestamp": 1700000001000_i64,
                "dataMessage": {
                    "message": "Group message",
                    "timestamp": 1700000001000_i64,
                    "groupInfo": {
                        "groupId": "abc123group"
                    }
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let inbound = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        )
        .unwrap();

        assert!(inbound.is_group);
        assert_eq!(inbound.conversation_id.as_str(), "abc123group");
    }

    #[test]
    fn test_convert_filters_own_messages() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+1234567890",
                "sourceName": "Me",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "My own message",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let result = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_filters_by_allowed_users() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+5555555555",
                "sourceName": "Stranger",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "Not allowed",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            allowed_users: vec!["+9876543210".to_string()],
            ..Default::default()
        };

        let result = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_skips_empty_message() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let result = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_skips_no_data_message() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let result = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_skips_empty_source() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "Hello",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let result = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_convert_timestamp_parsing() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "sourceName": "Alice",
                "timestamp": 1700000000123_i64,
                "dataMessage": {
                    "message": "Timestamp test",
                    "timestamp": 1700000000123_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let inbound = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        )
        .unwrap();

        assert_eq!(inbound.timestamp.timestamp(), 1700000000);
    }

    #[test]
    fn test_convert_preserves_raw() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "sourceName": "Alice",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "Raw test",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let inbound = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        )
        .unwrap();

        assert!(inbound.raw.is_some());
        assert_eq!(
            inbound.raw.unwrap()["envelope"]["source"].as_str().unwrap(),
            "+9876543210"
        );
    }

    #[test]
    fn test_convert_fallback_source_name() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "No name",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            ..Default::default()
        };

        let inbound = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        )
        .unwrap();

        // When sourceName is not present, falls back to source phone number
        assert_eq!(inbound.sender_name.as_deref(), Some("+9876543210"));
    }

    #[test]
    fn test_convert_allowed_user_passes() {
        let msg = serde_json::json!({
            "envelope": {
                "source": "+9876543210",
                "sourceName": "Allowed",
                "timestamp": 1700000000000_i64,
                "dataMessage": {
                    "message": "Allowed user",
                    "timestamp": 1700000000000_i64
                }
            }
        });

        let channel_id = ChannelId::new("signal");
        let config = SignalConfig {
            phone_number: "+1234567890".to_string(),
            allowed_users: vec!["+9876543210".to_string()],
            ..Default::default()
        };

        let result = SignalMessageOps::convert_message(
            &msg,
            &channel_id,
            "+1234567890",
            &config,
        );
        assert!(result.is_some());
    }
}
