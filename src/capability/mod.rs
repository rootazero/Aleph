//! Process-global capability handles that can say whether they were installed.
//!
//! # The problem
//!
//! A bare `static X: OnceLock<Arc<T>>` plus `install_x()` plus `x()` cannot
//! distinguish "boot never installed this" from "boot installed exactly this
//! value" — §5.22 round-7 recorded the shape on `spend`: `spend.query` reports
//! `configured: false`, which is a true statement about a box with no ceiling
//! AND a true statement about a box that configured one whose handle was never
//! installed. That round fixed two handles by hand. There are 46 — counted by
//! the rule in `census`, not by hand.
//!
//! ⚠️ That 46 is NOT the 46 an earlier hand count reported, even though the
//! numbers agree: three members differ, and the decomposition is the tell — as
//! first derived on 2026-08-24, **before any migration**, this one was 45
//! written + 1 first-caller-wins and that one was 46 written. `census` names
//! all three and why each moved. Do not read the agreement as confirmation that
//! nothing changed.
//!
//! Those two figures are a first derivation, not a live reading: migration moves
//! members from `written` into `slots`, and the invariant is the SUM. Compare
//! against the live split `census`'s assertion prints on failure — never
//! against a figure written in prose, this paragraph included. An earlier draft
//! named the split as of the `spend` migration and would have gone stale at the
//! very next migrated handle: the same defect the sentence above qualifies,
//! reintroduced one paragraph lower by the fix for it.
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

use crate::sync_primitives::Arc;
use std::sync::OnceLock;

/// The membership rule that decides what belongs in this module's roster.
#[cfg(test)]
pub(crate) mod census;

/// What a read observes when this capability was NEVER installed.
///
/// ⚠️ Membership in the roster is decided by THIS — the failure direction —
/// not by the handle's type or its name. A handle belongs iff losing it yields
/// a *wrong answer* rather than a crash. The 64 lazy caches in `src/` cannot
/// write an honest variant here ("not built yet" is not a wrong answer), which
/// is why the Task 6 rule excludes them by derivation rather than by a
/// hand-written exclusion list that would rot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingSemantics {
    /// A read yields a legal-looking value and no caller can tell.
    /// (`spend` policy reads as "no ceiling" — the round-7 shape.)
    IndistinguishableDefault { reads_as: &'static str },
    /// A read yields `None` and every consumer decides for itself what that
    /// means. A per-handle census (how many, which ones agree) belongs on
    /// the handle's own `static`, not here — this is a general enum, and a
    /// count copied into it is a second figure nothing keeps in sync with
    /// the first.
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
    Declined {
        because: &'static str,
    },
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
    pub const fn new(id: &'static str, missing: MissingSemantics) -> Self {
        Self {
            id,
            missing,
            value: OnceLock::new(),
            outcome: OnceLock::new(),
        }
    }

    /// Install the value and stamp the roster. Returns `false` when already
    /// installed — the same idempotence the `set_*` / `init_*` functions this
    /// replaces already promised in their doc comments.
    ///
    /// ⚠️ ORDERING IS LOAD-BEARING: the stamp is written **after** `value.set`
    /// and **inside** `if fresh`, so a reader can never observe `Installed` on a
    /// slot whose value is not yet readable. Task 12 relies on this to trust
    /// `Installed` without re-reading the value. Hoisting the stamp above
    /// `value.set` opens a window in which another thread sees the stamp and
    /// `get()` returns `None` — and **no single-threaded test catches that**
    /// (verified by mutation: the whole suite stays green), so this comment is
    /// the only thing defending it.
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
    /// Forwards to the inherent method on purpose: both must exist (the trait
    /// one is only reachable through `&dyn SlotStatus`, the inherent one wins at
    /// every concrete call site), but two copies of one fact in a file four
    /// later tasks will edit is a divergence waiting to happen. One body.
    fn outcome(&self) -> Option<&Outcome> {
        CapabilitySlot::outcome(self)
    }
}

