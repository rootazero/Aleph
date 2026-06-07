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
    /// Gateway run id of the running turn. Lets the config-approval gate emit a
    /// run-scoped "waiting for operator approval" notice on the requester's own
    /// output stream. Empty for non-gateway runs (cron, internal, tests) — the
    /// notice is then skipped (best-effort).
    pub run_id: String,
    /// Originating channel id (e.g. `telegram`). Empty for non-channel turns
    /// (cron, webhook, internal).
    pub channel_id: String,
    /// Originating conversation id. Empty for non-channel turns.
    pub conversation_id: String,
    /// Originating gateway connection's authorization role (`"operator"` /
    /// `"guest"`), stamped at run start from `CALLER_ROLE`. `None` for
    /// non-gateway runs (cron, internal) and for the local no-auth daemon —
    /// both treated as trusted by the config-tier gate.
    pub caller_role: Option<String>,
}

impl TurnContext {
    /// True when the turn has a reachable channel to deliver a prompt to.
    /// HITL tools must deny / fail gracefully when this is false.
    pub fn is_channel_routable(&self) -> bool {
        !self.channel_id.is_empty() && !self.conversation_id.is_empty()
    }

    /// True when the originating connection may mutate Aleph's own config.
    /// Absent role = trusted local/internal run; `"operator"` = config tier;
    /// any other value (e.g. `"guest"`) = chat tier (gated).
    pub fn caller_is_operator(&self) -> bool {
        match self.caller_role.as_deref() {
            None | Some("operator") => true,
            Some(_) => false,
        }
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

#[cfg(test)]
mod caller_tier_tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    fn ctx(role: Option<&str>) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("t"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: role.map(String::from),
        }
    }

    #[test]
    fn operator_and_local_are_operator() {
        assert!(ctx(Some("operator")).caller_is_operator());
        assert!(
            ctx(None).caller_is_operator(),
            "no role = trusted local/internal run"
        );
    }

    #[test]
    fn chat_tier_is_not_operator() {
        assert!(!ctx(Some("guest")).caller_is_operator());
    }
}
