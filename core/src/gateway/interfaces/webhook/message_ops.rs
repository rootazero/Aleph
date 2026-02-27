//! Webhook Message Operations
//!
//! Low-level functions for parsing inbound webhook payloads and building
//! outbound webhook payloads. Separated from the channel struct for testability.
//!
//! # Protocol
//!
//! Both inbound and outbound use the same JSON format:
//! ```json
//! {
//!     "sender_id": "user-123",
//!     "sender_name": "Alice",
//!     "message": "Hello!",
//!     "conversation_id": "conv-abc",
//!     "thread_id": null,
//!     "is_group": false,
//!     "metadata": {}
//! }
//! ```

use crate::gateway::channel::{
    ChannelError, ChannelId, ChannelResult, ConversationId, InboundMessage, MessageId,
    OutboundMessage, SendResult, UserId,
};
use crate::gateway::webhook_receiver::WebhookReceiver;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Webhook message JSON payload (used for both inbound parsing and outbound building).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Sender identifier
    #[serde(default = "default_sender_id")]
    pub sender_id: String,

    /// Sender display name
    #[serde(default = "default_sender_name")]
    pub sender_name: String,

    /// Message text content
    pub message: String,

    /// Conversation/chat identifier
    #[serde(default)]
    pub conversation_id: Option<String>,

    /// Thread identifier for threading support
    #[serde(default)]
    pub thread_id: Option<String>,

    /// Whether this is a group message
    #[serde(default)]
    pub is_group: bool,

    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

fn default_sender_id() -> String {
    "webhook-user".to_string()
}

fn default_sender_name() -> String {
    "Webhook User".to_string()
}

/// Webhook message operations helper.
pub struct WebhookMessageOps;

impl WebhookMessageOps {
    /// Parse inbound webhook payload into InboundMessage(s).
    ///
    /// The body is expected to be a single JSON object or an array of objects.
    /// Each object must contain at least a `message` field.
    /// A UUID is generated for each message ID.
    pub fn parse_inbound_payload(
        body: &[u8],
        channel_id: &ChannelId,
    ) -> ChannelResult<Vec<InboundMessage>> {
        // Try parsing as array first, then as single object
        let payloads: Vec<WebhookPayload> =
            if let Ok(arr) = serde_json::from_slice::<Vec<WebhookPayload>>(body) {
                arr
            } else {
                let single: WebhookPayload = serde_json::from_slice(body).map_err(|e| {
                    ChannelError::ReceiveFailed(format!("Invalid webhook JSON: {e}"))
                })?;
                vec![single]
            };

        let mut messages = Vec::with_capacity(payloads.len());

        for payload in payloads {
            if payload.message.is_empty() {
                continue; // Skip empty messages
            }

            let msg_id = uuid::Uuid::new_v4().to_string();
            let conversation_id = payload
                .conversation_id
                .unwrap_or_else(|| payload.sender_id.clone());

            messages.push(InboundMessage {
                id: MessageId::new(msg_id),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(conversation_id),
                sender_id: UserId::new(payload.sender_id),
                sender_name: Some(payload.sender_name),
                text: payload.message,
                attachments: Vec::new(),
                timestamp: Utc::now(),
                reply_to: payload.thread_id.map(MessageId::new),
                is_group: payload.is_group,
                raw: serde_json::to_value(&payload.metadata).ok(),
            });
        }

        Ok(messages)
    }

    /// Build outbound JSON payload from an OutboundMessage.
    pub fn build_outbound_payload(message: &OutboundMessage) -> serde_json::Value {
        serde_json::json!({
            "sender_id": "aleph",
            "sender_name": "Aleph",
            "message": message.text,
            "conversation_id": message.conversation_id.as_str(),
            "thread_id": message.reply_to.as_ref().map(|id| id.as_str()),
            "is_group": false,
            "metadata": message.metadata,
        })
    }

    /// Send an outbound message by POSTing to the callback URL.
    ///
    /// Signs the request body with HMAC-SHA256 and includes the signature
    /// in the `X-Webhook-Signature` header.
    pub async fn send_outbound(
        client: &reqwest::Client,
        callback_url: &str,
        secret: &str,
        message: &OutboundMessage,
    ) -> ChannelResult<SendResult> {
        let payload = Self::build_outbound_payload(message);
        let body_bytes = serde_json::to_vec(&payload).map_err(|e| {
            ChannelError::SendFailed(format!("Failed to serialize outbound payload: {e}"))
        })?;

        let signature = WebhookReceiver::compute_signature(secret, &body_bytes);

        let resp = client
            .post(callback_url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Signature", &signature)
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Webhook POST failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let resp_body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::SendFailed(format!(
                "Webhook callback error ({status}): {resp_body}"
            )));
        }

