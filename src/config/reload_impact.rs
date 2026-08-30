//! `reload_impact` — when does a `config.toml` change actually take effect?
//!
//! Self-management SSOT. The agent edits `config.toml` through `self_config`'s
//! `update_config`, but [`crate::config::patcher::ConfigPatcher::apply`] only
//! refreshes the in-memory `Config` + the file on disk — it does **not** reach
//! runtime structures that captured their configuration at startup. As
//! [`crate::providers::route_handle`] states plainly: *"a config edit alone
//! never reaches the already-built chain — nothing rebuilds it."* Only `route`
//! carries explicit hot-swap wiring (the `RouteHandle` `ArcSwap`).
//!
//! Until now the reload rules lived as prose scattered through the `/self`
//! SKILL.md ("generation providers need restart", "route hot-applies", the
//! reliability sections "wire into the harness at startup"). The agent had to
//! remember that prose and frequently guessed wrong — telling the user a change
//! was live when it needed a restart, or vice-versa.
//!
//! This module turns that prose into one typed, deterministic mapping so the
//! `update_config` response can carry an accurate "what happens next" signal.
//! It mirrors the role of openclaw's `config-reload-plan.ts` (which classifies
//! every config path as `hot` / `restart` / `none`) but with stronger typing
//! and a conservative default: when a section's effect is uncertain, we report
//! `Restart`, because an unnecessary restart is a minor annoyance whereas a
//! falsely-"live" change is a silent failure.

use serde::Serialize;

/// When does a change to a given `config.toml` section take effect on the
/// running `aleph-server`?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadImpact {
    /// Hot-swapped onto the running runtime immediately — effective on the next
    /// prompt, no restart needed. Only sections with explicit live-apply wiring
    /// (currently `route`, via [`crate::providers::route_handle`]) qualify.
    Live,
    /// Persisted to `config.toml`, but the running runtime will not observe it
    /// until `aleph-server` restarts: the consuming subsystem captured this
    /// section at startup and nothing rebuilds it from a live edit.
    Restart,
    /// The section is parsed for backward compatibility but has no runtime
    /// consumer (legacy). Editing it persists to disk yet changes no behavior.
    Inert,
}

/// Top-level sections applied live (effective on the next conversation turn)
/// without a daemon restart.
///
/// Why each one qualifies:
/// - `route` — the failover chain reads its route state from a boot-installed
///   `ArcSwap`; storing into it is what makes a route change live.
/// - `behavior` — `output_mode` is re-read fresh from the shared config on
///   every run by all channel paths (`server_init`, the `inbound_router`
///   executor, and the `agent` handler's `resolved_output_mode`), so the
///   typewriter/instant switch takes effect on the next turn. Its liveness is
///   a property of the READER — no hot-swap call exists or is needed.
/// - `execution` — `[execution] max_runs_*` are pushed to the live
///   `ConcurrencyLimiter`; new caps bind on the next admission.
///
/// ⚠️ This is a **declaration**, and the code that executes it is
/// [`crate::config::live_apply::apply_live_sections`] — one table, one
/// executor, reached from the single write chokepoint (`ConfigPatcher`).
/// Do not re-implement the hot-apply at a call site: this list used to be
/// asserted by every write surface while only one of them acted on it, so a
/// `config.patch` of `route.mode` reported "no restart needed" and changed
/// nothing. `live_apply`'s `every_live_section_has_an_apply_arm` fails if an
/// entry here has no arm there, or vice versa.
pub(crate) const LIVE_SECTIONS: &[&str] = &["route", "behavior", "execution"];

