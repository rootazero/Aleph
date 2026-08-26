//! `core/capability-wiring` — did boot install the process-global capabilities?
//!
//! # Why this check is three-state
//!
//! A capability roster is a fact about *one process*. `diagnostics.run` and
//! the `doctor` builtin execute inside the daemon, where the handles are live;
//! a `DiagnosticEngine` built anywhere else sees an empty roster that means
//! nothing. Reporting that empty roster as broken would cry wolf on a healthy
//! machine; reporting it as healthy would be the mistake this whole round
//! exists to remove ("unknown" must never read as "healthy").
//!
//! Registration answers this first: only
//! `DiagnosticEngine::with_capability_wiring_check()` attaches this check, and
//! only the two in-daemon callers invoke it, so the offline `aleph-server
//! doctor` no longer asks a question it structurally cannot answer. The
//! three-state verdict below is what remains for the paths that *do* run it.
//!
//! So the verdict keys on `shutdown_forensics::booted()`:
//!
//! | booted | roster | verdict |
//! |---|---|---|
//! | false | — | `Warning`, tagged: this process did not boot; ask the daemon |
//! | true | complete | ok |
//! | true | holes | one finding per slot, severity from `MissingSemantics` |
//!
//! The third row is free extra value: `mark_boot()` runs at the *start* of
//! boot and the installs come after, so "booted but incomplete" is a real
//! failure state (boot died or early-returned) that nothing could observe
//! before.
//!
//! # Why the cold row is `Warning`, not `Info` (fix round 2, F1 follow-up)
//!
//! The first fix round shipped `Info` + a tag
//! (`TAG_WIRING_UNKNOWN`), citing `media_codecs::TAG_CODECS_UNKNOWN` as
//! precedent. There is a second precedent in the same directory,
//! `idle_extensions`, which answers the identical "unknown must not read as
//! healthy" problem with `Warning` instead — and it is the more applicable
//! one here. The criterion that separates them: **does the rest of the
//! report become misleading if this line is read as fine?** For
//! `media_codecs`, no — a missing `gst-inspect-1.0` is an isolated,
//! environment-specific gap, uncommon, and unrelated to every other check in
//! the battery. For this check's cold branch, the answer measured against
//! the real render is yes, in a way `media_codecs` never has to face:
//!
//! - `render_human` (`DiagnosticReport::render_human`) only ever maps
//!   `Severity::Info` to the tag `"ok"` — identical to a check that actually
//!   ran and found nothing wrong — and it only prints a finding's `detail`
//!   text when `Finding::is_problem()` is true (`severity > Info`). At
//!   `Info`, this finding's title is the ONLY signal a human running
//!   `aleph-server doctor` without `--json` ever sees, sitting in a wall of
//!   30+ genuinely-healthy `[ok] core/sqlite-integrity ...` lines from a real
//!   report. `render_human` never prints `Finding::tags` at all, so
//!   `TAG_WIRING_UNKNOWN` — the *entire* fix from round 1 — is invisible
//!   outside `--json`. The "unknown must not read as healthy" rule, applied
//!   to the surface most people actually read, was still being broken.
//! - `report.ok()`, the `--json` lint surface, and `aleph-server doctor`'s
//!   exit code all read `Info` as "no unresolved problem", exactly the
//!   `report.ok() == true` / exit 0 / "no unresolved problems" outcome F1's
//!   review measured — the reason F1 was raised in the first place. The tag
//!   does not change any of these, because none of them look at tags
//!   (`is_problem()` is severity-only, by design — see `Finding::redacted`'s
//!   doc for the same "one field every consumer reads" argument stated for a
//!   different purpose).
//!
//! This is `idle_extensions`' hazard, not `media_codecs`': a cold
//! `aleph-server doctor` is not a rare misconfiguration, it is the entire
//! and permanent behaviour of one of this project's two doctor entry points
//! (`aleph-server doctor` vs `aleph doctor` against a live daemon), and on
//! that entry point the wiring question is unanswerable every single time,
//! not occasionally.
//!
//! # The consequence that paragraph used to accept, and why it is now gone
//!
//! `Warning` keeps this finding out of `report.ok()`, and for one round this
//! check was registered in `DiagnosticEngine::default_registry()` — the
//! offline registry `aleph-server doctor` builds. Since that command is by
//! definition a cold process, the cold branch fired on every invocation, so
//! `report.ok()` was always false and the command exited 1 on every machine,
//! forever. The argument accepted at the time was that the exit code was
//! already unreliable, because `core/duplicate-instance` and
//! `core/config-parse` can also flip it. That argument does not survive the
//! distinction it elides: those two flip on a *condition* an operator can
//! clear, while this one flipped *unconditionally*. An always-true predicate
//! is not a weaker gate, it is not a gate — and the exit code's documented
//! purpose (`bin/aleph-server/commands/doctor.rs`: "so CI can gate on it")
//! means a CI consumer could never have been added.
//!
//! The fix was not to lower the severity back to `Info` — that reopens the
//! invisibility defect this section exists to explain. It was to stop
//! registering, on an offline path, a check whose own `fix_hint` reads *"Run
//! `aleph doctor` ... rather than `aleph-server doctor`"*. The check is now
//! attached by `DiagnosticEngine::with_capability_wiring_check()`, which only
//! the two in-daemon callers invoke; see that builder's doc.
//!
//! The cold branch below therefore stays as **defence in depth**, not because
//! it is on the default path any more: that builder is reachable in principle
//! from a process that never booted, and `gateway::shutdown_forensics`'s
//! `booted_is_false_before_mark_boot_and_true_after` asserts it. Do not delete
//! it as unreachable.
//!
//! The tag stays regardless of this severity change — see
//! [`TAG_WIRING_UNKNOWN`]'s doc — because `Warning` is shared with
//! `core/config-parse`, `core/duplicate-instance`, and this check's own
//! `IndistinguishableDefault`/`ConsumerDecides` booted-branch holes; severity
//! alone still cannot tell a consumer "the check could not run at all" from
//! "the check ran and found a real gap".
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

