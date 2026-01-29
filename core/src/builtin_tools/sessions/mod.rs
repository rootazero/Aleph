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
//! use aethecore::builtin_tools::sessions::{SessionsListTool, SessionsListArgs};
//! use aethecore::builtin_tools::sessions::{SessionsSendTool, SessionsSendArgs};
//! use aethecore::gateway::context::GatewayContext;
//! use aethecore::tools::AetherTool;
//!
//! // Create tools with gateway context
//! let list_tool = SessionsListTool::new(gateway_context.clone(), "main");
//! let send_tool = SessionsSendTool::with_context(gateway_context, "main");
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
//!
//! let result = send_tool.call(send_args).await?;
//! println!("Reply: {:?}", result.reply);
//! ```

pub mod helpers;
#[cfg(feature = "gateway")]
pub mod list_tool;
pub mod send_tool;

pub use helpers::{
    classify_session_kind, derive_channel, parse_session_key, resolve_display_key, SessionKind,
};

#[cfg(feature = "gateway")]
pub use list_tool::{
    SessionListRow, SessionsListArgs, SessionsListOutput, SessionsListTool,
};

pub use send_tool::{
    SessionsSendArgs, SessionsSendOutput, SessionsSendStatus, SessionsSendTool,
};