/// Sub-sections applied live even though their *parent* top-level section is
/// not — the parent has other fields with no live-apply wiring, so declaring
/// the whole parent live (adding it to [`LIVE_SECTIONS`]) would advertise
/// "no restart needed" for those too. Checked *before* [`LIVE_SECTIONS`] in
/// [`dotted_prefix_matches`]-based lookups: most specific declaration wins.
///
/// Why each one qualifies:
/// - `policies.spend` — the per-principal/machine USD ceiling is read fresh
///   on every call from a boot-installed `ArcSwap`
///   ([`crate::spend::current_policy`]); storing a patched policy into it
///   (`crate::spend::update_policy`) is what makes a ceiling change live,
///   exactly like `route`'s `ArcSwap`. `[policies]`'s other fields
///   (`tool_permissions` and friends) have no such handle, hence the parent
///   section itself stays out of [`LIVE_SECTIONS`]. (Deliberately no count
///   here: a number in a comment is a list that rots. `PoliciesConfig` has
///   ten fields today; `terminal` is the only one this change added, so the
///   previous "six" was already wrong before it arrived.)
/// - `policies.terminal` — declared live because each of its three fields is
///   either applied at apply time or applies to work started afterwards, and
///   NONE of them silently requires a restart:
///   * `enabled` — read fresh from the live config on every `pty.spawn`, and
///     turning it off runs `PtyManager::close_all`, so the change is
///     complete when the patch returns.
///   * `max_sessions` — read fresh at spawn time (deliberately NOT a
///     `const`; a bare constant would make the key inert while this list
///     advertised it as live).
///   * `scrollback_lines` — applies to sessions started after the patch.
///     Sessions already running keep the ring they were built with, because
///     rewriting a live ring would destroy scrollback the user can still
///     see. No restart is required to get the new value — only a new
///     terminal.
pub(crate) const LIVE_SUBSECTIONS: &[&str] = &["policies.spend", "policies.terminal"];

/// Legacy top-level sections that are parsed but inert (no runtime consumer).
///
/// Mirrors the `⚠️ Legacy — parsed but inert` markers in the `/self` SKILL.md.
/// Editing these is a no-op at runtime.
///
/// Empty as of 2026-08-16: the former `task_routing` entry named a `Config`
/// section that no longer exists, so it was removed — the `dead_keys` scan
/// reports nonexistent sections at load time instead.
const INERT_SECTIONS: &[&str] = &[];

/// True when `prefix` is `path` itself, or a dot-segment ancestor of it
/// (e.g. `"policies"` of `"policies.spend"`, or `"policies.spend"` of
/// `"policies.spend.per_user_usd"`). A plain [`str::starts_with`] would
/// also match `"policies_v2"` against `"policies"` — the trailing `.`
/// requirement is what keeps this a *segment* boundary, not a substring
/// one.
///
/// The one prefix-matching primitive in this module. It is deliberately
/// argument-order-symmetric so both directions this module needs can reuse
/// it instead of hand-rolling their own match:
/// - [`live_target_for`] calls it as "is this *declared* target an ancestor
///   of the *specific* path being classified" (most specific target wins —
///   see that function's doc).
/// - [`crate::config::live_apply::apply_live_sections`] calls it with the
///   arguments swapped, as "is this *requested* (possibly coarser) section
///   name an ancestor of this *declared* target" — the single-patch caller
///   only ever knows the top-level section of the path it patched, so a
///   write to `"policies.spend.per_user_usd"` reaches that function as
///   `top_sections = ["policies"]`, and `"policies"` must still be
///   recognised as covering the `"policies.spend"` target. These are
///   opposite queries (one asks for the single best-matching *ancestor*, the
///   other asks "does any requested name cover this target"), so neither
///   can be phrased as a call to the other — but both reduce to this one
///   boundary check, which is the point: one hand-written prefix match, not
///   two that can drift apart.
pub(crate) fn dotted_prefix_matches(prefix: &str, path: &str) -> bool {
    prefix == path
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Resolve `config_path` to the most specific declared live target: a
/// subsection in [`LIVE_SUBSECTIONS`] if one covers it, else a top-level
/// section in [`LIVE_SECTIONS`], else `None`. Subsections are checked first
/// so a more specific declaration always outranks a coarser one — there is
/// no live top-level/subsection conflict today (no top-level section that
/// contains a live subsection is itself declared live), but the ordering is
/// the invariant this function exists to fix in place, not a reaction to a
/// conflict that currently exists.
pub(crate) fn live_target_for(config_path: &str) -> Option<&'static str> {
    LIVE_SUBSECTIONS
        .iter()
        .chain(LIVE_SECTIONS.iter())
        .find(|&&target| dotted_prefix_matches(target, config_path))
        .copied()
}

