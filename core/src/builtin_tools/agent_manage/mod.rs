//! Agent management tools — create, switch, list, delete agents at runtime.

pub mod create;
pub mod delete;
pub mod list;
pub mod switch;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Shared session context injected by ExecutionEngine each run.
///
/// Carries the channel and peer_id of the current conversation so that
/// agent management tools can auto-switch the active agent for the caller.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub channel: String,
    pub peer_id: String,
}

pub type SessionContextHandle = Arc<RwLock<SessionContext>>;

pub fn new_session_context_handle() -> SessionContextHandle {
    Arc::new(RwLock::new(SessionContext::default()))
}

pub use create::{AgentCreateArgs, AgentCreateOutput, AgentCreateTool};
pub use delete::{AgentDeleteArgs, AgentDeleteOutput, AgentDeleteTool};
pub use list::{AgentListArgs, AgentListInfo, AgentListOutput, AgentListTool};
pub use switch::{AgentSwitchArgs, AgentSwitchOutput, AgentSwitchTool};