/// Install-once, then live-swap. Exactly one member today:
/// `spend::GLOBAL_POLICY` (`OnceLock<ArcSwap<SpendPolicy>>`, hot-applied by
/// the config live-reload path).
///
/// `update` returning `false` when nothing was installed is an EXISTING
/// contract, not a new one: `spend::update_policy` feeds the live-apply
/// verdict's honest downgrade to `Restart`. It is preserved exactly.
///
/// ⚠️ The withdrawal question this type shipped with — "if migration finds no
/// second member and `spend` could use `CapabilitySlot<ArcSwap<T>>` directly,
/// delete it (R10)" — was asked during the `spend` migration and answered NO.
/// There is still exactly one member, but the substitution is not equivalent:
/// with a bare `CapabilitySlot<ArcSwap<T>>` the hot-apply is
/// `get().map_or(false, |c| { c.store(..); true })` written at the call site,
/// so the `#[must_use] -> bool` below — the thing that stops a dropped return
/// from reporting a hot-apply that never happened — becomes something each
/// future call site must remember rather than something the type enforces.
/// That is the discipline-instead-of-construction failure the module doc opens
/// with. Re-ask if `update`'s return ever stops being load-bearing.
///
/// ⚠️ **No `decline` here, deliberately.** The symmetric method existed with
/// zero callers in the whole workspace — production *or* test — and was
/// withdrawn (R10) rather than left as a hook for later. It has no reachable
/// caller by construction, not by omission: `spend::install_policy` is called
/// unconditionally at boot, so this slot has no `else` arm for a decline to
/// live in, and it is the only member of the type. Restore it in the same
/// change that adds a conditional install, not before —
/// `census::every_public_slot_method_has_a_production_caller` will name it the
/// moment it goes back in without one.
pub struct MutableCapabilitySlot<T: 'static> {
    id: &'static str,
    missing: MissingSemantics,
    value: OnceLock<arc_swap::ArcSwap<T>>,
    outcome: OnceLock<Outcome>,
}

impl<T: 'static> MutableCapabilitySlot<T> {
    pub const fn new(id: &'static str, missing: MissingSemantics) -> Self {
        Self {
            id,
            missing,
            value: OnceLock::new(),
            outcome: OnceLock::new(),
        }
    }

    /// Install the value and stamp the roster.
    ///
    /// ⚠️ Same load-bearing ordering as [`CapabilitySlot::install`]: stamp after
    /// the value, inside `if fresh`.
    pub fn install(&'static self, v: T) -> bool {
        let fresh = self.value.set(arc_swap::ArcSwap::from_pointee(v)).is_ok();
        if fresh {
            let _ = self.outcome.set(Outcome::Installed);
        }
        fresh
    }

    /// Hot-apply a new value. `false` means no handle has been installed yet.
    ///
    /// `#[must_use]` because this is the one return in this API whose loss
    /// produces a lie: `src/config/live_apply.rs` binds it to `landed`, and
    /// `landed` decides between reporting the section applied `Live` and an
    /// honest downgrade to `Restart`. A caller who writes `SLOT.update(v);` and
    /// drops the bool reports a successful hot-apply for a store that never
    /// happened — the "报成功的 no-op" class this round exists to remove.
    #[must_use]
    pub fn update(&'static self, v: T) -> bool {
        match self.value.get() {
            Some(cell) => {
                cell.store(Arc::new(v));
                true
            }
            None => false,
        }
    }

    #[inline]
    pub fn load(&self) -> Option<arc_swap::Guard<Arc<T>>> {
        self.value.get().map(arc_swap::ArcSwap::load)
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.get()
    }
}

impl<T: Send + Sync + 'static> SlotStatus for MutableCapabilitySlot<T> {
    fn id(&self) -> &'static str {
        self.id
    }
    fn missing(&self) -> MissingSemantics {
        self.missing
    }
    /// Forwards to the inherent method — see [`CapabilitySlot`]'s impl.
    fn outcome(&self) -> Option<&Outcome> {
        MutableCapabilitySlot::outcome(self)
    }
}

