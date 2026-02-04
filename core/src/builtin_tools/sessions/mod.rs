//! Session management tools for cross-session communication.
//!
//! This module provides tools and helper functions for managing and interacting
//! with sessions, enabling agent-to-agent communication.
//!
//! # Tools
//!
//! - [`SessionsListTool`] - List accessible sessions for discovery
//! - [`SessionsSendTool`] - Send messages to other sessions (same or different agent)
//! - [`SessionsSpawnTool`] - Spawn sub-agent sessions for delegated tasks
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
//! use alephcore::builtin_tools::sessions::{SessionsSpawnTool, SessionsSpawnArgs};
//! use alephcore::gateway::context::GatewayContext;
//! use alephcore::tools::AlephTool;
//!
//! // Create tools with gateway context
//! let list_tool = SessionsListTool::new(gateway_context.clone(), "main");
//! let send_tool = SessionsSendTool::with_context(gateway_context.clone(), "main");
//! let spawn_tool = SessionsSpawnTool::with_context(
//!     gateway_context,
//!     "main",
//!     vec!["*".to_string()], // Allow spawning any agent
//! );
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
//!
//! // Spawn a sub-agent for a delegated task
//! let spawn_args = SessionsSpawnArgs {
//!     task: "Analyze this code and provide suggestions".to_string(),
//!     label: Some("code-reviewer".to_string()),
//!     agent_id: Some("reviewer".to_string()),
//!     model: None,
//!     thinking: None,
//!     run_timeout_seconds: 120,
//!     cleanup: CleanupPolicy::Ephemeral,
//! };
//! let spawn_result = spawn_tool.call(spawn_args).await?;
//! println!("Spawned: {:?}", spawn_result.child_session_key);
//! ```

pub mod helpers;
#[cfg(feature = "gateway")]
pub mod list_tool;
#[cfg(feature = "gateway")]
pub mod send_tool;
#[cfg(feature = "gateway")]
pub mod spawn_tool;

pub use helpers::{
    classify_session_kind, derive_channel, parse_session_key, resolve_display_key, SessionKind,
};

#[cfg(feature = "gateway")]
pub use list_tool::{
    SessionListRow, SessionsListArgs, SessionsListOutput, SessionsListTool,
};

#[cfg(feature = "gateway")]
pub use send_tool::{
    SessionsSendArgs, SessionsSendOutput, SessionsSendStatus, SessionsSendTool,
};

#[cfg(feature = "gateway")]
pub use spawn_tool::{
    CleanupPolicy, SessionsSpawnArgs, SessionsSpawnOutput, SessionsSpawnTool, SpawnStatus,
};
