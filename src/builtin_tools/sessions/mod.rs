//! Session management tools for cross-session communication.
//!
//! This module provides tools and helper functions for managing and interacting
//! with sessions, enabling agent-to-agent communication.
//!
//! # Tools
//!
//! - [`SessionsListTool`] - List accessible sessions for discovery
//! - [`SessionsSendTool`] - Send messages to other sessions (same or different agent)
//!
//! # Helper Functions
//!
//! - [`classify_session_kind`] - Classify a session key into its kind
//! - [`resolve_display_key`] - Format a session key for display
//! - [`parse_session_key`] - Parse a session key from its display format
//! - [`derive_channel`] - Extract the channel from a session key
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::builtin_tools::sessions::{SessionsListTool, SessionsListArgs};
//! use alephcore::builtin_tools::sessions::{SessionsSendTool, SessionsSendArgs};
//! use alephcore::gateway::context::GatewayContext;
//! use alephcore::tools::AlephTool;
//!
//! // Create tools with gateway context
//! let list_tool = SessionsListTool::new(gateway_context.clone(), "main");
//! let send_tool = SessionsSendTool::with_context(gateway_context.clone(), "main");
//!
//! // List accessible sessions
//! let list_args = SessionsListArgs {
//!     kinds: Some(vec!["main".to_string()]),
//!     limit: Some(10),
//!     active_minutes: None,
//!     message_limit: None,
//! };
//! let sessions = list_tool.call(list_args).await?;
//!
//! // Send message to another agent
//! let send_args = SessionsSendArgs {
//!     session_key: Some("agent:translator:main".to_string()),
//!     message: "Translate 'Hello' to French".to_string(),
//!     timeout_seconds: 30,
//! };
//! let result = send_tool.call(send_args).await?;
//! println!("Reply: {:?}", result.reply);
//! ```

pub mod compact_tool;
pub mod helpers;
pub mod list_tool;
pub mod new_tool;
pub mod send_tool;
pub mod set_topic_tool;

pub use helpers::{
    classify_session_kind, derive_channel, parse_session_key, resolve_display_key, SessionKind,
};

pub use list_tool::{SessionListRow, SessionsListArgs, SessionsListOutput, SessionsListTool};

pub use send_tool::{SessionsSendArgs, SessionsSendOutput, SessionsSendStatus, SessionsSendTool};

pub use compact_tool::{SessionCompactArgs, SessionCompactOutput, SessionCompactTool};
pub use new_tool::{SessionNewArgs, SessionNewOutput, SessionNewTool};
pub use set_topic_tool::{SessionSetTopicArgs, SessionSetTopicOutput, SessionSetTopicTool};
