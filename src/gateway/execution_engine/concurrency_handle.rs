//! Process-global handle to the live `ConcurrencyLimiter`, mirroring
//! `providers::route_handle`. The limiter is built once inside the
//! `ExecutionEngine`; a `[execution]` config patch alone never reaches it.
//! This handle lets `self_config` hot-apply new run caps on the next
//! admission — no daemon restart (the task's hot-state requirement).
//!
//! Holds a `Weak` so a torn-down engine doesn't keep the limiter alive;
//! `reconfigure_global` is a no-op returning `false` when nothing is live.

use std::sync::Weak;

use super::concurrency::ConcurrencyLimiter;
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

/// `FailsClosed`: the sole reader is [`reconfigure_global`], whose `false`
/// feeds `live_apply`'s `landed`, which downgrades the `execution` section's
/// verdict from `Live` to `Restart`. So the operator IS told the resize did not
/// take — the same honest downgrade `spend::update_policy` gives. What the
/// `false` cannot say is WHICH of its two causes fired: no engine ever
/// installed a limiter, or the engine installed one and has since been dropped.
/// That is the distinction the outcome stamp adds, and the reason this handle
/// is on the roster despite already failing honestly.
static HANDLE: CapabilitySlot<Weak<ConcurrencyLimiter>> =
    CapabilitySlot::new("gateway/concurrency-limiter", MissingSemantics::FailsClosed);

/// Register the engine's limiter once (idempotent). Called from
/// `ExecutionEngine::new`.
pub(super) fn install_global(limiter: &Arc<ConcurrencyLimiter>) {
    let _ = HANDLE.install(Arc::downgrade(limiter));
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn concurrency_limiter_slot() -> &'static dyn SlotStatus {
    &HANDLE
}

/// Live-resize the global run caps. Returns `false` if no engine is installed
/// or it has been dropped (the caller reports "no live limiter").
pub fn reconfigure_global(global_cap: usize, per_agent_cap: usize) -> bool {
    match HANDLE.get().and_then(Weak::upgrade) {
        Some(limiter) => {
            limiter.reconfigure(global_cap, per_agent_cap);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = concurrency_limiter_slot();
        assert_eq!(slot.id(), "gateway/concurrency-limiter");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }
}