/// Every declared live target — [`LIVE_SECTIONS`] and [`LIVE_SUBSECTIONS`]
/// together — for callers that hot-apply an entire reloaded/rolled-back
/// config rather than one patched path (a whole-file operation can touch
/// any live target, not just the top-level ones). Passing [`LIVE_SECTIONS`]
/// alone to [`crate::config::live_apply::apply_live_sections`] from such a
/// caller is exactly how a whole-file rollback would silently skip
/// `policies.spend` — the same "one caller acts on the table, the others
/// only assert it" shape `live_apply` exists to remove.
pub(crate) fn live_targets() -> Vec<&'static str> {
    LIVE_SECTIONS
        .iter()
        .chain(LIVE_SUBSECTIONS.iter())
        .copied()
        .collect()
}

impl ReloadImpact {
    /// Classify a dot-path config target (e.g. `"providers.openai"`,
    /// `"route.mode"`, `"generation"`, `"policies.spend.per_user_usd"`) by
    /// the most specific declared live target that covers it — see
    /// [`live_target_for`].
    ///
    /// Conservative by design: anything not known to be live or inert is
    /// reported as [`ReloadImpact::Restart`]. In particular, the bare
    /// top-level `"policies"` path is `Restart`, not `Live` — only its
    /// `spend` subsection has live-apply wiring; see [`LIVE_SUBSECTIONS`].
    pub fn classify(config_path: &str) -> Self {
        if live_target_for(config_path).is_some() {
            return Self::Live;
        }
        let top = config_path.split('.').next().unwrap_or(config_path).trim();
        if INERT_SECTIONS.contains(&top) {
            Self::Inert
        } else {
            Self::Restart
        }
    }

