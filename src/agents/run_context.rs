//! Per-run active-agent inheritance via a `tokio::task_local`.
//!
//! Mirrors [`crate::projects::run_context`] (project-root inheritance) for the
//! *agent identity* of the run currently executing. The gateway run loop sets
//! it once per run from the resolved `AgentInstance::id`, so any tool invoked
//! inside that run can resolve agent-scoped resources — in particular the
//! per-agent skills directory `~/.aleph/agents/<id>/skills`
//! ([`crate::utils::paths::get_all_skills_dirs`]).
//!
//! Why a task-local instead of threading `agent_id` through every tool
//! constructor: the skill tools (`skill_list` / `skill_read`) are built without
//! agent context at the dispatch chokepoint, exactly like project mode. A
//! task-local keeps the wiring to one scope at the run boundary.
//!
//! Best-effort everywhere: callers outside any [`with_agent_id`] scope get
//! `None` and agent-scoped resources are simply skipped — byte-identical to the
//! pre-wiring behaviour. Children spawned via `tokio::spawn` must capture the
//! value before the spawn boundary (read [`current_agent_id`] at the call site
//! and re-establish the scope in the new task).

tokio::task_local! {
    static CURRENT_AGENT_ID: Option<String>;
}

/// Run `fut` with the given agent id visible to [`current_agent_id`] for the
/// lifetime of the future. The scope is bounded by the future, so once it
/// resolves the task-local reverts to whatever the parent stack had.
pub async fn with_agent_id<F, T>(agent_id: Option<String>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_AGENT_ID.scope(agent_id, fut).await
}

/// Read the active agent id, if any. Returns `None` outside a [`with_agent_id`]
/// scope or when the surrounding scope explicitly set `None`.
#[must_use]
pub fn current_agent_id() -> Option<String> {
    CURRENT_AGENT_ID.try_with(|a| a.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn current_is_none_outside_any_scope() {
        assert!(current_agent_id().is_none());
    }

    #[tokio::test]
    async fn current_returns_set_value_inside_scope() {
        let observed =
            with_agent_id(Some("researcher".to_string()), async { current_agent_id() }).await;
        assert_eq!(observed, Some("researcher".to_string()));
    }

    #[tokio::test]
    async fn explicit_none_scope_reads_none() {
        let observed = with_agent_id(None, async { current_agent_id() }).await;
        assert_eq!(observed, None);
    }
}
