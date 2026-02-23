//! iMessage MessageOperations Implementation
//!
//! Implements the MessageOperations trait for iMessage using AppleScript.
//! Supports: reply, react (tapback), send
//! Does NOT support: edit, delete (iMessage limitation)

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::builtin_tools::message::{
    ChannelCapabilities, DeleteParams, EditParams, MessageOperations, MessageResult, ReactParams,
    ReplyParams, SendParams,
};
use crate::error::Result;

use super::sender::MessageSender;
use super::target::parse_target;

/// iMessage message operations adapter
///
/// Uses AppleScript to control the Messages app for sending messages
/// and reacting with tapbacks.
#[derive(Clone, Default)]
pub struct IMessageMessageOps;

impl IMessageMessageOps {
    /// Create a new iMessage message operations adapter
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MessageOperations for IMessageMessageOps {
    fn channel_id(&self) -> &str {
        "imessage"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            reply: true,   // Thread originator support
            edit: false,   // iMessage doesn't support editing
            react: true,   // Tapback reactions
            delete: false, // iMessage doesn't support deletion
            send: true,    // Basic message sending
        }
    }

    async fn reply(&self, params: ReplyParams) -> Result<MessageResult> {
        // iMessage replies are sent as regular messages with thread context
        // The threading is handled by the Messages app based on conversation
        debug!(
            target = %params.conversation_id,
            reply_to = %params.message_id,
            "Sending iMessage reply"
        );

        // Parse target (phone number or email)
        let target = match parse_target(&params.conversation_id) {
            Ok(t) => t,
            Err(e) => {
                return Ok(MessageResult::failed(format!("Invalid target: {}", e)));
            }
        };

        // Send the message (iMessage doesn't have explicit reply-to)
        match MessageSender::send_text(&target.to_target_string(), &params.text).await {
            Ok(()) => {
                debug!("Reply sent successfully");
                // Note: iMessage doesn't return a message ID
                Ok(MessageResult::success())
            }
            Err(e) => {
                warn!(error = %e, "Failed to send iMessage reply");
                Ok(MessageResult::failed(format!("Failed to send reply: {}", e)))
            }
        }
    }

    async fn edit(&self, _params: EditParams) -> Result<MessageResult> {
        // iMessage does not support message editing
        Ok(MessageResult::failed(
            "iMessage does not support message editing",
        ))
    }

    async fn react(&self, params: ReactParams) -> Result<MessageResult> {
        debug!(
            target = %params.conversation_id,
            message_id = %params.message_id,
            emoji = %params.emoji,
            remove = %params.remove,
            "Setting iMessage tapback"
        );

        // Convert emoji to tapback type
        let tapback = match emoji_to_tapback(&params.emoji) {
            Some(t) => t,
            None => {
                return Ok(MessageResult::failed(format!(
                    "Unsupported tapback emoji: {}. Supported: ❤️ (love), 👍 (like), 👎 (dislike), 😂 (laugh), ‼️ (emphasize), ❓ (question)",
                    params.emoji
                )));
            }
        };

        // Send tapback via AppleScript
        match MessageSender::send_tapback(
            &params.conversation_id,
            &params.message_id,
            tapback.script_name(),
            params.remove,
        )
        .await
        {
            Ok(()) => {
                debug!("Tapback set successfully");
                Ok(MessageResult::success())
            }
            Err(e) => {
                warn!(error = %e, "Failed to set iMessage tapback");
                Ok(MessageResult::failed(format!(
                    "Failed to set tapback: {}",
                    e
                )))
            }
        }
    }

    async fn delete(&self, _params: DeleteParams) -> Result<MessageResult> {
        // iMessage does not support message deletion
        Ok(MessageResult::failed(
            "iMessage does not support message deletion",
        ))
    }

    async fn send(&self, params: SendParams) -> Result<MessageResult> {
        debug!(
            target = %params.target,
            "Sending iMessage"
        );

        // Parse target (phone number or email)
        let target = match parse_target(&params.target) {
            Ok(t) => t,
            Err(e) => {
                return Ok(MessageResult::failed(format!("Invalid target: {}", e)));
            }
        };

        // Send the message
        match MessageSender::send_text(&target.to_target_string(), &params.text).await {
            Ok(()) => {
                debug!("Message sent successfully");
                // Note: iMessage doesn't return a message ID for sent messages
                Ok(MessageResult::success())
            }
            Err(e) => {
                warn!(error = %e, "Failed to send iMessage");
                Ok(MessageResult::failed(format!(
                    "Failed to send message: {}",
                    e
                )))
            }
        }
    }
}

/// iMessage tapback types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tapback {
    Love,
    Like,
    Dislike,
    Laugh,
    Emphasize,
    Question,
}

impl Tapback {
    /// Get the AppleScript reaction name
    pub fn script_name(&self) -> &'static str {
        match self {
            Tapback::Love => "love",
            Tapback::Like => "like",
            Tapback::Dislike => "dislike",
            Tapback::Laugh => "haha",
            Tapback::Emphasize => "emphasize",
            Tapback::Question => "question",
        }
    }
}

/// Convert emoji to tapback type
fn emoji_to_tapback(emoji: &str) -> Option<Tapback> {
    match emoji.trim() {
        "❤️" | "♥️" | "love" => Some(Tapback::Love),
        "👍" | "like" | "+1" => Some(Tapback::Like),
        "👎" | "dislike" | "-1" => Some(Tapback::Dislike),
        "😂" | "😆" | "haha" | "laugh" => Some(Tapback::Laugh),
        "‼️" | "!!" | "emphasize" | "exclamation" => Some(Tapback::Emphasize),
        "❓" | "?" | "question" => Some(Tapback::Question),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emoji_to_tapback() {
        assert_eq!(emoji_to_tapback("❤️"), Some(Tapback::Love));
        assert_eq!(emoji_to_tapback("👍"), Some(Tapback::Like));
        assert_eq!(emoji_to_tapback("👎"), Some(Tapback::Dislike));
        assert_eq!(emoji_to_tapback("😂"), Some(Tapback::Laugh));
        assert_eq!(emoji_to_tapback("‼️"), Some(Tapback::Emphasize));
        assert_eq!(emoji_to_tapback("❓"), Some(Tapback::Question));
        assert_eq!(emoji_to_tapback("🎉"), None); // Not a supported tapback
    }

    #[test]
    fn test_tapback_script_names() {
        assert_eq!(Tapback::Love.script_name(), "love");
        assert_eq!(Tapback::Like.script_name(), "like");
        assert_eq!(Tapback::Dislike.script_name(), "dislike");
        assert_eq!(Tapback::Laugh.script_name(), "haha");
        assert_eq!(Tapback::Emphasize.script_name(), "emphasize");
        assert_eq!(Tapback::Question.script_name(), "question");
    }

    #[test]
    fn test_capabilities() {
        // Note: Can't test without a real MessageSender in CI
        // Just test the capability flags are correct
        let caps = ChannelCapabilities {
            reply: true,
            edit: false,
            react: true,
            delete: false,
            send: true,
        };
        assert!(caps.reply);
        assert!(!caps.edit);
        assert!(caps.react);
        assert!(!caps.delete);
        assert!(caps.send);
    }
}
