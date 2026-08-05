//! `live_apply` — push a persisted config change onto the running runtime.
//!
//! [`ReloadImpact`](super::ReloadImpact) *claims* which sections take effect
//! without a restart. This module is what makes the claim true. Before it
//! existed the two lived apart: the claim was attached to every write surface
//! (the `self_config` tool response, the `config.patch` RPC response), while
//! the hot-apply was hand-inlined in exactly one of them. A Panel user who
//! patched `route.mode` over `config.patch` was told "takes effect on the next
//! prompt, no restart needed" and got a running failover chain that had never
//! heard of the change; `execution` was worse still — its hot-apply was
//! missing from `rollback` too, so undoing a concurrency change left the old
//! caps installed under a response saying otherwise.
//!
//! Two rules follow, and both are enforced by tests here:
//!
//! 1. **The table that says "live" and the code that makes it live are the
//!    same table.** [`ReloadImpact::LIVE_SECTIONS`] drives the match below,
//!    and [`tests::every_live_section_has_an_apply_arm`] fails if a section is
//!    declared live with nothing to apply it (or vice versa).
//! 2. **A claim is downgraded when the action did not happen.** The live
//!    handles are process-global `OnceLock`s registered at boot; in a CLI
//!    process, a test, or before the failover chain is assembled they are
//!    absent, and "hot-applied" would be a lie. [`apply_live_sections`]
//!    returns what actually landed so the caller can classify honestly
//!    ([`classify_verified`]).
//!
//! Layering note: `config` reaching into `providers` / `gateway` inverts the
//! usual direction. It is deliberate and narrow — both targets are
//! process-global handles that exist *precisely* to be poked from wherever a
//! config write lands, and both are no-ops when unregistered. The alternative
//! (every write surface re-implementing the poke) is the arrangement that
//! produced the bug.

use super::reload_impact::{ReloadImpact, LIVE_SECTIONS};
use super::Config;

/// Hot-apply the sections of `cfg` named by `top_sections` onto the running
/// runtime, returning the sections that actually landed.
///
/// `top_sections` are top-level `config.toml` section names (the
/// `applied_sections` of a patch, or every live section after a whole-file
/// rollback). Unknown / non-live names are ignored — this function never
/// decides *whether* a section is live, it only executes the table.
///
/// A declared-live section whose runtime handle is absent is logged and
/// omitted from the return value; it is NOT reported as a separate list,
/// because "declared live minus applied" already says it and a field nobody
/// reads is a field that will eventually be wrong.
pub fn apply_live_sections(cfg: &Config, top_sections: &[&str]) -> Vec<&'static str> {
    let mut applied = Vec::new();

    for section in LIVE_SECTIONS {
        if !top_sections.contains(section) {
            continue;
        }
        let landed = match *section {
            // The failover chain reads its route state from an `ArcSwap`
            // installed at boot; storing here is what a "live" route change
            // means.
            "route" => match crate::providers::route_handle::try_global_route_handle() {
                Some(handle) => {
                    handle.store(&cfg.route);
                    true
                }
                None => false,
            },
            // New admission caps bind on the next run admission.
            "execution" => {
                crate::gateway::execution_engine::concurrency_handle::reconfigure_global(
                    cfg.execution.max_runs_global,
                    cfg.execution.max_runs_per_agent,
                )
            }
            // `behavior` needs no poke: every run path re-reads `output_mode`
            // fresh from the shared `Config` that the patcher already swapped
            // (`handlers::agent::resolved_output_mode` and the inbound-router
            // executor). Its liveness is a property of the *reader*, not of a
            // handle — so it is live unconditionally, and saying otherwise
            // would be the mirror of the bug this module fixes.
            "behavior" => true,
            // Unreachable while the guard test below passes: a new entry in
            // LIVE_SECTIONS without an arm here fails at compile-review time
            // via that test, not silently at runtime.
            _ => false,
        };
        if landed {
            applied.push(*section);
        } else {
            tracing::debug!(
                section,
                "config section is declared live but its runtime handle is not registered; \
                 the change is persisted and needs a restart"
            );
        }
    }

    applied
}

