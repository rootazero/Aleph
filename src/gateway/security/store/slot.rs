//! The process-global handle to the users table, for code that runs with
//! no request in hand.
//!
//! One consumer today: the fire-time authority resolver
//! (`crate::scope::authority::resolve`), which every background trigger asks
//! "is the person this work acts for still here, and in what role?". A
//! trigger has no gateway dispatch around it, so it cannot be handed the
//! store the way an RPC handler is; threading it through each executor's
//! constructor is the per-executor parameter chain this slot replaces.

use super::SecurityStore;
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

/// `FailsOpen`, named as what it is: with nothing installed the resolver
/// answers `Legacy` for every subject, so a deactivated principal's cron
/// job fires and a demoted admin's heartbeat runs as operator — the
/// fire-time gate silently stops gating. That is also exactly the
/// behaviour tests and minimal servers have always had (they never had a
/// store to ask), which is why the variant is honest rather than alarming
/// there, and why boot installs this unconditionally: `initialize_vault`
/// always yields a store (on disk, else in memory), so there is no decline
/// arm to write.
static USERS_STORE: CapabilitySlot<Arc<SecurityStore>> =
    CapabilitySlot::new("security/users-store", MissingSemantics::FailsOpen);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn users_store_slot() -> &'static dyn SlotStatus {
    &USERS_STORE
}

/// Install the process-wide users store. Idempotent — a second call is
/// ignored (mirrors `spend::install_ledger`). Boot calls this once, right
/// after `initialize_vault`, with the SAME `Arc` every other durable
/// consumer shares, so a deactivation or demotion `users.update` writes is
/// what the next background fire reads.
pub fn install_users_store(store: Arc<SecurityStore>) {
    let _ = USERS_STORE.install(store);
}

/// Read the installed store. `None` = boot never installed one; see the
/// static's doc for what the one consumer does with that.
#[must_use]
pub(crate) fn users_store() -> Option<Arc<SecurityStore>> {
    USERS_STORE.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `census::every_slot_pins_its_own_missing_semantics` requires this by
    /// slot id. `FailsOpen` is the variant that makes `aleph doctor` exit
    /// non-zero when the slot is missing; changing it means re-reading the
    /// resolver, not re-typing this line.
    #[test]
    fn the_users_store_slot_pins_its_missing_semantics() {
        assert_eq!(users_store_slot().id(), "security/users-store");
        assert!(
            matches!(users_store_slot().missing(), MissingSemantics::FailsOpen),
            "`security/users-store` is FailsOpen: an uninstalled store makes \
             every fire-time authority check answer Legacy"
        );
    }

    /// No lib test may install this process-global. Once installed it
    /// stays for the whole test binary, and every other test that fires an
    /// owned cron job or heartbeat would then resolve its owner against
    /// that store — and be refused as `Gone`. Tests inject a store through
    /// `scope::authority::resolve_with` instead.
    #[test]
    fn no_lib_test_installs_the_process_global_users_store() {
        assert!(
            users_store().is_none(),
            "some lib test called install_users_store; use resolve_with"
        );
    }
}
