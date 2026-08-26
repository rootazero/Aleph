//! Standing-goal subsystem: a persistent user objective with lifecycle +
//! budget, managed by the LLM via the `goal` tool (R8), re-surfaced each
//! turn by `StandingGoalLayer`. Distinct from the per-task `scratchpad`.

pub mod pursuit;
pub mod store;
pub mod types;

pub use store::{ContinuationDecision, FieldUpdate, FireDecision, GoalStore, RearmDecision};
pub use types::{GateOutcome, Goal, GoalStatus, PursuitMode};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

/// Process-global goal store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as
/// "no goal subsystem" and the prompt layer stays dormant.
///
/// `ConsumerDecides`, and it is the shape by weight of evidence: **21**
/// production call sites, each writing its own meaning for `None`. The
/// standing goal block silently vanishes from the prompt
/// (`context_blocks.rs`), a loop tick's fire decision comes back absent
/// (`execute.rs`), the budget gate stops gating (`goal_budget.rs`), and
/// `users.update`'s deactivation freeze reports `goals: 0`, which reads as
/// "this principal owned none".
static GLOBAL: CapabilitySlot<Arc<GoalStore>> =
    CapabilitySlot::new("goal/store", MissingSemantics::ConsumerDecides);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_slot() -> &'static dyn SlotStatus {
    &GLOBAL
}

/// Install the global store at boot. Idempotent: a second call is ignored.
pub fn init_global(store: Arc<GoalStore>) {
    let _ = GLOBAL.install(store);
}

/// Read the global store, if initialized.
pub fn global() -> Option<Arc<GoalStore>> {
    GLOBAL.get().cloned()
}

/// Test-only override. In production `init_global` is the only writer.
#[cfg(test)]
pub fn set_global_for_test(store: Arc<GoalStore>) {
    let _ = GLOBAL.install(store);
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

    /// The variant is the operator-facing severity of this handle going
    /// missing (`FailsOpen` => Error and a non-zero `aleph doctor`;
    /// `IndistinguishableDefault` / `ConsumerDecides` => Warning;
    /// `FailsClosed` => Info), and it is DERIVED from the consumers named on
    /// the static above. Pinned in the module that owns the handle, because
    /// that is the only place a reclassification and a re-read of those
    /// consumers can be made to happen together — the aggregate figure in
    /// FEATURE_LOCATOR cannot tell a reclassification from a new slot.
    /// `census::every_slot_pins_its_own_missing_semantics` requires this by
    /// slot id.
    #[test]
    fn the_store_slot_pins_its_missing_semantics() {
        assert_eq!(global_slot().id(), "goal/store");
        assert!(
            matches!(global_slot().missing(), MissingSemantics::ConsumerDecides),
            "`goal/store` is classified ConsumerDecides from its consumers; changing that \
             means re-reading them, not re-typing this line"
        );
    }
}
