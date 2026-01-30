//! Message Tool Types
//!
//! Core types for cross-channel message operations (reply, edit, react, delete).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;

// ============================================================================
// Message Actions
// ============================================================================

/// Supported message actions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageAction {
    /// Reply to an existing message
    Reply,
    /// Edit an existing message
    Edit,
    /// Add/remove reaction to a message
    React,
    /// Delete a message
    Delete,
    /// Send a new message (not a reply)
    Send,
}

impl MessageAction {
    /// Get the action name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reply => "reply",
            Self::Edit => "edit",
            Self::React => "react",
            Self::Delete => "delete",
            Self::Send => "send",
        }
    }
}

impl std::fmt::Display for MessageAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// Action Parameters
// ============================================================================

/// Parameters for replying to a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyParams {
    /// Channel identifier (e.g., "telegram", "discord")
    pub channel: String,
    /// Conversation/chat ID
    pub conversation_id: String,
    /// Message ID to reply to
    pub message_id: String,
    /// Reply text content
    pub text: String,
}

/// Parameters for editing a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditParams {
    /// Channel identifier
    pub channel: String,
    /// Conversation/chat ID
    pub conversation_id: String,
    /// Message ID to edit
    pub message_id: String,
    /// New text content
    pub text: String,
}

/// Parameters for reacting to a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactParams {
    /// Channel identifier
    pub channel: String,
    /// Conversation/chat ID
    pub conversation_id: String,
    /// Message ID to react to
    pub message_id: String,
    /// Emoji to react with (e.g., "👍", "❤️")
    pub emoji: String,
    /// Whether to remove the reaction (default: false = add)
    #[serde(default)]
    pub remove: bool,
}

/// Parameters for deleting a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteParams {
    /// Channel identifier
    pub channel: String,
    /// Conversation/chat ID
    pub conversation_id: String,
    /// Message ID to delete
    pub message_id: String,
}

/// Parameters for sending a new message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendParams {
    /// Channel identifier
    pub channel: String,
    /// Target (conversation ID or user ID)
    pub target: String,
    /// Message text content
    pub text: String,
}

// ============================================================================
// Result Types
// ============================================================================

/// Result of a message operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResult {
    /// Whether the operation succeeded
    pub success: bool,
    /// ID of the sent/edited message (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Error message if operation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MessageResult {
    /// Create a successful result
    pub fn success() -> Self {
        Self {
            success: true,
            message_id: None,
            error: None,
        }
    }

    /// Create a successful result with message ID
    pub fn success_with_id(message_id: impl Into<String>) -> Self {
        Self {
            success: true,
            message_id: Some(message_id.into()),
            error: None,
        }
    }

    /// Create a failed result
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            success: false,
            message_id: None,
            error: Some(error.into()),
        }
    }
}

// ============================================================================
// Channel Capabilities
// ============================================================================

/// Capabilities of a channel for message operations
#[derive(Debug, Clone, Default)]
pub struct ChannelCapabilities {
    /// Supports reply action
    pub reply: bool,
    /// Supports edit action
    pub edit: bool,
    /// Supports react action
    pub react: bool,
    /// Supports delete action
    pub delete: bool,
    /// Supports send action
    pub send: bool,
}

impl ChannelCapabilities {
    /// Create capabilities where all actions are supported
    pub fn all() -> Self {
        Self {
            reply: true,
            edit: true,
            react: true,
            delete: true,
            send: true,
        }
    }

    /// Check if an action is supported
    pub fn supports(&self, action: MessageAction) -> bool {
        match action {
            MessageAction::Reply => self.reply,
            MessageAction::Edit => self.edit,
            MessageAction::React => self.react,
            MessageAction::Delete => self.delete,
            MessageAction::Send => self.send,
        }
    }

    /// Get list of supported actions
    pub fn supported_actions(&self) -> Vec<MessageAction> {
        let mut actions = Vec::new();
        if self.reply {
            actions.push(MessageAction::Reply);
        }
        if self.edit {
            actions.push(MessageAction::Edit);
        }
        if self.react {
            actions.push(MessageAction::React);
        }
        if self.delete {
            actions.push(MessageAction::Delete);
        }
        if self.send {
            actions.push(MessageAction::Send);
        }
        actions
    }
}

// ============================================================================
// Message Operations Trait
// ============================================================================

/// Trait for channel-specific message operations
///
/// Implement this trait for each channel adapter to enable
/// cross-channel message operations.
#[async_trait]
pub trait MessageOperations: Send + Sync {
    /// Get the channel identifier (e.g., "telegram", "discord")
    fn channel_id(&self) -> &str;

    /// Get the capabilities of this channel
    fn capabilities(&self) -> ChannelCapabilities;

    /// Reply to a message
    async fn reply(&self, params: ReplyParams) -> Result<MessageResult>;

    /// Edit a message
    async fn edit(&self, params: EditParams) -> Result<MessageResult>;

    /// React to a message
    async fn react(&self, params: ReactParams) -> Result<MessageResult>;

    /// Delete a message
    async fn delete(&self, params: DeleteParams) -> Result<MessageResult>;

    /// Send a new message
    async fn send(&self, params: SendParams) -> Result<MessageResult>;
}

// ============================================================================
// Tool Arguments and Output
// ============================================================================

