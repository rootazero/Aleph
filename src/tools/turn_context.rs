//! `TURN_CONTEXT` task-local — carries the originating channel + session of
//! the agent turn currently executing a tool, so human-in-the-loop tools can
//! route a prompt back to the user.
//!
//! Scoped by [`ScopedToolService::execute`](crate::tools::scoped::ScopedToolService)
//! — the single production tool-dispatch chokepoint — for the duration of
//! every tool call. Reading it never crosses a `tokio::spawn` boundary, so
//! unlike `SESSION_ID` (which the gateway path never scopes) it is reliably
//! visible inside tool execution.
//!
//! Read by:
//! - the approval requester adapter (sandbox escalations + `requires_confirmation`)
//! - the `ask_user` clarification tool

use tokio::task_local;

use crate::routing::session_key::SessionKey;

/// Routing context of the agent turn currently executing a tool.
#[derive(Clone, Debug)]
pub struct TurnContext {
    /// Session key of the running turn — the key HITL managers register under.
    pub session_key: SessionKey,
    /// Originating channel id (e.g. `telegram`). Empty for non-channel turns
    /// (cron, webhook, internal).
    pub channel_id: String,
    /// Originating conversation id. Empty for non-channel turns.
    pub conversation_id: String,
}

impl TurnContext {
    /// True when the turn has a reachable channel to deliver a prompt to.
    /// HITL tools must deny / fail gracefully when this is false.
    pub fn is_channel_routable(&self) -> bool {
        !self.channel_id.is_empty() && !self.conversation_id.is_empty()
    }
}

task_local! {
    /// The routing context of the agent turn currently executing a tool.
    pub static TURN_CONTEXT: TurnContext;
}

/// Returns the current turn's routing context, or `None` outside a turn scope.
pub fn current_turn_context() -> Option<TurnContext> {
    TURN_CONTEXT.try_with(|t| t.clone()).ok()
}
