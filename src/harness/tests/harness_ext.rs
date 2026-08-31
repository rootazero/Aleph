//! Test-only single-turn convenience over [`AgentHarness`].
//!
//! `run_turn` drives exactly one Think→Act turn, recomputing the loop's
//! iteration / tool-call counters from the full event log so a test can fire a
//! turn without threading the outer loop's state. It has **zero production
//! callers** — production drives the loop exclusively through
//! `AgentHarness::run` → `run_turn_internal`. Living in `agent.rs`, this test
//! affordance and its two counter helpers cost the R10 line budget (the frozen
//! `src/harness/tests/budget.rs` ceiling) for something no shipping code path
//! touches. It lives here instead — outside the 12-file / CEILING budget — as an
//! extension trait, so the ~50 call sites keep the identical `harness.run_turn(…)`
//! shape while the loop pays down its ceiling.

use crate::harness::trait_def::HarnessError;
use crate::harness::{AgentHarness, HarnessCallback, TurnState};
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::session::service::SessionId;

/// One-shot turn driver, available only in the harness test tree.
#[allow(
    async_fn_in_trait,
    reason = "test-only trait with a single concrete implementor; dyn dispatch is never used so the Send-bound / object-safety footgun of async-fn-in-trait does not apply"
)]
pub(crate) trait AgentHarnessTestExt {
    /// One Think→Act turn; returns whether the session should continue.
    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError>;
}

impl AgentHarnessTestExt for AgentHarness {
    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError> {
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let iterations = count_assistant_messages(&events).saturating_add(1);
        let tool_calls_made = count_tool_calls(&events);
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut history = std::collections::VecDeque::new();
        self.run_turn_internal(
            session_id,
            callback,
            iterations,
            tool_calls_made,
            &mut history,
            &cancel,
        )
        .await
        .map(|step| step.state)
    }
}

/// Count `AssistantMessage` events in the log.
fn count_assistant_messages(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count()
}

/// Count `ToolCallRequested` events.
fn count_tool_calls(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolCallRequested { .. }))
        .count()
}
