//! `core/capability-wiring` — did boot install the process-global capabilities?
//!
//! # Why this check is three-state
//!
//! `aleph-server doctor` builds a **fresh** `DiagnosticEngine` in a cold
//! process where no capability has been installed; `diagnostics.run` executes
//! inside the daemon where they are live. Same battery, two processes, two
//! truths. Reporting the cold process's empty roster as broken would cry wolf
//! on a healthy machine; reporting it as healthy would be the mistake this
//! whole round exists to remove ("unknown" must never read as "healthy").
//!
//! So the verdict keys on `shutdown_forensics::booted()`:
//!
//! | booted | roster | verdict |
//! |---|---|---|
//! | false | — | `Info`: this process did not boot; ask the daemon |
//! | true | complete | ok |
//! | true | holes | one finding per slot, severity from `MissingSemantics` |
//!
//! The third row is free extra value: `mark_boot()` runs at the *start* of
//! boot and the installs come after, so "booted but incomplete" is a real
//! failure state (boot died or early-returned) that nothing could observe
//! before.
//!
//! # Why the roster loop is a three-way match, not a two-way one
//!
//! `slot.outcome()` is `Option<&Outcome>`, and this check matches all three of
//! its shapes separately rather than collapsing to `!= Some(&Installed)`:
//! **declined** (boot reached the slot and chose not to install — there is a
//! reason, quoted verbatim, and the operator's move is to change the
//! condition) is a different investigation from **never reached** (nothing
//! ever got here — boot may have died or early-returned, and the operator's
//! move is to find out why that boot path did not run). A check that renders
//! both the same tells half its readers to go looking for a setting that is
//! working exactly as configured.

use async_trait::async_trait;

use crate::capability::{MissingSemantics, Outcome, ALL_SLOTS};
use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/capability-wiring";

/// Tag on the cold-process finding. `Severity::Info` alone is indistinguishable
/// downstream from `Finding::ok` — `Finding::is_problem()` is `severity >
/// Info`, so `report.ok()`, the `--json` lint surface, the CLI exit code, and
/// the human `[ok]` render all read this finding as a pass unless something
/// else carries the "I don't know" signal. `media_codecs::TAG_CODECS_UNKNOWN`
/// is the precedent: severity stays `Info` (crying wolf with `Warning`/`Error`
/// on a healthy cold process would be worse), and the tag is what lets
/// "unknown" stay distinguishable from "fine" for a consumer that checks for
/// it.
///
/// `pub(crate)`, not private: `gateway::shutdown_forensics`'s
/// `booted_is_false_before_mark_boot_and_true_after` test asserts this tag
/// (see that test's doc for why the assertion has to live there), and a
/// hand-copied string literal there would be the same drift risk F3
/// eliminated for the `concurrency-limiter` slot id — a rename here would
/// silently break the cross-module assertion instead of failing to compile.
pub(crate) const TAG_WIRING_UNKNOWN: &str = "capability-wiring-unknown";

/// Severity is derived from the failure direction, never hand-assigned per
/// slot — a hand-assigned table is a second source of truth about what a
/// missing handle costs.
fn severity_for(m: MissingSemantics) -> Severity {
    match m {
        // A gate that silently stopped gating.
        MissingSemantics::FailsOpen => Severity::Error,
        // The round-7 shape: a true sentence hiding a false world.
        MissingSemantics::IndistinguishableDefault { .. } => Severity::Warning,
        // N consumers each inventing an answer.
        MissingSemantics::ConsumerDecides => Severity::Warning,
        // Safe, but the feature is dead and says nothing.
        MissingSemantics::FailsClosed => Severity::Info,
    }
}

/// What a caller actually sees when the handle is absent — the sentence an
/// operator needs, not the enum's name.
///
/// Takes the slot's id, not just its [`MissingSemantics`], because
/// `FailsClosed`'s own doc promises "the feature is dead and says nothing" —
/// true for most members of that variant, but not for
/// `gateway/concurrency-limiter`: its `reconfigure_global() == false` already
/// reaches the operator honestly, downgrading the `execution` config
/// section's live-apply verdict from `Live` to `Restart` (see that slot's own
/// doc). Rendering it with the generic "inert, says nothing" sentence would
/// tell an operator who is already being told something that nothing was
/// said at all.
///
/// The general form this one exception stands for: severity may be derived
/// from `MissingSemantics` alone (see [`severity_for`]); the *sentence* may
/// not be, without first checking whether that particular handle already has
/// a voice elsewhere. Read a slot's own declaration before adding a second
/// exception here — do not extend this list without one.
///
/// The comparison is keyed off
/// `gateway::execution_engine::concurrency_handle::concurrency_limiter_slot().id()`,
/// not a second literal `"gateway/concurrency-limiter"` — same shape as
/// `providers::route_handle`'s exemption in `capability::census`. A literal
/// here would break the exception in production on a rename while a test
/// that compares the same literal to itself stayed green.
fn describe(slot_id: &str, missing: MissingSemantics) -> String {
    use crate::gateway::execution_engine::concurrency_handle::concurrency_limiter_slot;

    if slot_id == concurrency_limiter_slot().id() {
        return "a closed gate, but not a silent one: `self_config`'s live-apply \
                already downgrades the `execution` config section to `Restart` \
                when this handle is absent. What that downgrade cannot say is \
                WHICH cause fired — no engine has ever installed a limiter, or \
                one was installed and has since been dropped"
            .into();
    }
    match missing {
        MissingSemantics::IndistinguishableDefault { reads_as } => {
            format!("{reads_as} — indistinguishable from a deliberate configuration")
        }
        MissingSemantics::ConsumerDecides => {
            "`None`; each consumer decides for itself what that means".into()
        }
        MissingSemantics::FailsClosed => "a closed gate; the feature is inert".into(),
        MissingSemantics::FailsOpen => "an OPEN gate; this check is not being enforced".into(),
    }
}

