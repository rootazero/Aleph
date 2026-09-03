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
            "route" => {
                // The `config_problems` half of the same hot write. The
                // observability bundle's list is computed at boot, so without
                // this the one field that answers "why did my routing config
                // do nothing" describes the *previous* generation — a typo'd
                // pin written at runtime would never appear in it. Poked
                // unconditionally, like `execution`'s sub-agent cap and for
                // the same reason: its success is deliberately NOT folded
                // into `landed`, which tracks the route HANDLE (the part that
                // needs a live chain). A process with no observability bundle
                // has nothing to republish; that is not a failed route apply.
                if let Some(obs) = crate::providers::route_observe::global_route_observability() {
                    obs.hot_apply_problems(&cfg.route);
                }
                match crate::providers::route_handle::try_global_route_handle() {
                    Some(handle) => {
                        handle.store(&cfg.route);
                        true
                    }
                    None => false,
                }
            }
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
            // `[policies.terminal]`'s handle is the process-global PTY
            // manager singleton (`crate::gateway::pty::manager()`) — always
            // present, unlike `route`/`spend`'s boot-installed `ArcSwap`s,
            // so this arm always lands. Turning the switch off must also
            // kill live sessions: a gate evaluated only at admission would
            // leave a shell that is already open still open.
            "policies.terminal" => {
                if !cfg.policies.terminal.enabled {
                    let killed = crate::gateway::pty::manager().close_all();
                    if killed > 0 {
                        tracing::warn!(killed, "terminal disabled; live PTY sessions terminated");
                    }
                }
                true
            }
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
        let known_arms = [
            "route",
            "execution",
            "behavior",
            "policies.spend",
            "policies.terminal",
        ];
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
    ///
    /// `"policies.terminal"` also fires from this same coarse name — it is
    /// the other member of `LIVE_SUBSECTIONS` and its handle (the
    /// process-global PTY manager) is always present, so it always lands.
    /// That is not a false positive: the sibling-does-not-earn-a-verdict
    /// guarantee comes from `classify`/`classify_verified` matching the
    /// *specific* target, not from this function only ever applying one
    /// thing at a time — see `a_sibling_policies_subsection_does_not_earn_a_live_verdict`.
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
            vec!["policies.spend", "policies.terminal"]
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
    /// to `Restart` rather than lie. `GLOBAL_POLICY` is a
    /// `MutableCapabilitySlot` (`spend::update_policy`'s handle) that this
    /// binary's other tests may already have installed by the time this test
    /// runs, in an order this crate does not control, and a slot cannot be
    /// uninstalled — see `spend::update_policy`'s doc. So this exercises the
    /// downgrade decision the same way
    /// `route_without_a_registered_chain_downgrades_to_restart`
    /// does for `route`: directly, against a synthetic empty `live_applied`,
    /// which is exactly what `apply_live_sections` produces when the arm's
    /// `update_policy` call returns `false` — pinned in isolation,
    /// independent of process-global ordering, by
    /// `capability::tests::update_before_install_returns_false_and_changes_nothing`.
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
        // `"policies.terminal"` rides along too — same coarse name, its own
        // always-present handle (see `policies_spend_arm_fires_from_the_coarse_top_level_name`'s doc).
        let applied = apply_live_sections(&cfg, &["policies"]);
        assert_eq!(applied, vec!["policies.spend", "policies.terminal"]);
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
        assert_eq!(applied, vec!["policies.spend", "policies.terminal"]);
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

    /// The `route` arm republishes `config_problems`, not just the handle.
    ///
    /// This executor is reached by the `update_config` tool / `config.patch`
    /// RPC, `ConfigPatcher::rollback` and `config.reload`. `config_problems`
    /// — the one field that answers "why did my routing config do nothing" —
    /// must therefore be recomputed here and not only in the dedicated
    /// `route_config.update` handler, or a pin patched at runtime is judged
    /// against the previous generation.
    ///
    /// Asserted through the **process-global** bundle `route_status` reads,
    /// because that is what the arm looks up: a local `RouteObservability`
    /// would prove the function works, not that the executor reaches it. Both
    /// directions of the gate are exercised (a broken config raises a problem,
    /// a clean one drains it) — a republish that only ever appends would leave
    /// a fixed config permanently accused.
    ///
    /// ⚠️ The slot is a `CapabilitySlot` (install-once, no uninstall), so this
    /// bundle outlives the test in the `--lib` binary. The serial key is shared
    /// with `route_config`'s handler-face test, which pokes the same bundle
    /// through `apply_live_sections` and would otherwise race these two
    /// assertions.
    #[tokio::test]
    #[serial_test::serial(route_observability_global)]
    async fn a_route_patch_through_the_executor_republishes_config_problems() {
        use crate::config::types::route::{ModelRouteConfig, RouteMode};
        use crate::providers::default_handle::StaticDefault;
        use crate::providers::mock::MockProvider;
        use crate::providers::route_observe::{
            global_route_observability, set_global_route_observability, test_observability,
        };
        use crate::providers::route_policy::EndpointTier;
        use crate::sync_primitives::Arc;

        set_global_route_observability(test_observability(
            Arc::new(StaticDefault::new(Arc::new(MockProvider::new("ok")))),
            std::collections::HashMap::from([("ollama".to_string(), EndpointTier::Local)]),
        ));
        let obs = global_route_observability()
            .expect("a bundle is installed (this test's, or an earlier installer's)");

        // Installed (get-or-init) so the arm's verdict is decided, not left to
        // whether some other test in this binary happened to init it first:
        // `landed` tracks the HANDLE, so without this the returned vec would be
        // empty here for reasons that have nothing to do with `config_problems`.
        let handle =
            crate::providers::route_handle::global_route_handle(&ModelRouteConfig::default());

        let mut cfg = Config::default();
        // A pin naming a provider that is not configured: a problem against
        // ANY tier catalog, so this holds even if another test in this binary
        // won the install-once race with a different bundle.
        cfg.route.local_provider = Some("olama".to_string());
        cfg.route.mode = RouteMode::AlwaysLocal;
        let applied = apply_live_sections(&cfg, &["route"]);
        assert!(
            applied.contains(&"route"),
            "the route target must report as applied once its handle exists; got {applied:?}"
        );
        assert_eq!(
            handle.snapshot().mode,
            RouteMode::AlwaysLocal,
            "`applied` naming `route` must mean the handle carries the new config"
        );

        let problems = obs.snapshot().await["config_problems"].clone();
        let problems = problems.as_array().expect("array").clone();
        assert_eq!(
            problems.len(),
            1,
            "a [route] write through the executor must republish config_problems; got: \
             {problems:?}"
        );
        assert_eq!(problems[0]["field"], "local_provider");
        assert!(problems[0]["detail"]
            .as_str()
            .expect("detail")
            .contains("olama"));

        // The other direction: fixing the config must drain the list, not
        // leave the old accusation standing.
        cfg.route.local_provider = None;
        let _ = apply_live_sections(&cfg, &["route"]);
        assert!(
            obs.snapshot().await["config_problems"]
                .as_array()
                .expect("array")
                .is_empty(),
            "a clean [route] write must clear the previous generation's problems"
        );
    }

    /// Every dedicated `*_config.update` handler that persists a wholly-live
    /// section must also run the declaration table.
    ///
    /// `every_live_section_has_an_apply_arm` above proves the *arm* exists. It
    /// cannot prove that every write surface reaches it, and the dedicated
    /// handlers under `gateway/handlers/` are all outside `ConfigPatcher` — the
    /// chokepoint `reload_impact::LIVE_SECTIONS`'s doc assumes is the only
    /// caller. `execution_config::handle_update` sat in that gap: it wrote
    /// `max_runs_global` to disk, answered `{"success": true}`, and the running
    /// `ConcurrencyLimiter` kept admitting at the boot-time cap for the rest of
    /// the process, while the *same* change made through `config.patch` applied
    /// instantly and reported `Live`.
    ///
    /// The section list is read from `LIVE_SECTIONS`, and the handler set is
    /// read off the filesystem, so a fourth live section — or a fourth
    /// dedicated handler — is covered without editing this test. That is the
    /// whole point: an enumerated version of this test would have been written
    /// listing the handlers that existed on the day it was written, which is
    /// the same shape as the bug.
    ///
    /// # The exemption falsifies itself
    ///
    /// One handler legitimately does not call the executor today, and the
    /// exemption below asserts the *reason* still holds rather than the name.
    /// A list that only named files would rot into a licence: the day someone
    /// gives `behavior` a real runtime handle, the exemption must go — and it
    /// goes red instead of quietly vouching for a handler that has become
    /// non-compliant.
    #[test]
    fn every_dedicated_config_handler_that_saves_a_live_section_calls_apply_live_sections() {
        // `strip_comment_lines`, NOT `code_text`: the thing being searched for
        // IS a string literal (`save_incremental(&["execution"])`), and
        // `code_text` deletes literal payloads by design. Comments are still
        // stripped so a doc comment naming the call cannot vouch for a handler
        // that does not make it, and `production_prefix` drops each file's own
        // test module for the same reason.
        use crate::utils::source_scan::{production_prefix, strip_comment_lines};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let handlers = root.join("src/gateway/handlers");

        // Read this module's own production half so the exemption checks below
        // cannot be satisfied by a string sitting in this very test.
        let live_apply_src = strip_comment_lines(&production_prefix(
            &std::fs::read_to_string(root.join("src/config/live_apply.rs")).expect("live_apply.rs"),
        ));

        let mut files: Vec<std::path::PathBuf> = Vec::new();
        let mut stack = vec![handlers.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("handlers dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    files.push(path);
                }
            }
        }
        assert!(
            files.len() > 20,
            "only {} handler files found — the walk stopped matching, so this \
             test's green would mean nothing",
            files.len()
        );

        let mut checked = 0usize;
        let mut missing: Vec<String> = Vec::new();
        for path in &files {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let src = strip_comment_lines(&production_prefix(
                &std::fs::read_to_string(path).expect("handler source"),
            ));

            for section in LIVE_SECTIONS {
                let save = format!("save_incremental(&[\"{section}\"])");
                let Some(at) = src.find(&save) else { continue };
                checked += 1;

                // The enclosing item: everything from the last top-level `fn`
                // header before the write to the next one. Bounded by the
                // file's own syntax rather than by a line count, which drifts
                // onto a neighbouring declaration the first time something
                // above it grows.
                let start = ["\npub async fn ", "\npub fn ", "\nasync fn ", "\nfn "]
                    .iter()
                    .filter_map(|kw| src[..at].rfind(kw))
                    .max()
                    .unwrap_or(0);
                let end = ["\npub async fn ", "\npub fn ", "\nasync fn ", "\nfn "]
                    .iter()
                    .filter_map(|kw| src[at..].find(kw).map(|i| at + i))
                    .min()
                    .unwrap_or(src.len());
                let body = &src[start..end];

                if body.contains("apply_live_sections") {
                    continue;
                }

                // Exemption: `behavior` has no handle to poke — its arm in
                // this file is the literal `true`, because every reader
                // re-reads `output_mode` from the shared `Config`. Derived from
                // that arm's source, so giving `behavior` a real handle turns
                // this red and forces `behavior_config` to be wired.
                if name == "behavior_config.rs" && live_apply_src.contains("\"behavior\" => true,")
                {
                    continue;
                }

                missing.push(format!("{name} saves [{section}]"));
            }
        }

        assert!(
            checked >= 3,
            "expected a write site for each of the three live sections \
             ({LIVE_SECTIONS:?}); found {checked} — the scan stopped matching"
        );
        assert!(
            missing.is_empty(),
            "these handlers persist a section declared live but never run \
             `apply_live_sections`, so the change lands on disk while the \
             running process keeps its boot-time values under a success \
             response: {missing:?}"
        );
    }

    /// The census above (`every_live_section_has_an_apply_arm`) only proves
    /// the name is on both lists. `known_arms` is a hand-written third copy,
    /// so a missing `match` arm still passes it — the call falls through to
    /// `_ => false` and honestly downgrades. This asserts the wire itself: a
    /// live patch that disables the terminal must reach `close_all`, and the
    /// target must be reported as applied.
    ///
    /// Uses the process-global `pty::manager()` singleton deliberately, not
    /// a fresh `PtyManager` — the arm under test hardcodes the call to
    /// `crate::gateway::pty::manager()`, so a local instance would prove
    /// nothing about the real wire.
    ///
    /// ⚠️ That deliberate choice is why the serial key below is mandatory,
    /// not tidiness. `close_all` kills EVERY live session in the process,
    /// including the ones `gateway::handlers::pty`'s tests spawn on the same
    /// singleton and then assert about — and libtest runs this binary's
    /// tests on parallel threads. Measured before the key was added:
    /// `cargo test -p alephcore --lib -- gateway::handlers::pty
    /// config::live_apply` failed 5 runs out of 6, with a DIFFERENT subset
    /// of the handler tests red each time; the same handler module alone was
    /// 6/6 green. A full `--lib` run happened not to show it (8/8 clean),
    /// which is luck, not safety: the pair simply rarely overlaps among
    /// 17k tests. The counterpart key lives on every test in
    /// `gateway::handlers::pty::tests` as
    /// `#[serial_test::parallel(pty_global_manager)]` — they stay parallel
    /// with each other and only exclude this one — and a source-level census
    /// in that module fails by name if a new test there forgets it.
    #[test]
    #[serial_test::serial(pty_global_manager)]
    fn disabling_the_terminal_live_kills_sessions_through_apply_live_sections() {
        use crate::gateway::pty::SpawnOptions;

        let mgr = crate::gateway::pty::manager();
        let sid = mgr
            .spawn(&SpawnOptions::default())
            .expect("spawn")
            .session_id;

        let mut cfg = Config::default();
        cfg.policies.terminal.enabled = false;
        let applied = apply_live_sections(&cfg, &["policies"]);

        assert!(
            applied.contains(&"policies.terminal"),
            "a declared-live target that does not land is not live"
        );
        assert!(
            mgr.list().iter().all(|s| s.session_id != sid || s.closed),
            "the in-flight session must be gone, not merely reported gone"
        );
    }
}
