//! Agent management tools — create, switch, list, info, delete, unbind, update
//! agents at runtime.
//!
//! Module layout:
//! - `error` — typed [`AgentManageError`]; single reason per failure mode.
//! - `validation` — agent-ID grammar + slug-from-name generator (shared by
//!   `agent_create` and `AgentManager` so the LLM-side and TOML-side errors
//!   agree byte-for-byte).
//! - `context` — [`AgentManageContext`] builder; the construction-time seam
//!   that owns the optional dependencies (event bus, TOML manager, raw-memory
//!   writer) so each tool only sees what it actually uses.
//! - `test_utils` — shared fixtures; `#[cfg(test)]` only.
//! - `create` / `delete` / `list` / `info` / `switch` / `unbind` / `update`
//!   — the actual `AlephTool` implementations.
//!
//! Single-source binding operations live in `crate::gateway::agent_binding`;
//! the tools here are its callers. Single-source lifecycle event publishing
//! lives in `crate::gateway::agent_lifecycle::AgentLifecycleEvent::publish`.

pub mod context;
pub mod create;
pub mod delete;
pub mod error;
pub mod info;
pub mod list;
pub mod switch;
pub mod unbind;
pub mod update;
pub mod validation;

#[cfg(test)]
pub(crate) mod test_utils;

use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

pub use context::AgentManageContext;
pub use create::{AgentCreateArgs, AgentCreateOutput, AgentCreateTool};
pub use delete::{AgentDeleteArgs, AgentDeleteOutput, AgentDeleteTool};
pub use error::AgentManageError;
pub use info::{AgentInfoArgs, AgentInfoOutput, AgentInfoTool};
pub use list::{AgentListArgs, AgentListInfo, AgentListOutput, AgentListTool};
pub use switch::{AgentSwitchArgs, AgentSwitchOutput, AgentSwitchTool};
pub use unbind::{AgentUnbindArgs, AgentUnbindOutput, AgentUnbindTool};
pub use update::{AgentUpdateArgs, AgentUpdateOutput, AgentUpdateTool};
pub use validation::{generate_agent_id_from_name, validate_agent_id};

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

/// Convert an [`AgentManageError`] into the project's general error type.
///
/// Centralised so the LLM-facing message and the future `reason_code` /
/// structured-log path share one rendering function — and so a future
/// addition (e.g. `agent_create`'s `guard_reasons` field) doesn't have to
/// touch five call sites. The reason code is preserved via
/// [`AgentManageError::reason_code`] for callers that want a stable
/// machine-readable tag (Panel badges, structured logs, model retry policy).
impl From<AgentManageError> for crate::error::AlephError {
    fn from(err: AgentManageError) -> Self {
        // Log the structured reason so server-side traces carry the stable
        // code; user-visible text stays in the message field.
        tracing::debug!(
            reason = err.reason_code(),
            error = %err,
            "agent_manage operation refused"
        );
        crate::error::AlephError::other(err.to_string())
    }
}