#[derive(Default)]
pub struct CapabilityWiringCheck;

impl CapabilityWiringCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HealthCheck for CapabilityWiringCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Capability wiring"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        if !crate::gateway::shutdown_forensics::booted() {
            return vec![Finding::problem(
                ID,
                Severity::Info,
                "Wiring is not observable from this process",
                "This process did not run `aleph-server start`, so no capability handle \
                 was installed here. Reporting the empty roster either way would be \
                 fiction — the daemon is the only process that knows.",
            )
            .with_fix_hint(
                "Run `aleph doctor` (it asks the running gateway over `diagnostics.run`) \
                 rather than `aleph-server doctor`.",
            )
            .with_tag(TAG_WIRING_UNKNOWN)];
        }

        let mut findings: Vec<Finding> = Vec::new();
        for slot in ALL_SLOTS {
            match slot.outcome() {
                Some(Outcome::Installed) => {}
                Some(Outcome::Declined { because }) => findings.push(
                    Finding::problem(
                        ID,
                        severity_for(slot.missing()),
                        format!("Capability `{}` was declined", slot.id()),
                        format!(
                            "Boot reached this handle and could not install it: {because}. \
                             Reads observe: {}",
                            describe(slot.id(), slot.missing())
                        ),
                    )
                    .with_tag("capability-declined"),
                ),
                None => findings.push(
                    Finding::problem(
                        ID,
                        severity_for(slot.missing()),
                        format!("Capability `{}` was never reached", slot.id()),
                        format!(
                            "Boot started but nothing installed or declined this handle — \
                             boot may have failed or returned early. Reads observe: {}",
                            describe(slot.id(), slot.missing())
                        ),
                    )
                    .with_tag("capability-unreached"),
                ),
            }
        }

        if findings.is_empty() {
            vec![Finding::ok(
                ID,
                "Capability wiring complete",
                format!(
                    "All {} process-global capabilities were installed.",
                    ALL_SLOTS.len()
                ),
            )]
        } else {
            findings
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Severity is derived from the failure direction, never hand-assigned
    /// per slot — a hand-assigned table is a second source of truth about
    /// what a missing handle costs.
    #[test]
    fn severity_is_derived_from_the_failure_direction() {
        assert_eq!(severity_for(MissingSemantics::FailsOpen), Severity::Error);
        assert_eq!(
            severity_for(MissingSemantics::IndistinguishableDefault { reads_as: "x" }),
            Severity::Warning
        );
        assert_eq!(
            severity_for(MissingSemantics::ConsumerDecides),
            Severity::Warning
        );
        assert_eq!(severity_for(MissingSemantics::FailsClosed), Severity::Info);
    }

    /// A2: `gateway/concurrency-limiter` already has a voice elsewhere (the
    /// `execution` config section's honest downgrade to `Restart`), so it
    /// must not be rendered with the generic "inert, says nothing" sentence
    /// every other `FailsClosed` member gets.
    #[test]
    fn the_concurrency_limiter_exception_does_not_use_the_generic_fails_closed_sentence() {
        // Derived from the real accessor, not a second hand-copied literal:
        // a rename of the slot id must move both the production `if` and this
        // assertion together, not leave the test comparing a stale literal to
        // itself.
        let id =
            crate::gateway::execution_engine::concurrency_handle::concurrency_limiter_slot().id();
        let generic = describe("some/other-slot", MissingSemantics::FailsClosed);
        let exception = describe(id, MissingSemantics::FailsClosed);
        assert_ne!(generic, exception);
        assert!(exception.contains("Restart"));
        assert!(!exception.contains("says nothing"));
    }

    // The process-truth rule (cold process -> Info, tagged `TAG_WIRING_UNKNOWN`,
    // not a pass) is NOT tested here. `booted()` is backed by a process-global
    // `OnceLock` (`gateway::shutdown_forensics::BOOT_INSTANT`) that, once set by
    // ANY test in this lib binary, stays set for the rest of that process's
    // life — and that module's own doc declares its
    // `booted_is_false_before_mark_boot_and_true_after` test "THE ONLY TEST IN
    // THE LIB BINARY THAT MAY TOUCH `BOOT_INSTANT`", precisely because a second
    // reader of `booted()` is only correct when it wins an unspecified libtest
    // ordering race against that one. A prior version of this test carried an
    // `if booted() { skip }` guard for that race; measured across the full
    // suite the guard's `eprintln!` was swallowed by cargo's output capture on
    // a pass, so it deterministically skipped 8/8 runs and the assertions
    // below never executed. See
    // `gateway::shutdown_forensics::tests::booted_is_false_before_mark_boot_and_true_after`,
    // which now carries this check's cold-process assertion at its top, before
    // `mark_boot()` runs — guaranteed by program order within one test
    // function, not by test scheduling.
}
