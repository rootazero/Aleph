//! Process-global capability handles that can say whether they were installed.
//!
//! # The problem
//!
//! A bare `static X: OnceLock<Arc<T>>` plus `install_x()` plus `x()` cannot
//! distinguish "boot never installed this" from "boot installed exactly this
//! value" — §5.22 round-7 recorded the shape on `spend`: `spend.query` reports
//! `configured: false`, which is a true statement about a box with no ceiling
//! AND a true statement about a box that configured one whose handle was never
//! installed. That round fixed two handles by hand. There are 46.
//!
//! # The shape
//!
//! [`CapabilitySlot::install`] writes the value **and** stamps the outcome in
//! one act: a caller that cannot reach the inner `OnceLock` cannot forget the
//! stamp. This is the `MetaGuard` idiom (make the correct thing the only
//! constructible thing), not a "remember to also call `mark()`" discipline —
//! that discipline fails in exactly the shape this type prevents, and its
//! failure mode is a *confident lie* ("not installed" about an installed
//! handle), which is worse than today's silence.
//!
//! [`CapabilitySlot::decline`] is the other half and the reason this round
//! exists: boot's conditional-install `else` arms now have somewhere to say
//! **why**. That is deepseek-harness/Cordis's unsatisfied `static inject`
//! ("waiting for: sessionPersistence") in Rust's shape — no plugin tree, no
//! topological boot, just the sentence a reader needs.

use std::sync::OnceLock;

/// What a read observes when this capability was NEVER installed.
///
/// ⚠️ Membership in the roster is decided by THIS — the failure direction —
/// not by the handle's type or its name. A handle belongs iff losing it yields
/// a *wrong answer* rather than a crash. The 63 lazy caches in `src/` cannot
/// write an honest variant here ("not built yet" is not a wrong answer), which
/// is why the Task 6 rule excludes them by derivation rather than by a
/// hand-written exclusion list that would rot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSemantics {
    /// A read yields a legal-looking value and no caller can tell.
    /// (`spend` policy reads as "no ceiling" — the round-7 shape.)
    IndistinguishableDefault { reads_as: &'static str },
    /// A read yields `None` and every consumer decides for itself what that
    /// means. (`GLOBAL_SESSION_SERVICE`: 9 consumers, one silently returns.)
    ConsumerDecides,
    /// Fails closed — safe, but the feature is dead and says nothing.
    FailsClosed,
    /// Fails OPEN — a gate silently stops gating.
    FailsOpen,
}

/// What boot did about this slot, when boot reached it at all.
///
/// `None` (no outcome recorded) is a third state and is NOT `Declined`: it
/// means nothing ever reached this slot — either this process did not boot, or
/// boot died before getting here. Collapsing the two is the mistake this type
/// exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    /// Boot reached this slot and could not install it. `because` is shown to
    /// operators verbatim, so name the missing input, not the symptom.
    Declined { because: &'static str },
}

/// Type-erased view of a slot, for the roster and the diagnostics check.
pub trait SlotStatus: Sync {
    fn id(&self) -> &'static str;
    /// By value: `MissingSemantics` is `Copy`, which keeps this trait free of
    /// lifetime gymnastics and of any `unsafe`.
    fn missing(&self) -> MissingSemantics;
    fn outcome(&self) -> Option<&Outcome>;
}

/// Install-once capability handle. Replaces a bare `static X: OnceLock<T>`.
pub struct CapabilitySlot<T: 'static> {
    id: &'static str,
    missing: MissingSemantics,
    value: OnceLock<T>,
    outcome: OnceLock<Outcome>,
}

impl<T: 'static> CapabilitySlot<T> {
    #[must_use]
    pub const fn new(id: &'static str, missing: MissingSemantics) -> Self {
        Self { id, missing, value: OnceLock::new(), outcome: OnceLock::new() }
    }

    /// Install the value and stamp the roster. Returns `false` when already
    /// installed — the same idempotence the `set_*` / `init_*` functions this
    /// replaces already promised in their doc comments.
    pub fn install(&'static self, v: T) -> bool {
        let fresh = self.value.set(v).is_ok();
        if fresh {
            let _ = self.outcome.set(Outcome::Installed);
        }
        fresh
    }

    /// Record that boot reached this slot and could not install it.
    ///
    /// First writer wins, mirroring `install`: a slot is decided once.
    pub fn decline(&'static self, because: &'static str) {
        let _ = self.outcome.set(Outcome::Declined { because });
    }

    /// Read the installed value.
    ///
    /// This is `OnceLock::get()` and nothing else — the stamp is written only
    /// on the `install`/`decline` side, so migrating a hot handle onto this
    /// type does not add a branch or an atomic to any read.
    #[inline]
    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }

    #[must_use]
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.get()
    }
}

impl<T: Send + Sync + 'static> SlotStatus for CapabilitySlot<T> {
    fn id(&self) -> &'static str {
        self.id
    }
    fn missing(&self) -> MissingSemantics {
        self.missing
    }
    fn outcome(&self) -> Option<&Outcome> {
        self.outcome.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static UNSET: CapabilitySlot<u32> =
        CapabilitySlot::new("test/unset", MissingSemantics::ConsumerDecides);
    static INSTALLED: CapabilitySlot<u32> =
        CapabilitySlot::new("test/installed", MissingSemantics::FailsOpen);
    static DECLINED: CapabilitySlot<u32> = CapabilitySlot::new(
        "test/declined",
        MissingSemantics::IndistinguishableDefault { reads_as: "0" },
    );

    #[test]
    fn an_untouched_slot_reports_no_outcome_at_all() {
        // The distinction this whole type exists for: "nobody reached it" is
        // NOT "installed", and it is NOT "declined".
        assert!(UNSET.get().is_none());
        assert!(UNSET.outcome().is_none());
    }

    #[test]
    fn install_writes_the_value_and_stamps_the_roster_in_one_act() {
        assert!(INSTALLED.install(7));
        assert_eq!(INSTALLED.get(), Some(&7));
        assert!(matches!(INSTALLED.outcome(), Some(Outcome::Installed)));
    }

    #[test]
    fn install_is_idempotent_like_the_setters_it_replaces() {
        static S: CapabilitySlot<u32> =
            CapabilitySlot::new("test/idem", MissingSemantics::FailsClosed);
        assert!(S.install(1));
        assert!(!S.install(2), "second install must be a no-op returning false");
        assert_eq!(S.get(), Some(&1));
    }

    #[test]
    fn decline_records_why_and_leaves_the_value_unset() {
        DECLINED.decline("state database absent: [gateway] state_db unset");
        assert!(DECLINED.get().is_none());
        match DECLINED.outcome() {
            Some(Outcome::Declined { because }) => {
                assert!(because.contains("state_db"));
            }
            other => panic!("expected Declined, got {other:?}"),
        }
    }

    #[test]
    fn slot_status_erases_the_type_for_the_roster() {
        let erased: &'static dyn SlotStatus = &UNSET;
        assert_eq!(erased.id(), "test/unset");
        assert!(matches!(erased.missing(), MissingSemantics::ConsumerDecides));
    }
}
