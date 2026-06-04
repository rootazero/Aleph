//! reload_impact — when does a `config.toml` change actually take effect?
//!
//! Self-management SSOT. The agent edits `config.toml` through `self_config`'s
//! `update_config`, but [`crate::config::patcher::ConfigPatcher::apply`] only
//! refreshes the in-memory `Config` + the file on disk — it does **not** reach
//! runtime structures that captured their configuration at startup. As
//! [`crate::providers::route_handle`] states plainly: *"a config edit alone
//! never reaches the already-built chain — nothing rebuilds it."* Only `route`
//! carries explicit hot-swap wiring (the `RouteHandle` ArcSwap).
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

/// Top-level sections applied live via explicit hot-swap wiring.
///
/// Keep in lock-step with the actual hot-apply call sites. Today the only one
/// is `route` — `self_config::update_config` and the gateway `route_config`
/// handler both call `route_handle.store(..)` after patching it.
const LIVE_SECTIONS: &[&str] = &["route"];

/// Legacy top-level sections that are parsed but inert (no runtime consumer).
///
/// Mirrors the `⚠️ Legacy — parsed but inert` markers in the `/self` SKILL.md.
/// Editing these is a no-op at runtime.
const INERT_SECTIONS: &[&str] = &[
    "smart_flow",
    "smart_matching",
    "dispatcher",
    "subagent",
    "task_routing",
    "evolution",
    "media",
];

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

    /// Short machine label (matches the serde representation).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Restart => "restart",
            Self::Inert => "inert",
        }
    }

    /// Actionable, model-facing guidance the agent can relay to the user.
    pub fn agent_hint(&self) -> &'static str {
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
    pub fn user_hint_zh(&self) -> &'static str {
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
    fn route_is_live() {
        assert_eq!(ReloadImpact::classify("route"), ReloadImpact::Live);
        assert_eq!(ReloadImpact::classify("route.mode"), ReloadImpact::Live);
        assert_eq!(
            ReloadImpact::classify("route.allow_cloud_escalation"),
            ReloadImpact::Live
        );
    }

    #[test]
    fn legacy_sections_are_inert() {
        for s in [
            "smart_flow",
            "smart_matching",
            "dispatcher",
            "subagent",
            "task_routing",
            "evolution",
            "media",
        ] {
            assert_eq!(
                ReloadImpact::classify(s),
                ReloadImpact::Inert,
                "expected '{s}' to be inert"
            );
        }
        // Nested path under a legacy section still classifies inert.
        assert_eq!(
            ReloadImpact::classify("dispatcher.fallback_policy"),
            ReloadImpact::Inert
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
    fn label_matches_serde() {
        assert_eq!(ReloadImpact::Live.label(), "live");
        assert_eq!(ReloadImpact::Restart.label(), "restart");
        assert_eq!(ReloadImpact::Inert.label(), "inert");
        // serde rename_all = "snake_case" agrees with label()
        let v = serde_json::to_value(ReloadImpact::Restart).unwrap();
        assert_eq!(v, serde_json::json!("restart"));
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
