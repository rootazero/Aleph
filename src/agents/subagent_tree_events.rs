//! Live sub-agent tree event emission — the agents-side half of the panel's
//! background sub-agent tree.
//!
//! Pure observability: fire-and-forget broadcasts of [`SubagentTreeEvent`]s
//! onto the [`GlobalBus`] (scope = root session). The gateway
//! `subagent_tree_relay` forwards them to the panel's `run.subagent_tree`
//! stream. No reasoning happens here (R4/R10) — the harness only announces what
//! the tracker already recorded.

use crate::event::{AlephEvent, GlobalBus};
use aleph_protocol::subagent_tree::SubagentTreeEvent;

/// Wall-clock unix milliseconds for tree node timestamps. Saturates on clock
/// error rather than panicking.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Fire-and-forget broadcast of a tree event onto the `GlobalBus`. A no-op in
/// practice when no relay is subscribed (CLI / library tests / headless).
///
/// Skips emission when `session_id` is empty — there is no session to scope to
/// (mirrors the `SubAgentCompleted` announce's "no parent session" guard).
/// Must be called from within a Tokio runtime (every spawn / trace path is).
pub fn emit_tree_event(agent_id: String, session_id: String, ev: SubagentTreeEvent) {
    if session_id.is_empty() {
        return;
    }
    tokio::spawn(async move {
        GlobalBus::global()
            .broadcast(&agent_id, &session_id, AlephEvent::SubAgentTreeUpdate(ev))
            .await;
    });
}
