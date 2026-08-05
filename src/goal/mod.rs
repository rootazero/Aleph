//! Standing-goal subsystem: a persistent user objective with lifecycle +
//! budget, managed by the LLM via the `goal` tool (R8), re-surfaced each
//! turn by `StandingGoalLayer`. Distinct from the per-task `scratchpad`.

pub mod pursuit;
pub mod store;
pub mod types;

pub use store::{ContinuationDecision, FieldUpdate, FireDecision, GoalStore, RearmDecision};
pub use types::{GateOutcome, Goal, GoalStatus, PursuitMode};

use crate::sync_primitives::Arc;
use once_cell::sync::OnceCell;

/// Process-global goal store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as
/// "no goal subsystem" and the prompt layer stays dormant.
static GLOBAL: OnceCell<Arc<GoalStore>> = OnceCell::new();

/// Install the global store at boot. Idempotent: a second call is ignored.
pub fn init_global(store: Arc<GoalStore>) {
    let _ = GLOBAL.set(store);
}

/// Read the global store, if initialized.
pub fn global() -> Option<Arc<GoalStore>> {
    GLOBAL.get().cloned()
}

/// Test-only override. In production `init_global` is the only writer.
#[cfg(test)]
pub fn set_global_for_test(store: Arc<GoalStore>) {
    let _ = GLOBAL.set(store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_then_global_returns_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        set_global_for_test(store.clone());
        assert!(global().is_some());
    }
}