/// Arguments for the message tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MessageToolArgs {
    /// The action to perform
    pub action: MessageAction,

    /// Target channel (e.g., "telegram", "discord", "imessage")
    /// If not specified, uses the current channel context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,

    /// Target identifier (conversation ID, user ID, or channel:conversation format)
    pub target: String,

    /// Message ID for reply/edit/react/delete operations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,

    /// Text content for reply/edit/send operations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Emoji for react operation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,

    /// Whether to remove a reaction (for react action)
    #[serde(default)]
    pub remove: bool,
}

/// Output from the message tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MessageToolOutput {
    /// The action that was performed
    pub action: MessageAction,
    /// Whether the operation succeeded
    pub success: bool,
    /// ID of the sent/edited message (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Channel that was used
    pub channel: String,
    /// Error message if operation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MessageToolOutput {
    /// Create a successful output
    pub fn success(action: MessageAction, channel: impl Into<String>) -> Self {
        Self {
            action,
            success: true,
            message_id: None,
            channel: channel.into(),
            error: None,
        }
    }

    /// Create a successful output with message ID
    pub fn success_with_id(
        action: MessageAction,
        channel: impl Into<String>,
        message_id: impl Into<String>,
    ) -> Self {
        Self {
            action,
            success: true,
            message_id: Some(message_id.into()),
            channel: channel.into(),
            error: None,
        }
    }

    /// Create a failed output
    pub fn failed(
        action: MessageAction,
        channel: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            action,
            success: false,
            message_id: None,
            channel: channel.into(),
            error: Some(error.into()),
        }
    }

    /// Create an unsupported action error
    pub fn unsupported(action: MessageAction, channel: impl Into<String>) -> Self {
        let channel = channel.into();
        Self {
            action,
            success: false,
            message_id: None,
            channel: channel.clone(),
            error: Some(format!(
                "Action '{}' is not supported on channel '{}'",
                action, channel
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_action_serialization() {
        assert_eq!(
            serde_json::to_string(&MessageAction::Reply).unwrap(),
            r#""reply""#
        );
        assert_eq!(
            serde_json::to_string(&MessageAction::Edit).unwrap(),
            r#""edit""#
        );
        assert_eq!(
            serde_json::to_string(&MessageAction::React).unwrap(),
            r#""react""#
        );
        assert_eq!(
            serde_json::to_string(&MessageAction::Delete).unwrap(),
            r#""delete""#
        );
        assert_eq!(
            serde_json::to_string(&MessageAction::Send).unwrap(),
            r#""send""#
        );
    }

    #[test]
    fn test_message_action_deserialization() {
        let reply: MessageAction = serde_json::from_str(r#""reply""#).unwrap();
        assert_eq!(reply, MessageAction::Reply);
    }

    #[test]
    fn test_message_result_success() {
        let result = MessageResult::success();
        assert!(result.success);
        assert!(result.message_id.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_message_result_success_with_id() {
        let result = MessageResult::success_with_id("msg-123");
        assert!(result.success);
        assert_eq!(result.message_id, Some("msg-123".to_string()));
    }

    #[test]
    fn test_message_result_failed() {
        let result = MessageResult::failed("Something went wrong");
        assert!(!result.success);
        assert_eq!(result.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_channel_capabilities_all() {
        let caps = ChannelCapabilities::all();
        assert!(caps.supports(MessageAction::Reply));
        assert!(caps.supports(MessageAction::Edit));
        assert!(caps.supports(MessageAction::React));
        assert!(caps.supports(MessageAction::Delete));
        assert!(caps.supports(MessageAction::Send));
    }

    #[test]
    fn test_channel_capabilities_partial() {
        let caps = ChannelCapabilities {
            reply: true,
            edit: false,
            react: true,
            delete: false,
            send: true,
        };
        assert!(caps.supports(MessageAction::Reply));
        assert!(!caps.supports(MessageAction::Edit));
        assert!(caps.supports(MessageAction::React));
        assert!(!caps.supports(MessageAction::Delete));
        assert!(caps.supports(MessageAction::Send));

        let actions = caps.supported_actions();
        assert_eq!(actions.len(), 3);
        assert!(actions.contains(&MessageAction::Reply));
        assert!(actions.contains(&MessageAction::React));
        assert!(actions.contains(&MessageAction::Send));
    }

    #[test]
    fn test_message_tool_args_serialization() {
        let args = MessageToolArgs {
            action: MessageAction::Reply,
            channel: Some("telegram".to_string()),
            target: "chat123".to_string(),
            message_id: Some("msg456".to_string()),
            text: Some("Hello!".to_string()),
            emoji: None,
            remove: false,
        };

        let json = serde_json::to_string(&args).unwrap();
        assert!(json.contains(r#""action":"reply""#));
        assert!(json.contains(r#""channel":"telegram""#));
    }

    #[test]
    fn test_message_tool_output_success() {
        let output = MessageToolOutput::success(MessageAction::Reply, "telegram");
        assert!(output.success);
        assert_eq!(output.action, MessageAction::Reply);
        assert_eq!(output.channel, "telegram");
    }

    #[test]
    fn test_message_tool_output_unsupported() {
        let output = MessageToolOutput::unsupported(MessageAction::Edit, "imessage");
        assert!(!output.success);
        assert!(output.error.unwrap().contains("not supported"));
    }
}
