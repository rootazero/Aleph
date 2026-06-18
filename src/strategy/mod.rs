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

/// Composite-key prefix for a `/workflow`-flow strategy, keyed by run (a
/// workflow run is run-wide, not session-wide).
#[must_use]
pub fn workflow_key(run_id: &str) -> String {
    format!("workflow:{run_id}")
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
    fn workflow_key_uses_run_id() {
        assert_eq!(workflow_key("run-abc"), "workflow:run-abc");
    }

    #[test]
    fn goal_and_loop_keys_for_same_session_differ() {
        // CRITICAL: a session running /goal AND /loop concurrently must not
        // collide — composite keying is the whole point.
        assert_ne!(goal_key("sess-1"), loop_key("sess-1"));
    }

    #[test]
    fn init_then_global_returns_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(StrategyStore::open(&dir.path().join("strat.db")).unwrap());
        set_global_for_test(store);
        assert!(global().is_some());
    }
}