        Ok(SendResult {
            message_id: MessageId::new(uuid::Uuid::new_v4().to_string()),
            timestamp: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::OutboundMessage;

    #[test]
    fn test_parse_inbound_valid_full() {
        let body = serde_json::to_vec(&serde_json::json!({
            "sender_id": "user-123",
            "sender_name": "Alice",
            "message": "Hello from webhook!",
            "conversation_id": "conv-abc",
            "thread_id": "thread-1",
            "is_group": true,
            "metadata": {"key": "value"}
        }))
        .unwrap();

        let channel_id = ChannelId::new("webhook");
        let messages = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.channel_id.as_str(), "webhook");
        assert_eq!(msg.sender_id.as_str(), "user-123");
        assert_eq!(msg.sender_name.as_deref(), Some("Alice"));
        assert_eq!(msg.text, "Hello from webhook!");
        assert_eq!(msg.conversation_id.as_str(), "conv-abc");
        assert_eq!(msg.reply_to.as_ref().unwrap().as_str(), "thread-1");
        assert!(msg.is_group);
        // message ID should be a valid UUID
        assert!(uuid::Uuid::parse_str(msg.id.as_str()).is_ok());
    }

    #[test]
    fn test_parse_inbound_minimal() {
        let body = serde_json::to_vec(&serde_json::json!({
            "message": "Just a message"
        }))
        .unwrap();

        let channel_id = ChannelId::new("webhook");
        let messages = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.sender_id.as_str(), "webhook-user");
        assert_eq!(msg.sender_name.as_deref(), Some("Webhook User"));
        assert_eq!(msg.text, "Just a message");
        // conversation_id defaults to sender_id when not provided
        assert_eq!(msg.conversation_id.as_str(), "webhook-user");
        assert!(msg.reply_to.is_none());
        assert!(!msg.is_group);
    }

    #[test]
    fn test_parse_inbound_batch() {
        let body = serde_json::to_vec(&serde_json::json!([
            {
                "sender_id": "user-1",
                "sender_name": "Alice",
                "message": "First message"
            },
            {
                "sender_id": "user-2",
                "sender_name": "Bob",
                "message": "Second message"
            }
        ]))
        .unwrap();

        let channel_id = ChannelId::new("webhook");
        let messages = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "First message");
        assert_eq!(messages[0].sender_id.as_str(), "user-1");
        assert_eq!(messages[1].text, "Second message");
        assert_eq!(messages[1].sender_id.as_str(), "user-2");

        // Each message should have a unique ID
        assert_ne!(messages[0].id.as_str(), messages[1].id.as_str());
    }

    #[test]
    fn test_parse_inbound_skips_empty_messages() {
        let body = serde_json::to_vec(&serde_json::json!([
            {"sender_id": "user-1", "message": ""},
            {"sender_id": "user-2", "message": "Valid message"}
        ]))
        .unwrap();

        let channel_id = ChannelId::new("webhook");
        let messages = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "Valid message");
    }

    #[test]
    fn test_parse_inbound_invalid_json() {
        let body = b"not json at all";
        let channel_id = ChannelId::new("webhook");
        let result = WebhookMessageOps::parse_inbound_payload(body, &channel_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_inbound_missing_message_field() {
        let body = serde_json::to_vec(&serde_json::json!({
            "sender_id": "user-123"
        }))
        .unwrap();

        let channel_id = ChannelId::new("webhook");
        let result = WebhookMessageOps::parse_inbound_payload(&body, &channel_id);
        // serde should fail because `message` is a required field
        assert!(result.is_err());
    }

    #[test]
    fn test_build_outbound_payload() {
        let message = OutboundMessage::text("conv-abc", "Hello from Aleph!");
        let payload = WebhookMessageOps::build_outbound_payload(&message);

        assert_eq!(payload["sender_id"], "aleph");
        assert_eq!(payload["sender_name"], "Aleph");
        assert_eq!(payload["message"], "Hello from Aleph!");
        assert_eq!(payload["conversation_id"], "conv-abc");
        assert!(payload["thread_id"].is_null());
        assert_eq!(payload["is_group"], false);
    }

    #[test]
    fn test_build_outbound_payload_with_reply() {
        let message = OutboundMessage::text("conv-abc", "Reply text")
            .with_reply_to("thread-42");
        let payload = WebhookMessageOps::build_outbound_payload(&message);

        assert_eq!(payload["thread_id"], "thread-42");
    }

    #[test]
    fn test_build_outbound_payload_with_metadata() {
        let mut message = OutboundMessage::text("conv-abc", "With metadata");
        message.metadata.insert("key".to_string(), "value".to_string());
        let payload = WebhookMessageOps::build_outbound_payload(&message);

        assert_eq!(payload["metadata"]["key"], "value");
    }

    #[test]
    fn test_parse_inbound_conversation_id_defaults_to_sender() {
        let body = serde_json::to_vec(&serde_json::json!({
            "sender_id": "custom-sender",
            "message": "Test"
        }))
        .unwrap();

        let channel_id = ChannelId::new("webhook");
        let messages = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();
        assert_eq!(messages[0].conversation_id.as_str(), "custom-sender");
    }

    #[test]
    fn test_parse_inbound_conversation_id_explicit() {
        let body = serde_json::to_vec(&serde_json::json!({
            "sender_id": "user-1",
            "message": "Test",
            "conversation_id": "explicit-conv"
        }))
        .unwrap();

        let channel_id = ChannelId::new("webhook");
        let messages = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();
        assert_eq!(messages[0].conversation_id.as_str(), "explicit-conv");
    }

    #[test]
    fn test_webhook_payload_serde() {
        let payload = WebhookPayload {
            sender_id: "user-1".to_string(),
            sender_name: "Alice".to_string(),
            message: "Hello".to_string(),
            conversation_id: Some("conv-1".to_string()),
            thread_id: None,
            is_group: false,
            metadata: HashMap::new(),
        };

        let json = serde_json::to_string(&payload).unwrap();
        let deserialized: WebhookPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.sender_id, "user-1");
        assert_eq!(deserialized.message, "Hello");
    }
}
