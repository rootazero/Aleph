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

/// Legacy top-level sections that are parsed but inert (no runtime consumer).
///
/// Mirrors the `⚠️ Legacy — parsed but inert` markers in the `/self` SKILL.md.
/// Editing these is a no-op at runtime.
///
/// Empty as of 2026-08-16: the former `task_routing` entry named a `Config`
/// section that no longer exists, so it was removed — the `dead_keys` scan
/// reports nonexistent sections at load time instead.
const INERT_SECTIONS: &[&str] = &[];

impl ReloadImpact {
    /// Classify a dot-path config target (e.g. `"providers.openai"`,
    /// `"route.mode"`, `"generation"`) by its top-level section.
    ///
    /// Conservative by design: anything not known to be live or inert is
    /// reported as [`ReloadImpact::Restart`].
    pub fn classify(config_path: &str) -> Self {
        let top = config_path.split('.').next().unwrap_or(config_path).trim();
        if LIVE_SECTIONS.contains(&top) {
            Self::Live
        } else if INERT_SECTIONS.contains(&top) {
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
        ] {
            assert_eq!(
                ReloadImpact::classify(s),
                ReloadImpact::Restart,
                "expected '{s}' to need restart"
            );
        }
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
