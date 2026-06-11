//! `CALLER_ROLE` task-local — carries the originating gateway connection's
//! authorization role across the in-task hop from the WS dispatch loop into
//! [`AgentRunManager::start_run`](crate::gateway::handlers::agent::AgentRunManager),
//! which copies it into the run's metadata BEFORE `tokio::spawn`. From there it
//! rides [`TurnContext`](crate::tools::turn_context::TurnContext) to the
//! tool-dispatch config-tier gate.
//!
//! Scoped only around `process_request` in the authenticated dispatch path
//! (`server::handler`). Never crosses the run's spawn boundary (`start_run` reads
//! it while still in-task). Unset for non-gateway callers (cron, internal) and,
//! by design, for the local no-auth daemon — the gate treats absent role as
//! trusted.

use tokio::task_local;

task_local! {
    /// Originating connection role: `Some("operator")` / `Some("guest")`, or
    /// `None` when auth is not required (local daemon) or outside any dispatch.
    pub static CALLER_ROLE: Option<String>;
}

/// The originating connection's role for the current task, or `None` outside a
/// `CALLER_ROLE` scope (non-gateway / internal runs) — trusted by the gate.
#[must_use]
pub fn current_caller_role() -> Option<String> {
    CALLER_ROLE.try_with(|r| r.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scope_round_trips() {
        let seen = CALLER_ROLE
            .scope(Some("guest".to_string()), async { current_caller_role() })
            .await;
        assert_eq!(seen.as_deref(), Some("guest"));
    }

    #[tokio::test]
    async fn unset_is_none() {
        assert_eq!(current_caller_role(), None);
    }
}