    /// Actionable, model-facing guidance the agent can relay to the user.
    pub const fn agent_hint(&self) -> &'static str {
        match self {
            Self::Live => {
                "Applied live — the change takes effect on the next prompt; no restart needed."
            }
            Self::Restart => {
                "Saved to config.toml, but it will NOT take effect until aleph-server restarts \
                 (the running runtime captured this section at startup). Tell the user to restart Aleph."
            }
            Self::Inert => {
                "This section is legacy and has no runtime consumer — the value is saved but \
                 changes no behavior. Confirm with the user whether this is really what they intended."
            }
        }
    }

    /// User-facing Chinese guidance, for the `dry_run` preview message.
    pub const fn user_hint_zh(&self) -> &'static str {
        match self {
            Self::Live => "此改动将即时生效（下一轮对话），无需重启。",
            Self::Restart => "此改动会写入 config.toml，但需重启 aleph-server 后才会生效（运行时在启动时已捕获该配置段）。",
            Self::Inert => "该配置段为遗留段，无运行时消费者 —— 写入后不改变任何行为，请与用户确认是否确需修改。",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_is_live() {
        assert_eq!(ReloadImpact::classify("execution"), ReloadImpact::Live);
        assert_eq!(
            ReloadImpact::classify("execution.max_runs_global"),
            ReloadImpact::Live
        );
    }

    #[test]
    fn route_is_live() {
        assert_eq!(ReloadImpact::classify("route"), ReloadImpact::Live);
        assert_eq!(ReloadImpact::classify("route.mode"), ReloadImpact::Live);
        assert_eq!(
            ReloadImpact::classify("route.allow_cloud_escalation"),
            ReloadImpact::Live
        );
    }

    #[test]
    fn behavior_is_live() {
        // output_mode (typewriter/instant) is re-read fresh per run by every
        // channel path, so the switch takes effect next turn — not on restart.
        assert_eq!(ReloadImpact::classify("behavior"), ReloadImpact::Live);
        assert_eq!(
            ReloadImpact::classify("behavior.output_mode"),
            ReloadImpact::Live
        );
    }

    #[test]
    fn runtime_built_sections_need_restart() {
        // Sections whose runtime is built at startup — the conservative default.
        for s in [
            "generation",
            "channels",
            "providers",
            "providers.openai",
            "guardrails",
            "stability",
            "fallback_provider",
            "context_budget",
            "resume",
            "mcp",
            "gateway",
            "memory",
            "sandbox",
            // The bare parent section: only its `spend` child has live-apply
            // wiring (`policies.tool_permissions` and friends do not), so
            // patching "policies" as a whole must NOT report Live — that
            // would silently promise "no restart needed" for the other six
            // fields too. See `LIVE_SUBSECTIONS`'s doc.
            "policies",
            "policies.tool_permissions",
        ] {
            assert_eq!(
                ReloadImpact::classify(s),
                ReloadImpact::Restart,
                "expected '{s}' to need restart"
            );
        }
    }

    #[test]
    fn spend_subsection_is_live() {
        // The subsection itself, and any leaf path under it, are Live — but
        // the parent "policies" is not (covered by
        // `runtime_built_sections_need_restart`).
        assert_eq!(ReloadImpact::classify("policies.spend"), ReloadImpact::Live);
        assert_eq!(
            ReloadImpact::classify("policies.spend.per_user_usd"),
            ReloadImpact::Live
        );
        assert_eq!(
            ReloadImpact::classify("policies.spend.total_usd"),
            ReloadImpact::Live
        );
    }

    /// A security switch that only takes effect after a restart is not a
    /// switch. It is declared live, and the declaration is backed by a real
    /// handle (the gate reads the live config at spawn time).
    #[test]
    fn the_terminal_switch_is_declared_live() {
        assert!(
            LIVE_SUBSECTIONS.contains(&"policies.terminal"),
            "turning the terminal off must not wait for a restart"
        );
        assert_eq!(
            ReloadImpact::classify("policies.terminal"),
            ReloadImpact::Live
        );
        assert_eq!(
            ReloadImpact::classify("policies.terminal.enabled"),
            ReloadImpact::Live
        );
    }

    #[test]
    fn dotted_prefix_matches_is_a_segment_boundary_not_a_substring_match() {
        // A name that merely starts with the same characters as a declared
        // target must not match — only a `.`-bounded segment counts.
        assert!(!dotted_prefix_matches("policies", "policies_v2.foo"));
        assert!(!dotted_prefix_matches("route", "route_config.mode"));
        // Exact equality and a real segment boundary both count.
        assert!(dotted_prefix_matches("policies.spend", "policies.spend"));
        assert!(dotted_prefix_matches(
            "policies.spend",
            "policies.spend.per_user_usd"
        ));
    }

    #[test]
    fn live_targets_returns_every_section_and_subsection() {
        let targets = live_targets();
        for s in LIVE_SECTIONS {
            assert!(targets.contains(s), "missing top-level '{s}'");
        }
        for s in LIVE_SUBSECTIONS {
            assert!(targets.contains(s), "missing subsection '{s}'");
        }
        assert_eq!(targets.len(), LIVE_SECTIONS.len() + LIVE_SUBSECTIONS.len());
    }

    #[test]
    fn unknown_section_defaults_to_restart() {
        assert_eq!(
            ReloadImpact::classify("some_future_section"),
            ReloadImpact::Restart
        );
        assert_eq!(ReloadImpact::classify(""), ReloadImpact::Restart);
    }

    #[test]
    fn serde_uses_snake_case_labels() {
        // The `kind` field emitted to the agent relies on this representation.
        assert_eq!(
            serde_json::to_value(ReloadImpact::Live).unwrap(),
            serde_json::json!("live")
        );
        assert_eq!(
            serde_json::to_value(ReloadImpact::Restart).unwrap(),
            serde_json::json!("restart")
        );
        assert_eq!(
            serde_json::to_value(ReloadImpact::Inert).unwrap(),
            serde_json::json!("inert")
        );
    }

    #[test]
    fn hints_are_non_empty_and_distinct() {
        let impacts = [
            ReloadImpact::Live,
            ReloadImpact::Restart,
            ReloadImpact::Inert,
        ];
        for i in impacts {
            assert!(!i.agent_hint().is_empty());
            assert!(!i.user_hint_zh().is_empty());
        }
        assert_ne!(
            ReloadImpact::Live.agent_hint(),
            ReloadImpact::Restart.agent_hint()
        );
    }
}
