//! Approval Bridge - connects ExecApprovalManager with chat channels.
//!
//! Provides utilities for:
//! - Building approval inline keyboards
//! - Parsing callback data from button clicks
//! - Tracking sent approval messages

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::gateway::channel::{ConversationId, InlineButton, InlineKeyboard, MessageId};

use super::socket::ApprovalDecisionType;

/// Tracks a sent approval message for later editing
#[derive(Debug, Clone)]
pub struct SentApprovalMessage {
    /// Approval ID this message is for
    pub approval_id: String,
    /// Channel the message was sent to
    pub channel: String,
    /// Chat ID where the message was sent
    pub chat_id: ConversationId,
    /// Message ID for editing
    pub message_id: MessageId,
}

/// Bridge utilities for approval message handling
pub struct ApprovalBridge {
    /// Track sent messages for editing
    sent_messages: Arc<RwLock<HashMap<String, Vec<SentApprovalMessage>>>>,
}

impl Default for ApprovalBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalBridge {
    /// Create a new bridge
    pub fn new() -> Self {
        Self {
            sent_messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build inline keyboard for approval request
    ///
    /// Creates a keyboard with:
    /// - Row 1: [Allow Once] [Allow Always]
    /// - Row 2: [Deny]
    pub fn build_approval_keyboard(approval_id: &str) -> InlineKeyboard {
        InlineKeyboard::new()
            .row(vec![
                InlineButton {
                    text: "✅ Allow Once".into(),
                    callback_data: format!("approve:{}:once", approval_id),
                },
                InlineButton {
                    text: "✅ Allow Always".into(),
                    callback_data: format!("approve:{}:always", approval_id),
                },
            ])
            .button("❌ Deny", format!("approve:{}:deny", approval_id))
    }

    /// Parse callback data into (approval_id, decision)
    ///
    /// Expected format: "approve:{id}:{decision}"
    /// where decision is "once", "always", or "deny"
    pub fn parse_callback(data: &str) -> Option<(String, ApprovalDecisionType)> {
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() != 3 || parts[0] != "approve" {
            return None;
        }

        let approval_id = parts[1].to_string();
        let decision = match parts[2] {
            "once" => ApprovalDecisionType::AllowOnce,
            "always" => ApprovalDecisionType::AllowAlways,
            "deny" => ApprovalDecisionType::Deny,
            _ => return None,
        };

        Some((approval_id, decision))
    }

    /// Get the response text for a decision
    pub fn decision_response_text(decision: &ApprovalDecisionType) -> &'static str {
        match decision {
            ApprovalDecisionType::AllowOnce => "✅ Allowed (once)",
            ApprovalDecisionType::AllowAlways => "✅ Allowed (always)",
            ApprovalDecisionType::Deny => "❌ Denied",
        }
    }

    /// Format the status line after resolution
    pub fn format_status_line(decision: &ApprovalDecisionType, resolved_by: &str) -> String {
        match decision {
            ApprovalDecisionType::AllowOnce => {
                format!("\n\n✅ **Allowed** (once) by {}", resolved_by)
            }
            ApprovalDecisionType::AllowAlways => {
                format!("\n\n✅ **Allowed** (always) by {}", resolved_by)
            }
            ApprovalDecisionType::Deny => {
                format!("\n\n❌ **Denied** by {}", resolved_by)
            }
        }
    }

    /// Track a sent approval message
    pub async fn track_sent_message(&self, msg: SentApprovalMessage) {
        let mut messages = self.sent_messages.write().await;
        messages
            .entry(msg.approval_id.clone())
            .or_default()
            .push(msg);
    }

    /// Get sent messages for an approval
    pub async fn get_sent_messages(&self, approval_id: &str) -> Vec<SentApprovalMessage> {
        let messages = self.sent_messages.read().await;
        messages.get(approval_id).cloned().unwrap_or_default()
    }

    /// Remove tracked messages for an approval
    pub async fn remove_sent_messages(&self, approval_id: &str) {
        let mut messages = self.sent_messages.write().await;
        messages.remove(approval_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_callback_allow_once() {
        let result = ApprovalBridge::parse_callback("approve:abc123:once");
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "abc123");
        assert!(matches!(decision, ApprovalDecisionType::AllowOnce));
    }

    #[test]
    fn test_parse_callback_allow_always() {
        let result = ApprovalBridge::parse_callback("approve:xyz789:always");
        assert!(result.is_some());
        let (id, decision) = result.unwrap();
        assert_eq!(id, "xyz789");
        assert!(matches!(decision, ApprovalDecisionType::AllowAlways));
    }

    #[test]
    fn test_parse_callback_deny() {
        let result = ApprovalBridge::parse_callback("approve:test:deny");
        assert!(result.is_some());
        let (_, decision) = result.unwrap();
        assert!(matches!(decision, ApprovalDecisionType::Deny));
    }

    #[test]
    fn test_parse_callback_invalid() {
        assert!(ApprovalBridge::parse_callback("invalid").is_none());
        assert!(ApprovalBridge::parse_callback("approve:only_two").is_none());
        assert!(ApprovalBridge::parse_callback("other:id:once").is_none());
        assert!(ApprovalBridge::parse_callback("approve:id:unknown").is_none());
    }

    #[test]
    fn test_build_approval_keyboard() {
        let keyboard = ApprovalBridge::build_approval_keyboard("test123");
        assert_eq!(keyboard.rows.len(), 2);
        assert_eq!(keyboard.rows[0].len(), 2); // Allow Once, Allow Always
        assert_eq!(keyboard.rows[1].len(), 1); // Deny
        assert!(keyboard.rows[0][0].callback_data.contains("test123"));
        assert!(keyboard.rows[0][0].callback_data.contains("once"));
        assert!(keyboard.rows[0][1].callback_data.contains("always"));
        assert!(keyboard.rows[1][0].callback_data.contains("deny"));
    }

    #[test]
    fn test_decision_response_text() {
        assert_eq!(
            ApprovalBridge::decision_response_text(&ApprovalDecisionType::AllowOnce),
            "✅ Allowed (once)"
        );
        assert_eq!(
            ApprovalBridge::decision_response_text(&ApprovalDecisionType::AllowAlways),
            "✅ Allowed (always)"
        );
        assert_eq!(
            ApprovalBridge::decision_response_text(&ApprovalDecisionType::Deny),
            "❌ Denied"
        );
    }

    #[tokio::test]
    async fn test_track_sent_messages() {
        let bridge = ApprovalBridge::new();

        let msg = SentApprovalMessage {
            approval_id: "test1".into(),
            channel: "telegram".into(),
            chat_id: ConversationId::new("123"),
            message_id: MessageId::new("456"),
        };

        bridge.track_sent_message(msg).await;

        let messages = bridge.get_sent_messages("test1").await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].channel, "telegram");
    }
}
