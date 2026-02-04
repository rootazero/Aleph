//! Message Tool
//!
//! Unified cross-channel message operations for Aleph agents.
//!
//! # Overview
//!
//! The message tool provides a consistent interface for message operations
//! across different messaging channels (Telegram, Discord, iMessage, etc.).
//!
//! # Supported Actions
//!
//! | Action | Description |
//! |--------|-------------|
//! | `reply` | Reply to an existing message |
//! | `edit` | Edit an existing message |
//! | `react` | Add/remove reaction to a message |
//! | `delete` | Delete a message |
//! | `send` | Send a new message |
//!
//! # Channel Support Matrix
//!
//! | Channel | Reply | Edit | React | Delete | Send |
//! |---------|-------|------|-------|--------|------|
//! | Telegram | ✅ | ✅ | ✅ | ✅ | ✅ |
//! | Discord | ✅ | ✅ | ✅ | ✅ | ✅ |
//! | iMessage | ✅ | ❌ | ✅* | ❌ | ✅ |
//!
//! *iMessage supports tapback reactions only
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::builtin_tools::message::{MessageTool, MessageToolArgs, MessageAction};
//!
//! let tool = MessageTool::new(registry);
//! let args = MessageToolArgs {
//!     action: MessageAction::Reply,
//!     channel: Some("telegram".to_string()),
//!     target: "chat123".to_string(),
//!     message_id: Some("msg456".to_string()),
//!     text: Some("Thanks for your message!".to_string()),
//!     emoji: None,
//!     remove: false,
//! };
//!
//! let result = tool.call(args).await?;
//! ```

pub mod types;
pub mod tool;

// Re-exports
pub use types::{
    ChannelCapabilities, DeleteParams, EditParams, MessageAction, MessageOperations,
    MessageResult, MessageToolArgs, MessageToolOutput, ReactParams, ReplyParams, SendParams,
};
pub use tool::MessageTool;
