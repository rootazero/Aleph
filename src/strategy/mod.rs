//! Strategic-planner subsystem: a welded `Strategy` (the StraTA application-layer
//! pattern) minted once at the top of `/goal` · `/loop` · `/workflow`, stored
//! persistently, and pinned into every downstream execution prompt. Distinct
//! from the standing `goal` (objective) and the per-task `scratchpad`.

pub mod planner;
pub mod render;
pub mod store;
pub mod types;

pub use render::{render_guardrails_only, render_strategy_summary, render_workflow_global_frame};
pub use store::StrategyStore;
pub use types::Strategy;

use once_cell::sync::OnceCell;

use crate::sync_primitives::Arc;

/// Composite-key prefix for a `/goal`-flow strategy, keyed by session.
#[must_use]
pub fn goal_key(session_id: &str) -> String {
    format!("goal:{session_id}")
}

/// Composite-key prefix for a `/loop`-flow strategy, keyed by session. Distinct
/// from `goal_key` so a session running both flows never clobbers either row.
#[must_use]
pub fn loop_key(session_id: &str) -> String {
    format!("loop:{session_id}")
}

// NOTE: there is deliberately NO `workflow_key` here. A workflow run's
// strategy frame rides the materialised tasks' metadata
// (`WORKFLOW_STRATEGY_KEY`, rendered by `build_handoff_context`) — a
// store row keyed by run id had zero readers (`resolve_active_strategy`
// resolves goal → loop → team → session only) and leaked one row per run,
// so it was removed (R10 YAGNI).

/// Composite-key prefix for a NAKED-loop (plain interactive chat) strategy,
/// keyed by session. Lowest precedence in `active_strategy` (goal > loop >
/// session) so an explicit `/goal` or `/loop` strategy in a reused session
/// always wins. Pass the canonical `SessionKey::to_key_string()` form so the
/// weld layers and the subagent weld read the same row.
#[must_use]
pub fn session_key(session_id: &str) -> String {
    format!("session:{session_id}")
}

/// Composite-key prefix for a TEAM group-chat strategy, keyed by team (a team
/// strategy is team-wide, not per-member-session).
/// Resolved in `active_strategy` BETWEEN `loop_key` and `session_key`: a
/// member's own `/goal` or `/loop` strategy still wins, but the leader's team
/// frame beats a bare session strategy. Callers MUST pass the NORMALIZED team
/// id (the form `SessionKey::task` stores in a `team_chat` key) so the planner
/// write and the weld read hit the same row.
#[must_use]
pub fn team_key(team_id: &str) -> String {
    format!("team:{team_id}")
}

/// Process-global strategy store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as "no
/// strategy subsystem" and the prompt layers stay dormant.
static GLOBAL: OnceCell<Arc<StrategyStore>> = OnceCell::new();

/// Install the global store at boot. Idempotent: a second call is ignored.
/// Holds an `Arc` (mirroring `goal::init_global`) so the boot constructor, the
/// `strategy` tool, and the lifecycle clears all share one store instance.
pub fn init_global(store: Arc<StrategyStore>) {
    let _ = GLOBAL.set(store);
}

/// Read the global store, if initialized (a cheap `Arc` clone).
#[must_use]
pub fn global() -> Option<Arc<StrategyStore>> {
    GLOBAL.get().cloned()
}

/// Test-only override. In production `init_global` is the only writer.
#[cfg(test)]
pub fn set_global_for_test(store: Arc<StrategyStore>) {
    let _ = GLOBAL.set(store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_key_is_prefixed() {
        assert_eq!(goal_key("sess-1"), "goal:sess-1");
    }

    #[test]
    fn loop_key_is_prefixed() {
        assert_eq!(loop_key("sess-1"), "loop:sess-1");
    }

    #[test]
    fn goal_and_loop_keys_for_same_session_differ() {
        // CRITICAL: a session running /goal AND /loop concurrently must not
        // collide — composite keying is the whole point.
        assert_ne!(goal_key("sess-1"), loop_key("sess-1"));
    }

    #[test]
    fn session_key_is_prefixed() {
        assert_eq!(session_key("sess-1"), "session:sess-1");
    }

    #[test]
    fn session_key_distinct_from_goal_and_loop() {
        // Naked-loop key must not collide with the explicit-flow keys.
        assert_ne!(session_key("s"), goal_key("s"));
        assert_ne!(session_key("s"), loop_key("s"));
    }

    #[test]
    fn team_key_is_team_prefixed() {
        assert_eq!(super::team_key("squad-1"), "team:squad-1");
    }

    #[test]
    fn init_then_global_returns_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(StrategyStore::open(&dir.path().join("strat.db")).unwrap());
        set_global_for_test(store);
        assert!(global().is_some());
    }
}
