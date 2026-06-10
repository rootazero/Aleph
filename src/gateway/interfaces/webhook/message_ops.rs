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
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Webhook message JSON payload (used for both inbound parsing and outbound building).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Optional stable message identifier supplied by the sender.
    ///
    /// When present it is used verbatim as the inbound message ID so the
    /// downstream dedup tracker can drop provider retries. Accepts the common
    /// aliases `id` and `idempotency_key` for ergonomic integration.
    #[serde(
        default,
        alias = "id",
        alias = "idempotency_key",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_id: Option<String>,

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
    /// Convenience wrapper around [`Self::parse_inbound_payload_with_hint`] with
    /// no delivery hint. Retained for backward compatibility.
    pub fn parse_inbound_payload(
        body: &[u8],
        channel_id: &ChannelId,
    ) -> ChannelResult<Vec<InboundMessage>> {
        Self::parse_inbound_payload_with_hint(body, channel_id, None)
    }

    /// Parse inbound webhook payload, deriving a *stable* ID per message.
    ///
    /// The body is expected to be a single JSON object or an array of objects.
    /// Each object must contain at least a `message` field.
    ///
    /// `delivery_hint` is a per-request idempotency identifier extracted from an
    /// HTTP header (e.g. `Idempotency-Key`). Together with any payload-supplied
    /// `message_id`, it lets the downstream dedup tracker drop provider retries —
    /// previously every delivery received a fresh UUID, so retries were never
    /// deduplicated and triggered duplicate agent runs.
    pub fn parse_inbound_payload_with_hint(
        body: &[u8],
        channel_id: &ChannelId,
        delivery_hint: Option<&str>,
    ) -> ChannelResult<Vec<InboundMessage>> {
        // Try parsing as array first, then as single object
        let payloads: Vec<WebhookPayload> = if let Ok(arr) =
            serde_json::from_slice::<Vec<WebhookPayload>>(body)
        {
            arr
        } else {
            let single: WebhookPayload = serde_json::from_slice(body)
                .map_err(|e| ChannelError::ReceiveFailed(format!("Invalid webhook JSON: {e}")))?;
            vec![single]
        };

        let mut messages = Vec::with_capacity(payloads.len());

        for (index, payload) in payloads.into_iter().enumerate() {
            if payload.message.is_empty() {
                continue; // Skip empty messages
            }

            let msg_id = Self::derive_message_id(&payload, delivery_hint, index);
            let conversation_id = payload
                .conversation_id
                .clone()
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
                metadata: vec![],
            });
        }

        Ok(messages)
    }

    /// Derive a *stable* inbound message ID using a multi-signal precedence,
    /// porting the idempotency-key/fingerprint fallback used by openclaw
    /// (`hooks.ts`) and hermes-agent (`_IdempotencyCache`):
    ///
    /// 1. an explicit `message_id` in the payload (integrator-supplied),
    /// 2. a per-request delivery hint from an idempotency HTTP header,
    ///    suffixed with the batch index so distinct elements stay unique,
    /// 3. a deterministic content fingerprint over the routing-relevant fields.
    ///
    /// All three branches are deterministic, so an identical retried delivery
    /// maps to the same ID and is dropped by the inbound dedup tracker.
    fn derive_message_id(
        payload: &WebhookPayload,
        delivery_hint: Option<&str>,
        index: usize,
    ) -> String {
        if let Some(explicit) = payload
            .message_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return explicit.to_string();
        }

        if let Some(hint) = delivery_hint.map(str::trim).filter(|s| !s.is_empty()) {
            return format!("{hint}#{index}");
        }

        // Content fingerprint: identical payloads (i.e. retries) collide; any
        // change in routing-relevant fields yields a distinct ID.
        let mut hasher = Sha256::new();
        hasher.update(payload.sender_id.as_bytes());
        hasher.update([0u8]);
        hasher.update(payload.conversation_id.as_deref().unwrap_or("").as_bytes());
        hasher.update([0u8]);
        hasher.update(payload.thread_id.as_deref().unwrap_or("").as_bytes());
        hasher.update([0u8]);
        hasher.update(payload.message.as_bytes());
        format!("wh-{}", hex::encode(hasher.finalize()))
    }

    /// Build outbound JSON payload from an OutboundMessage.
    #[must_use]
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
        // message ID is now a deterministic content fingerprint, not a UUID
        assert!(msg.id.as_str().starts_with("wh-"));
    }

    // --- Stable / deterministic message-id derivation (dedup correctness) ---

    #[test]
    fn test_inbound_id_is_deterministic_for_identical_payload() {
        // A retried webhook delivery (byte-identical body) must yield the SAME
        // message id so the downstream dedup tracker drops it.
        let body = serde_json::to_vec(&serde_json::json!({
            "sender_id": "user-1",
            "message": "ping",
            "conversation_id": "conv-1"
        }))
        .unwrap();
        let channel_id = ChannelId::new("webhook");

        let first = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();
        let second = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();

        assert_eq!(first[0].id.as_str(), second[0].id.as_str());
    }

    #[test]
    fn test_inbound_id_differs_for_different_content() {
        let channel_id = ChannelId::new("webhook");
        let a = WebhookMessageOps::parse_inbound_payload(
            &serde_json::to_vec(&serde_json::json!({"sender_id": "u", "message": "one"})).unwrap(),
            &channel_id,
        )
        .unwrap();
        let b = WebhookMessageOps::parse_inbound_payload(
            &serde_json::to_vec(&serde_json::json!({"sender_id": "u", "message": "two"})).unwrap(),
            &channel_id,
        )
        .unwrap();

        assert_ne!(a[0].id.as_str(), b[0].id.as_str());
    }

    #[test]
    fn test_inbound_id_honors_explicit_message_id() {
        let body = serde_json::to_vec(&serde_json::json!({
            "message_id": "delivery-42",
            "sender_id": "user-1",
            "message": "hi"
        }))
        .unwrap();
        let channel_id = ChannelId::new("webhook");
        let msgs = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();
        assert_eq!(msgs[0].id.as_str(), "delivery-42");
    }

    #[test]
    fn test_inbound_id_honors_idempotency_key_alias() {
        let body = serde_json::to_vec(&serde_json::json!({
            "idempotency_key": "evt-99",
            "sender_id": "user-1",
            "message": "hi"
        }))
        .unwrap();
        let channel_id = ChannelId::new("webhook");
        let msgs = WebhookMessageOps::parse_inbound_payload(&body, &channel_id).unwrap();
        assert_eq!(msgs[0].id.as_str(), "evt-99");
    }

    #[test]
    fn test_inbound_id_uses_delivery_hint_with_unique_batch_index() {
        // Same hint applied to a batch: distinct elements keep distinct ids.
        let body = serde_json::to_vec(&serde_json::json!([
            {"sender_id": "u", "message": "first"},
            {"sender_id": "u", "message": "second"}
        ]))
        .unwrap();
        let channel_id = ChannelId::new("webhook");
        let msgs = WebhookMessageOps::parse_inbound_payload_with_hint(
            &body,
            &channel_id,
            Some("X-Delivery-123"),
        )
        .unwrap();

        assert_eq!(msgs[0].id.as_str(), "X-Delivery-123#0");
        assert_eq!(msgs[1].id.as_str(), "X-Delivery-123#1");
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
        let message = OutboundMessage::text("conv-abc", "Reply text").with_reply_to("thread-42");
        let payload = WebhookMessageOps::build_outbound_payload(&message);

        assert_eq!(payload["thread_id"], "thread-42");
    }

    #[test]
    fn test_build_outbound_payload_with_metadata() {
        let mut message = OutboundMessage::text("conv-abc", "With metadata");
        message
            .metadata
            .insert("key".to_string(), "value".to_string());
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
            message_id: None,
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
