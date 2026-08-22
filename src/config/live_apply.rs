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

use super::reload_impact::{
    dotted_prefix_matches, live_target_for, ReloadImpact, LIVE_SECTIONS, LIVE_SUBSECTIONS,
};
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

    for target in LIVE_SECTIONS.iter().chain(LIVE_SUBSECTIONS.iter()) {
        // A target is requested by its own exact name — how the whole-config
        // callers pass it, via `reload_impact::live_targets()` — or by a
        // coarser ancestor: the single-patch caller (`patcher.rs`) only ever
        // knows the *top-level* section of the path it patched, so a write
        // to "policies.spend.per_user_usd" reaches this function as
        // `top_sections = ["policies"]`. `dotted_prefix_matches` is the one
        // prefix-matching primitive this module's sibling declares; see its
        // doc for why this call uses the arguments in the opposite order
        // from `live_target_for` below — this asks "does any requested name
        // cover this declared target", not "which declared target covers
        // this specific path".
        let requested = top_sections
            .iter()
            .any(|&s| dotted_prefix_matches(s, target));
        if !requested {
            continue;
        }
        let landed = match *target {
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
                // W27 — the sub-agent fan-out cap is a process-global atomic
                // read by `SubagentTool::new`, so it always lands (there is no
                // handle that can be missing). It is applied first and its
                // success is deliberately NOT folded into `landed`: the
                // section's honest-downgrade verdict tracks the run-admission
                // caps, which are the part that needs a live engine. Reporting
                // `Live` because a knob that cannot fail succeeded would be the
                // mirror of the bug this module exists to fix.
                crate::agents::subagent_tool::set_max_concurrent_subagents(
                    cfg.execution.max_concurrent_subagents,
                );
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
            // `[policies.spend]`'s handle is a hot-swappable `ArcSwap`
            // seeded at boot by `spend::install_policy`; storing the newly
            // patched policy into it is what makes a ceiling change live —
            // `spend::check`'s very next call (floor arm or admission arm)
            // reads it fresh via `spend::current_policy`. A missing handle
            // (a CLI process, most tests, or any process before boot installs
            // it) makes `spend::update_policy` return `false`, so the
            // verdict downgrades to `Restart` honestly instead of claiming a
            // store that did not happen — the same reasoning as `route`'s
            // arm above.
            //
            // Note this arm can fire on a patch to a *sibling* of
            // `policies.spend` (e.g. `policies.tool_permissions.foo`, which
            // arrives here as `top_sections = ["policies"]` too): it then
            // re-stores `cfg.policies.spend` unchanged, which is a harmless
            // idempotent no-op — `cfg` already carries spend's current
            // value either way. No false `Live` escapes from this: the
            // verdict for that sibling path is decided by `classify`, which
            // returns `Restart` before `classify_verified` ever consults
            // this function's return value (see `reload_impact::classify`'s
            // doc on the bare "policies" path).
            "policies.spend" => crate::spend::update_policy(cfg.policies.spend.clone()),
            // Unreachable while the guard test below passes: a new entry in
            // LIVE_SECTIONS/LIVE_SUBSECTIONS without an arm here fails at
            // compile-review time via that test, not silently at runtime.
            _ => false,
        };
        if landed {
            applied.push(*target);
        } else {
            tracing::debug!(
                section = *target,
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
/// The match is target-exact (section OR subsection, via
/// [`live_target_for`]) rather than "did anything apply at all". Today a
/// patch carries one section so the two agree, but a predicate that answers a
/// question adjacent to the one asked is how the next multi-section caller
/// gets a `Live` verdict for a section that never landed.
#[must_use]
pub fn classify_verified(config_path: &str, live_applied: &[&'static str]) -> ReloadImpact {
    let impact = ReloadImpact::classify(config_path);
    if impact != ReloadImpact::Live {
        return impact;
    }
    match live_target_for(config_path) {
        Some(target) if live_applied.contains(&target) => ReloadImpact::Live,
        _ => ReloadImpact::Restart,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard that keeps the declaration and the action from drifting.
    ///
    /// A section (or subsection) added to `LIVE_SECTIONS`/`LIVE_SUBSECTIONS`
    /// without an arm here would advertise "no restart needed" on every
    /// write surface and do nothing — the exact failure this module was
    /// created to close, reintroduced one const entry at a time.
    #[test]
    fn every_live_section_has_an_apply_arm() {
        let known_arms = ["route", "execution", "behavior", "policies.spend"];
        for target in LIVE_SECTIONS.iter().chain(LIVE_SUBSECTIONS.iter()) {
            assert!(
                known_arms.contains(target),
                "'{target}' is declared live in ReloadImpact but apply_live_sections has no \
                 arm for it — the Live claim would be unbacked"
            );
        }
        for arm in known_arms {
            assert!(
                LIVE_SECTIONS.contains(&arm) || LIVE_SUBSECTIONS.contains(&arm),
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

    /// W27 — the `execution` arm must actually push the sub-agent cap into the
    /// enforcement point, not merely be declared live.
    ///
    /// Asserted by reading the value back out of `agents::subagent_tool`, which
    /// is what `SubagentTool::new` consults: dropping the `set_…` call from the
    /// arm leaves `every_live_section_has_an_apply_arm` and every clamp test
    /// green while a patched `config.toml` changes nothing.
    #[test]
    #[serial_test::serial(subagent_concurrency_cap)]
    fn the_execution_arm_installs_the_subagent_concurrency_cap() {
        use crate::agents::subagent_tool::{
            max_concurrent_subagents, set_max_concurrent_subagents,
        };

        let restore = max_concurrent_subagents();
        let mut cfg = Config::default();
        cfg.execution.max_concurrent_subagents = 11;
        let _ = apply_live_sections(&cfg, &["execution"]);
        let observed = max_concurrent_subagents();
        set_max_concurrent_subagents(restore);
        assert_eq!(
            observed, 11,
            "[execution] max_concurrent_subagents must reach the fan-out semaphore's source"
        );
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
        // Restart verdicts are untouched by what did or did not apply.
        assert_eq!(
            classify_verified("providers.openai", &["route"]),
            ReloadImpact::Restart
        );
    }

    /// The single-patch caller (`patcher.rs`) only ever knows the *top-level*
    /// segment of the path it patched — a write to
    /// `"policies.spend.per_user_usd"` reaches `apply_live_sections` as
    /// `top_sections = ["policies"]`, never `["policies.spend"]`. Without
    /// `dotted_prefix_matches`'s ancestor check this arm would never fire
    /// from that path — exactly the mismatch the controller ruling called
    /// out.
    /// ⚠️ Serialised with every other test that WRITES `GLOBAL_POLICY`.
    /// `apply_live_sections`'s spend arm calls `spend::update_policy`, which
    /// overwrites a process-wide `ArcSwap` shared by the whole `--lib` test
    /// binary — so two such tests running concurrently are two writers to one
    /// cell, and the loser reads the winner's value. The precedent is
    /// `the_execution_arm_installs_the_subagent_concurrency_cap` in this same
    /// file, which takes the same annotation for the same shape.
    #[test]
    #[serial_test::serial(spend_global_policy)]
    fn policies_spend_arm_fires_from_the_coarse_top_level_name() {
        crate::spend::install_policy(crate::config::types::policies::SpendPolicy::default());
        let cfg = Config::default();
        assert_eq!(
            apply_live_sections(&cfg, &["policies"]),
            vec!["policies.spend"]
        );
    }

    /// A sibling subsection under the same top-level parent (e.g. a patch to
    /// `policies.tool_permissions.foo`) also arrives as
    /// `top_sections = ["policies"]` and therefore also runs this arm — a
    /// harmless idempotent re-store of `cfg.policies.spend`'s current value.
    /// No false `Live` escapes from that: `classify` (not `classify_verified`)
    /// decides the sibling path's verdict, and returns `Restart` before
    /// `classify_verified` ever looks at what this function applied.
    #[test]
    fn a_sibling_policies_subsection_does_not_earn_a_live_verdict() {
        assert_eq!(
            classify_verified("policies.tool_permissions.foo", &["policies.spend"]),
            ReloadImpact::Restart
        );
    }

    /// G14 — `[policies.spend]`'s honest live-apply.
    ///
    /// Positive: with the process-wide handle installed, applying a patched
    /// `policies.spend.per_user_usd` reports `Live`, and `spend::check`'s
    /// very next call — no restart, no new process — sees the new ceiling.
    ///
    /// Negative: when the section did not land, the verdict must downgrade
    /// to `Restart` rather than lie. `GLOBAL_POLICY` is a `OnceLock`
    /// (`spend::update_policy`'s handle) that this binary's other tests may
    /// already have set by the time this test runs, in an order this crate
    /// does not control, and a `OnceLock` cannot be uninstalled — see
    /// `spend::update_policy_into`'s doc. So this exercises the downgrade
    /// decision the same way `route_without_a_registered_chain_downgrades_to_restart`
    /// does for `route`: directly, against a synthetic empty `live_applied`,
    /// which is exactly what `apply_live_sections` produces when the arm's
    /// `update_policy` call returns `false` — pinned in isolation,
    /// independent of process-global ordering, by
    /// `spend::tests::update_policy_into_reports_false_with_no_handle`.
    /// ⚠️ Serialised with every other test that WRITES `GLOBAL_POLICY`.
    /// `apply_live_sections`'s spend arm calls `spend::update_policy`, which
    /// overwrites a process-wide `ArcSwap` shared by the whole `--lib` test
    /// binary — so two such tests running concurrently are two writers to one
    /// cell, and the loser reads the winner's value. The precedent is
    /// `the_execution_arm_installs_the_subagent_concurrency_cap` in this same
    /// file, which takes the same annotation for the same shape.
    #[test]
    #[serial_test::serial(spend_global_policy)]
    fn g14_spend_arm_applies_live_and_reaches_check_with_no_restart() {
        // A handle merely needs to exist for `update_policy` (which the arm
        // calls) to succeed — idempotent, so this is safe even if another
        // test in this binary already installed one; `update_policy` always
        // overwrites regardless of who installed it first.
        crate::spend::install_policy(crate::config::types::policies::SpendPolicy::default());

        let principal = crate::spend::Principal::User("u-g14-live-apply-spend-arm".to_string());
        let now_ms = chrono::Utc::now().timestamp_millis();
        let period_start_ms = crate::spend::period::period_start_ms(
            now_ms,
            crate::config::types::policies::SpendPeriod::Month,
        );
        // A brand-new principal key starts at zero spend in the shared
        // ledger, so recording exactly $5 gives a figure this test fully
        // controls, unaffected by any other test sharing the same
        // process-wide ledger.
        crate::spend::global_ledger()
            .record(&principal, period_start_ms, crate::spend::Delta::Usd(5.0))
            .expect("record");

        let mut cfg = Config::default();
        cfg.policies.spend.per_user_usd = Some(10.0);

        // The single-patch caller's exact shape: `top_sections` carries only
        // the coarse top-level segment, never "policies.spend" itself.
        let applied = apply_live_sections(&cfg, &["policies"]);
        assert_eq!(applied, vec!["policies.spend"]);
        assert_eq!(
            classify_verified("policies.spend.per_user_usd", &applied),
            ReloadImpact::Live
        );
        // $5 spent against a $10 ceiling: allowed.
        assert!(matches!(
            crate::spend::check(&principal, now_ms),
            crate::spend::Verdict::Allowed(_)
        ));

        // Lower the ceiling below what is already spent — live, with no
        // restart and no new process. `check`'s very next call must see it.
        cfg.policies.spend.per_user_usd = Some(1.0);
        let applied = apply_live_sections(&cfg, &["policies"]);
        assert_eq!(applied, vec!["policies.spend"]);
        assert!(matches!(
            crate::spend::check(&principal, now_ms),
            crate::spend::Verdict::Denied { .. }
        ));

        // Negative: a section that did not land must downgrade honestly.
        // See this test's doc for why the process-wide handle cannot be
        // forced absent here.
        assert_eq!(
            classify_verified("policies.spend.per_user_usd", &[]),
            ReloadImpact::Restart
        );
    }
}