/// Classify `config_path`, downgrading a `Live` verdict to `Restart` when the
/// hot-apply **for that section** did not actually happen.
///
/// This is the honest version of [`ReloadImpact::classify`] for callers that
/// have just performed a write: `classify` answers "is this section *the kind
/// of thing* that applies live", which is all a dry-run preview can know;
/// after a real write we also know whether the runtime was there to receive
/// it, and reporting `Live` when it was not is precisely the silent failure
/// the conservative default was chosen to avoid.
///
/// The match is section-exact rather than "did anything apply at all". Today a
/// patch carries one section so the two agree, but a predicate that answers a
/// question adjacent to the one asked is how the next multi-section caller
/// gets a `Live` verdict for a section that never landed.
#[must_use]
pub fn classify_verified(config_path: &str, live_applied: &[&'static str]) -> ReloadImpact {
    let impact = ReloadImpact::classify(config_path);
    if impact != ReloadImpact::Live {
        return impact;
    }
    let top = config_path.split('.').next().unwrap_or(config_path).trim();
    if live_applied.contains(&top) {
        ReloadImpact::Live
    } else {
        ReloadImpact::Restart
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard that keeps the declaration and the action from drifting.
    ///
    /// A section added to `LIVE_SECTIONS` without an arm here would advertise
    /// "no restart needed" on every write surface and do nothing — the exact
    /// failure this module was created to close, reintroduced one const entry
    /// at a time.
    #[test]
    fn every_live_section_has_an_apply_arm() {
        let known_arms = ["route", "execution", "behavior"];
        for section in LIVE_SECTIONS {
            assert!(
                known_arms.contains(section),
                "'{section}' is declared live in ReloadImpact but apply_live_sections has no \
                 arm for it — the Live claim would be unbacked"
            );
        }
        for arm in known_arms {
            assert!(
                LIVE_SECTIONS.contains(&arm),
                "apply_live_sections handles '{arm}' but ReloadImpact does not call it live — \
                 the runtime would be poked while the response says 'restart required'"
            );
        }
    }

    #[test]
    fn non_live_sections_are_ignored() {
        let cfg = Config::default();
        assert!(apply_live_sections(&cfg, &["providers", "memory"]).is_empty());
    }

    #[test]
    fn behavior_is_live_without_any_registered_handle() {
        // `behavior` liveness comes from readers re-reading the shared config,
        // which the patcher already swapped — no boot-time handle involved.
        let cfg = Config::default();
        assert_eq!(apply_live_sections(&cfg, &["behavior"]), vec!["behavior"]);
    }

    #[test]
    fn route_without_a_registered_chain_downgrades_to_restart() {
        // In a process where the failover chain was never assembled (CLI,
        // tests, early boot) nothing received the change, so claiming Live
        // would be a lie the user only discovers by the change not happening.
        assert_eq!(classify_verified("route.mode", &[]), ReloadImpact::Restart);
    }

    #[test]
    fn a_landed_live_apply_keeps_the_live_verdict() {
        assert_eq!(
            classify_verified("route.mode", &["route"]),
            ReloadImpact::Live
        );
    }

    #[test]
    fn classify_verified_matches_the_section_not_merely_any_success() {
        // A sibling section landing must NOT vouch for this one. The two agree
        // today (a patch carries one section), so only this assertion keeps the
        // predicate answering the question that was asked.
        assert_eq!(
            classify_verified("route.mode", &["execution"]),
            ReloadImpact::Restart
        );
        assert_eq!(
            classify_verified("execution.max_runs_global", &["execution"]),
            ReloadImpact::Live
        );
    }

    #[test]
    fn classify_verified_never_upgrades_a_non_live_section() {
        // Restart / Inert verdicts are untouched by what did or did not apply.
        assert_eq!(
            classify_verified("providers.openai", &["route"]),
            ReloadImpact::Restart
        );
        assert_eq!(
            classify_verified("task_routing", &["route"]),
            ReloadImpact::Inert
        );
    }
}
