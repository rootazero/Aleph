//! Agent management tools — create, switch, list, delete agents at runtime.

pub mod create;
pub mod delete;
pub mod info;
pub mod list;
pub mod switch;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Shared session context injected by `ExecutionEngine` each run.
///
/// Carries the channel and `peer_id` of the current conversation so that
/// agent management tools can auto-switch the active agent for the caller.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub channel: String,
    pub peer_id: String,
    /// Serialized session key string (e.g. "main:default:0" or "main:dm:telegram:user123:0")
    pub session_key_str: String,
    /// Conversation ID within the channel (e.g. Telegram `chat_id`)
    pub conversation_id: String,
}

pub type SessionContextHandle = Arc<RwLock<SessionContext>>;

#[must_use]
pub fn new_session_context_handle() -> SessionContextHandle {
    Arc::new(RwLock::new(SessionContext::default()))
}

pub use create::{AgentCreateArgs, AgentCreateOutput, AgentCreateTool};
pub use delete::{AgentDeleteArgs, AgentDeleteOutput, AgentDeleteTool};
pub use info::{AgentInfoArgs, AgentInfoOutput, AgentInfoTool};
pub use list::{AgentListArgs, AgentListInfo, AgentListOutput, AgentListTool};
pub use switch::{AgentSwitchArgs, AgentSwitchOutput, AgentSwitchTool};