/// Every capability slot in the process, for the `core/capability-wiring`
/// diagnostic.
///
/// Hand-written on purpose: `linkme`/`inventory` would add a dependency and
/// link-section magic for one feature (R3), and this list's completeness is
/// enforced by `census::every_declared_slot_is_in_the_roster`, which fails BY
/// ID when a new slot is not listed. The list is a data structure; the rule
/// is the guard.
///
/// 46 entries, not 47: `providers::route_handle::GLOBAL` is the census's one
/// first-caller-wins member and by ruling stays a raw `OnceLock` rather than
/// migrating onto [`CapabilitySlot`] — see this module's `census` submodule
/// doc ("Why form 2 is a rule and not an exemption") and
/// `census::route_handle_global_is_selected_by_the_first_caller_wins_arm_alone`
/// for why it genuinely belongs to the handle census while staying off this
/// list. Fitting it to [`CapabilitySlot::install`] would need either a
/// different `Outcome` shape or a changed call site — and the second one would
/// change which caller's config wins the initialisation. **Adjudicated
/// 2026-08-25: it stays raw**, and the full reasoning lives at the static
/// itself (`providers::route_handle::GLOBAL`), not only here, so a migrator who
/// reaches the code before the doc still finds the ruling.
/// `census::every_installed_global_is_a_capability_slot` names it as the one
/// deliberate exception rather than excluding it silently, so a second bare
/// handle appearing anywhere else still fails that guard.
///
/// Every entry below is a call through a `pub(crate) fn … -> &'static dyn
/// SlotStatus` accessor in the slot's own module (`const fn`, so this array
/// can be a `static` rather than a `LazyLock`), never a `pub` re-export of the
/// slot itself — see `spend::global_ledger_slot`'s doc for why: exposing 45
/// statics to answer one question would widen the crate's surface by 45 items,
/// and erasure at the accessor means a roster consumer gets exactly `id` /
/// `missing` / `outcome`, never the value.
pub static ALL_SLOTS: &[&'static dyn SlotStatus] = &[
    crate::metrics::metrics_runtime_slot(),
    crate::pii::engine::pii_engine_slot(),
    crate::tasks::cron::global_cron_slot(),
    crate::tools::result_store::global_tool_result_store_slot(),
    crate::tools::turn_budget::global_turn_result_budget_slot(),
    crate::tools::result_processing::result_budget_ceiling_slot(),
    crate::tools::in_flight::global_in_flight_tool_calls_slot(),
    crate::context::compact::manual::manual_wiring_slot(),
    crate::extension::manager_global::extension_manager_slot(),
    crate::memory::dreaming::dream_daemon_slot(),
    crate::identity::ledger::ledger_slot(),
    crate::identity::ledger::writer_slot(),
    crate::loop_graph::service::cron_trigger_slot(),
    crate::loop_graph::global_slot(),
    crate::loop_graph::event_bus_slot(),
    crate::config::load::effective_config_path_slot(),
    crate::config::defaults_override::defaults_override_slot(),
    crate::looping::global_slot(),
    crate::providers::route_observe::global_route_observability_slot(),
    crate::providers::session_model_handle::pin_sink_slot(),
    crate::providers::session_model_handle::pinnable_providers_slot(),
    crate::mcp::sampling_bridge::sampling_llm_slot(),
    crate::spend::global_ledger_slot(),
    crate::spend::global_policy_slot(),
    crate::search::handle::global_search_handle_slot(),
    crate::gateway::security::shared_token::global_shared_token_manager_slot(),
    crate::gateway::channel_policy::channel_config_snapshot_slot(),
    crate::gateway::shutdown_forensics::boot_instant_slot(),
    crate::gateway::resume_coordinator::global_resume_coordinator_slot(),
    crate::gateway::execution_engine::tool_service_builder::confirmation_requester_slot(),
    crate::gateway::execution_engine::tool_service_builder::config_approval_requester_slot(),
    crate::gateway::execution_engine::tool_service_builder::mcp_tool_registry_slot(),
    crate::gateway::execution_engine::concurrency_handle::concurrency_limiter_slot(),
    crate::gateway::codex_token_refresher::global_slot(),
    crate::gateway::i18n::installed_locale_slot(),
    crate::gateway::handlers::channel::telegram_tool_registry_slot(),
    crate::gateway::runtime_footer::global_footer_config_slot(),
    crate::goal::global_slot(),
    crate::thinker::memory_context_provider::session_end_mcp_slot(),
    crate::thinker::memory_context_provider::session_end_summarizer_slot(),
    crate::thinker::memory_context_provider::session_reflector_slot(),
    crate::thinker::memory_context_provider::session_end_compression_slot(),
    crate::thinker::memory_context_provider::open_loop_inject_slot(),
    crate::strategy::global_slot(),
    crate::session::store::global_session_event_store_slot(),
    crate::session::service::global_session_service_slot(),
];

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
        assert!(
            !S.install(2),
            "second install must be a no-op returning false"
        );
        assert_eq!(S.get(), Some(&1));
        // NOTE-3: a losing install disturbs neither half of the pair. The stamp
        // is written INSIDE `if fresh`, after the value lands, so "stamp present"
        // implies "value readable" — the property Task 12 trusts `Installed` for.
        assert!(matches!(S.outcome(), Some(Outcome::Installed)));
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
        // Fn-local, like every mutating test in this module: it keeps `UNSET`
        // down to a single reader and no reachable mutator, so
        // `an_untouched_slot_reports_no_outcome_at_all`'s "pristine" premise is
        // structural rather than a convention. A future test that installed a
        // shared `UNSET` would fail that one *nondeterministically* — libtest
        // runs these in parallel, so it would go red only when the mutator won
        // the race, and a flaky guard teaches people to re-run.
        static ERASED: CapabilitySlot<u32> =
            CapabilitySlot::new("test/erased", MissingSemantics::ConsumerDecides);
        let erased: &'static dyn SlotStatus = &ERASED;
        assert_eq!(erased.id(), "test/erased");
        assert!(matches!(
            erased.missing(),
            MissingSemantics::ConsumerDecides
        ));
    }

    /// ⚠️ KNOWN HAZARD, pinned deliberately — this test does not endorse the
    /// behaviour, it makes the day someone changes it a named red line.
    ///
    /// `outcome` is first-writer-wins, so a slot that declined and then
    /// installed holds a value while reporting `Declined { because }`. That is
    /// precisely the "confident lie … worse than today's silence" the module
    /// doc above argues this type exists to prevent — Task 12's diagnostics
    /// would tell an operator a live capability is missing.
    ///
    /// It is NOT hypothetical once migration lands, and the second writer is
    /// nowhere near the `else` arm its author is reading: e.g.
    /// `providers/moa/config_handle.rs::MOA_CONFIG` is written at boot
    /// (`orchestrator_init.rs`) *and* again from a live-apply path
    /// (`preset_store.rs`, after a `moa` preset patch). "`[moa]` absent at boot
    /// ⇒ decline; operator patches a preset ⇒ install" is decline-then-install
    /// in production.
    ///
    /// **Decide this in Task 14, not here.** Whether `install` should overwrite
    /// the stamp depends on what the migration finds; deciding with zero callers
    /// is speculative (R10). Task 14 must state, for each converted arm, whether
    /// that handle has any install site outside the `if`/`else` it is editing —
    /// if one does, the fix lands and this test goes red at a named line.
    #[test]
    fn decline_then_install_is_the_one_pair_this_type_cannot_describe() {
        static S: CapabilitySlot<u32> =
            CapabilitySlot::new("test/decline-then-install", MissingSemantics::FailsOpen);
        S.decline("input absent at boot");
        assert!(
            S.install(1),
            "install still succeeds -- the value write is unguarded"
        );
        assert_eq!(S.get(), Some(&1));
        assert!(
            matches!(S.outcome(), Some(Outcome::Declined { .. })),
            "today the stamp still says Declined about a working capability"
        );
    }

    #[test]
    fn update_before_install_returns_false_and_changes_nothing() {
        static M: MutableCapabilitySlot<u32> =
            MutableCapabilitySlot::new("test/mut-unset", MissingSemantics::FailsOpen);
        // This is spend::update_policy's EXISTING contract: the live-apply
        // verdict downgrades to Restart when no handle has been installed yet.
        assert!(
            !M.update(5),
            "update on an uninstalled slot must report false"
        );
        assert!(M.load().is_none());
        assert!(M.outcome().is_none());
    }

    #[test]
    fn install_then_update_swaps_the_value_and_keeps_the_stamp() {
        static M: MutableCapabilitySlot<u32> =
            MutableCapabilitySlot::new("test/mut-live", MissingSemantics::FailsOpen);
        assert!(M.install(1));
        assert_eq!(**M.load().expect("installed"), 1);
        assert!(M.update(2));
        assert_eq!(**M.load().expect("installed"), 2);
        assert!(matches!(M.outcome(), Some(Outcome::Installed)));
    }
}