/// Tag on the cold-process finding. Severity (`Warning`, see the module doc's
/// "Why the cold row is `Warning`, not `Info`" section) already keeps this
/// finding out of `report.ok()`; the tag is still needed because `Warning`
/// alone does not say WHICH problem this is — `core/config-parse` and
/// `core/duplicate-instance` are also `Warning`, for unrelated reasons, and a
/// consumer that wants to react specifically to "the wiring question could
/// not be answered" (as opposed to "a real gap was found") needs a signal
/// severity cannot carry.
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
                // See the module doc's "Why the cold row is `Warning`, not
                // `Info`" section: `Info` renders identically to a genuine
                // pass in both the human render (`[ok]`, detail suppressed)
                // and every machine consumer (`report.ok()`, `--json`,
                // the CLI exit code) — none of which look at `Finding::tags`.
                Severity::Warning,
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

    // The process-truth rule (cold process -> Warning, tagged
    // `TAG_WIRING_UNKNOWN`, not a pass) is NOT tested here. `booted()` is
    // backed by a process-global `OnceLock`
    // (`gateway::shutdown_forensics::BOOT_INSTANT`) that, once set by ANY
    // test in this lib binary, stays set for the rest of that process's
    // life — and that module's own doc declares its
    // `booted_is_false_before_mark_boot_and_true_after` test "THE ONLY TEST
    // IN THE LIB BINARY THAT MAY TOUCH `BOOT_INSTANT`", precisely because a
    // second reader of `booted()` is only correct when it wins an
    // unspecified libtest ordering race against that one.
    //
    // A prior version of this test carried an `if booted() { skip }` guard
    // for that race. What actually got measured, TWICE, with different
    // results — corrected here rather than restated, because the first
    // number did not reproduce under the invocation it was attributed to:
    // isolated to just the two relevant tests (a filtered run, effectively
    // racing two tests against each other in a small pool), the guard hit
    // SKIP 8/8 times, with the real assertions never executing. Under the
    // actual CI-shaped invocation (`cargo test -p alephcore --lib`, the full
    // suite, unfiltered, default thread count), it hit SKIP 0/2 times — both
    // runs executed the real assertions. (n=2, not padded further; treat it
    // as "did not reproduce in two tries" rather than a settled rate.) So
    // the defect this fix removes was not "the test never runs" — it is
    // invocation-dependent, and by this repo's own criteria that is the
    // worse shape: a guard that passes for real under CI and silently skips
    // under a narrower, filtered run teaches a reader that the assertion
    // ran when it may not have.
    //
    // The fix's mechanism does not depend on which of those numbers is
    // "the" rate — see
    // `gateway::shutdown_forensics::tests::booted_is_false_before_mark_boot_and_true_after`,
    // which now carries this check's cold-process assertion at its top,
    // before `mark_boot()` runs. That removes the race by program order
    // within one test function, not by winning or losing test scheduling
    // under any particular invocation.
}
