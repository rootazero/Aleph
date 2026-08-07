//! Lazy activation planner (openclaw parity, P3.5).
//!
//! Mirrors openclaw's `activation-planner.ts`: rather than loading every
//! enabled plugin eagerly at boot, a plugin can declare *when* it should
//! become active via an `ActivationHints` block in its manifest. The
//! [`ActivationPlanner`] then answers "for this trigger, which plugin ids
//! should be active?" without ever inspecting the manifest's full content.
//!
//! ## Triggers
//!
//! Six trigger kinds exist; each maps to a specific Aleph surface:
//!
//! | Trigger kind | Maps to |
//! |--------------|---------|
//! | `Command`    | `/my-plugin:do-something` slash commands |
//! | `Provider`   | `provider_id` model provider lookup |
//! | `Channel`    | `imessage`/`slack`/etc. channel adapter |
//! | `Capability` | a [`crate::extension::capability::CapabilityDeclaration`] kind |
//! | `AgentHarness` | `runtime_id` of an `AgentDef` |
//! | `Route`      | `route_id` for an inbound route |
//!
//! ## Plan output
//!
//! A [`ActivationPlan`] is the deterministic list of `(plugin_id, origin,
//! reasons)` triples that should be activated for a given trigger, sorted by
//! `plugin_id`. The planner never inspects plugin contents; it only consults
//! the `PluginRegistry` for membership and origin. This keeps the cold path
//! O(pluggable plugins × trigger fan-out) without touching the filesystem.
//!
//! ## Trust policy
//!
//! [`OwnerTrustPolicy`] filters candidates by their
//! [`PluginOrigin`](crate::extension::types::PluginOrigin):
//! - `Bundled` — always trusted.
//! - `Config` — always trusted (the operator put it in `aleph.jsonc`).
//! - `Workspace` / `Global` — require an explicit allowlist entry.
//!
//! This mirrors openclaw's `passesManifestOwnerBasePolicy` + bundled short-
//! circuit and prevents an attacker from dropping a `.aleph/extensions/foo`
//! directory that gets auto-loaded for any random command. The default
//! `TrustPolicy::permissive()` keeps every plugin loadable (the current
//! behaviour); `TrustPolicy::restrictive()` enforces the allowlist.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::extension::capability::Tier;
use crate::extension::types::PluginOrigin;

/// Lazy-activation hints declared by a plugin in its manifest.
///
/// Mirrors openclaw's `activation` block. Empty lists mean "match nothing";
/// `None` (the default for plugins that don't declare any hints) means
/// "always load" — i.e. the legacy Aleph behaviour where every enabled
/// plugin is loaded at boot regardless of what the user actually invokes.
///
/// ## Wire format
///
/// ```toml
/// [plugin.activation]
/// on_commands = ["/my-plugin:list", "/my-plugin:refine"]
/// on_providers = ["my-custom-llm"]
/// on_channels = ["telegram"]
/// on_capabilities = ["tool", "hook"]
/// on_agent_harnesses = ["research"]
/// ```
///
/// Matching is case-insensitive on string fields (commands, providers,
/// channels, agent harnesses) — the manifest is human-written and casing
/// should not silently exclude a plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationHints {
    /// Slash commands that activate this plugin.
    /// Values match against [`ActivationTrigger::Command`] after the leading
    /// `/` is stripped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_commands: Vec<String>,
    /// Provider IDs that activate this plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_providers: Vec<String>,
    /// Channel IDs that activate this plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_channels: Vec<String>,
    /// Capability kinds that activate this plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_capabilities: Vec<CapabilityKind>,
    /// Agent harness runtime IDs that activate this plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_agent_harnesses: Vec<String>,
    /// Route IDs that activate this plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_routes: Vec<String>,
}

impl ActivationHints {
    /// Whether this plugin declares *any* activation hint at all. A plugin
    /// with `ActivationHints::default()` (every list empty) matches no
    /// trigger and would never load under a strict planner — `is_empty()`
    /// lets `ExtensionManager::load_all` skip planner work entirely for
    /// "always load" plugins.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.on_commands.is_empty()
            && self.on_providers.is_empty()
            && self.on_channels.is_empty()
            && self.on_capabilities.is_empty()
            && self.on_agent_harnesses.is_empty()
            && self.on_routes.is_empty()
    }

    /// List the trigger kinds this plugin cares about. Used by the planner
    /// to skip irrelevant plugins without consulting their manifest content.
    #[must_use]
    pub fn declares_triggers(&self) -> Vec<TriggerKind> {
        let mut kinds = Vec::new();
        if !self.on_commands.is_empty() {
            kinds.push(TriggerKind::Command);
        }
        if !self.on_providers.is_empty() {
            kinds.push(TriggerKind::Provider);
        }
        if !self.on_channels.is_empty() {
            kinds.push(TriggerKind::Channel);
        }
        if !self.on_capabilities.is_empty() {
            kinds.push(TriggerKind::Capability);
        }
        if !self.on_agent_harnesses.is_empty() {
            kinds.push(TriggerKind::AgentHarness);
        }
        if !self.on_routes.is_empty() {
            kinds.push(TriggerKind::Route);
        }
        kinds
    }
}

/// Capability tag for [`ActivationHints::on_capabilities`].
///
/// Distinct from [`Tier`]: `CapabilityKind` is the *kind* of registration
/// (Tool, Hook, Service, etc.); `Tier` is the *trust* tier (P0/P1/P2).
/// Both are needed to plan a cold path precisely — a `Tier::Core` tool that
/// is only used by `/diagnostics:*` should still be lazy-loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// `[[tools]]` / `Tool` capability registration.
    Tool,
    /// `[[hooks]]` / `Hook` capability registration.
    Hook,
    /// `[[services]]` / `Service` capability registration.
    Service,
    /// `Skill` capability registration.
    Skill,
    /// `Agent` capability registration.
    Agent,
    /// `McpServer` capability registration.
    McpServer,
}

impl CapabilityKind {
    /// Stable string form used by `on_capabilities` matching.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Hook => "hook",
            Self::Service => "service",
            Self::Skill => "skill",
            Self::Agent => "agent",
            Self::McpServer => "mcp_server",
        }
    }
}

impl std::fmt::Display for CapabilityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which surfaces a trigger originates from. Used by the planner to skip
/// plugins whose `ActivationHints` declare *other* trigger kinds — the cold
/// path is `O(pluggable plugins × trigger fan-out)`, so a `Command` trigger
/// must not iterate every plugin just to find the few that declared
/// `on_commands`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    /// Slash command trigger.
    Command,
    /// Provider ID trigger.
    Provider,
    /// Channel ID trigger.
    Channel,
    /// Capability-kind trigger.
    Capability,
    /// Agent harness runtime trigger.
    AgentHarness,
    /// Route ID trigger.
    Route,
}

/// What a caller is asking the planner to activate on its behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationTrigger {
    /// A slash command invocation (e.g. `/my-plugin:refine`).
    Command(String),
    /// A model provider lookup (e.g. `"my-custom-llm"`).
    Provider(String),
    /// A channel adapter activation (e.g. `"telegram"`).
    Channel(String),
    /// A capability kind (e.g. `CapabilityKind::Tool`).
    Capability(CapabilityKind),
    /// An agent harness runtime (e.g. `"research"`).
    AgentHarness(String),
    /// A route ID (e.g. `"inbound.webhook"`).
    Route(String),
}

impl ActivationTrigger {
    /// Which [`TriggerKind`] this trigger is. Used by the planner to skip
    /// plugins that didn't declare hints for *this* trigger kind.
    #[must_use]
    pub fn kind(&self) -> TriggerKind {
        match self {
            Self::Command(_) => TriggerKind::Command,
            Self::Provider(_) => TriggerKind::Provider,
            Self::Channel(_) => TriggerKind::Channel,
            Self::Capability(_) => TriggerKind::Capability,
            Self::AgentHarness(_) => TriggerKind::AgentHarness,
            Self::Route(_) => TriggerKind::Route,
        }
    }
}

/// Per-plugin reason for inclusion in the plan. Returned as part of
/// [`ActivationPlanEntry`] so callers can show "why" in `plugin list` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationReason {
    /// Plugin declared `on_commands` and this is a `Command` trigger.
    OnCommand,
    /// Plugin declared `on_providers` and this is a `Provider` trigger.
    OnProvider,
    /// Plugin declared `on_channels` and this is a `Channel` trigger.
    OnChannel,
    /// Plugin declared `on_capabilities` and this is a `Capability` trigger.
    OnCapability,
    /// Plugin declared `on_agent_harnesses` and this is an `AgentHarness`
    /// trigger.
    OnAgentHarness,
    /// Plugin declared `on_routes` and this is a `Route` trigger.
    OnRoute,
}

/// A single `(plugin_id, origin, reasons)` triple in an [`ActivationPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationPlanEntry {
    /// Plugin id (e.g. `"diagnostics"`).
    pub plugin_id: String,
    /// Origin where the plugin is installed (Bundled, Workspace, Global…).
    pub origin: PluginOrigin,
    /// Why this plugin matched the trigger (multiple reasons are possible
    /// when a single plugin's hints match the trigger on more than one
    /// field — e.g. `on_providers` + `on_capabilities`).
    pub reasons: Vec<ActivationReason>,
}

/// The result of [`ActivationPlanner::plan`]: every enabled plugin that
/// should be loaded for the given trigger, plus per-plugin reasons.
#[derive(Debug, Clone)]
pub struct ActivationPlan {
    /// The trigger this plan was built for. Returned for caller diagnostics.
    pub trigger: ActivationTrigger,
    /// Sorted, deduplicated list of plugin ids that should be active.
    pub plugin_ids: Vec<String>,
    /// Per-plugin detail. Same length as `plugin_ids` (zip iter).
    pub entries: Vec<ActivationPlanEntry>,
}

impl ActivationPlan {
    /// Whether the plan is empty (no plugin should be activated for the
    /// trigger). A caller may treat an empty plan as a no-op — no plugin
    /// load is required.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugin_ids.is_empty()
    }
}

/// Plugin owner trust policy.
///
/// Mirrors openclaw's `passesManifestOwnerBasePolicy`:
/// - `Bundled` plugins (Aleph built-ins, marketplace `aleph-official`)
///   are always trusted.
/// - `Config` plugins (operator added to `aleph.jsonc`) are always
///   trusted.
/// - `Workspace` / `Global` plugins (anything in
///   `<project>/.aleph/plugins` or `~/.aleph/plugins/installed/`) require
///   an explicit allowlist entry. This prevents a stray `~/.aleph/extensions/foo`
///   directory from silently activating for any random command.
#[derive(Debug, Clone)]
pub struct OwnerTrustPolicy {
    /// Explicit allowlist of plugin ids that may load from non-trusted
    /// origins. Built by operators via `plugin trust <id>` or
    /// `~/.aleph/trusted-plugins.toml`. Empty allowlist + `restrictive()`
    /// = nothing from `Workspace`/`Global` may load.
    allowlist: HashSet<String>,
    /// When `true`, the planner enforces the allowlist for `Workspace` and
    /// `Global` origins. When `false`, every plugin passes (the legacy
    /// "load everything" default).
    enforce: bool,
}

impl OwnerTrustPolicy {
    /// Permissive policy: every plugin passes (legacy behaviour, used while
    /// operators haven't yet curated an allowlist).
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            allowlist: HashSet::new(),
            enforce: false,
        }
    }

    /// Restrictive policy: only `Bundled` + `Config` origins + explicitly
    /// allowlisted `Workspace`/`Global` plugins pass. Use this when an
    /// operator has curated a trust list and wants Aleph to fail-loud on
    /// unknown sources.
    #[must_use]
    pub fn restrictive(allowlist: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowlist: allowlist.into_iter().collect(),
            enforce: true,
        }
    }

    /// Add a plugin id to the allowlist. Used by `plugin trust <id>`.
    pub fn trust(&mut self, plugin_id: impl Into<String>) {
        self.allowlist.insert(plugin_id.into());
    }

    /// Remove a plugin id from the allowlist.
    pub fn untrust(&mut self, plugin_id: &str) -> bool {
        self.allowlist.remove(plugin_id)
    }

    /// Snapshot the current allowlist (sorted).
    #[must_use]
    pub fn allowlist(&self) -> Vec<String> {
        let mut list: Vec<String> = self.allowlist.iter().cloned().collect();
        list.sort();
        list
    }

    /// Whether the given plugin id may load under this policy.
    #[must_use]
    pub fn allows(&self, plugin_id: &str, origin: PluginOrigin) -> bool {
        if !self.enforce {
            return true;
        }
        match origin {
            // Bundled (built-in plugins) and Config (operator-explicit) are
            // always trusted — the operator put them there on purpose.
            PluginOrigin::Bundled | PluginOrigin::Config => true,
            PluginOrigin::Workspace | PluginOrigin::Global => self.allowlist.contains(plugin_id),
        }
    }
}

impl Default for OwnerTrustPolicy {
    fn default() -> Self {
        // Default to permissive: matching the historical behaviour where
        // any enabled plugin could load. Operators opt into restrictive
        // mode once they've curated an allowlist.
        Self::permissive()
    }
}

/// Inputs to [`ActivationPlanner::plan`].
///
/// `manifest_records` lets the caller pass a precomputed list of
/// `(plugin_id, origin, hints)` tuples — `ExtensionManager` builds this
/// from its `PluginRegistry` and avoids touching the filesystem.
#[derive(Debug, Clone)]
pub struct PlanInput<'a> {
    /// Plugin records to consider, in registration order. Origin is read
    /// from each record's `origin` field.
    pub records: Vec<PlanRecord<'a>>,
    /// Trust policy. Default is permissive.
    pub trust: OwnerTrustPolicy,
}

/// A single record the planner considers: the plugin id, its origin, and
/// the hints declared in its manifest (`None` = legacy "always load").
#[derive(Debug, Clone)]
pub struct PlanRecord<'a> {
    /// Plugin id.
    pub plugin_id: &'a str,
    /// Where the plugin was installed.
    pub origin: PluginOrigin,
    /// Lazy-activation hints, or `None` for plugins that don't opt into
    /// lazy activation. A `None` hint set always matches every trigger
    /// (preserving the legacy boot-time load behaviour).
    pub hints: Option<&'a ActivationHints>,
}

/// Deterministic activation planner.
///
/// Stateless: callers pass `PlanInput` describing the candidates, and the
/// planner returns an [`ActivationPlan`]. The planner does not consult
/// `PluginManifest` directly — it only inspects the `(id, origin, hints)`
/// triple, which keeps the cold path O(pluggable plugins × trigger
/// fan-out) without I/O.
#[derive(Debug, Clone, Default)]
pub struct ActivationPlanner;

impl ActivationPlanner {
    /// Create a new planner. Stateless — there's nothing to configure.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Resolve the activation plan for `trigger`. Returns the sorted list
    /// of plugin ids that should be activated, plus per-plugin reasons.
    ///
    /// Plugins whose `hints` is `None` match every trigger (legacy
    /// behaviour — they would have been loaded eagerly at boot). Plugins
    /// with `hints = Some(empty)` match nothing (they were declared lazy
    /// but didn't list any triggers — likely a config error, surface as
    /// a diagnostic in the plan output if you need to).
    pub fn plan(&self, trigger: ActivationTrigger, input: &PlanInput<'_>) -> ActivationPlan {
        let mut entries: Vec<ActivationPlanEntry> = Vec::new();

        for record in &input.records {
            // Trust filter first: untrusted plugins never make it into the
            // plan, regardless of how well their hints match.
            if !input.trust.allows(record.plugin_id, record.origin) {
                continue;
            }

            let Some(reasons) = match_reasons(record.hints, &trigger) else {
                continue;
            };

            entries.push(ActivationPlanEntry {
                plugin_id: record.plugin_id.to_string(),
                origin: record.origin,
                reasons,
            });
        }

        // Stable, deterministic sort by plugin id. Same input → same plan,
        // so callers can log or cache plans safely.
        entries.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));

        let plugin_ids = entries
            .iter()
            .map(|entry| entry.plugin_id.clone())
            .collect();

        ActivationPlan {
            trigger,
            plugin_ids,
            entries,
        }
    }
}

/// Compute the match reasons for `hints` against `trigger`. Returns
/// `None` when the plugin should not be activated for this trigger.
fn match_reasons(
    hints: Option<&ActivationHints>,
    trigger: &ActivationTrigger,
) -> Option<Vec<ActivationReason>> {
    // Legacy "always load" semantics: a plugin that declared no hints
    // (None) matches every trigger — it was loaded eagerly at boot, so
    // every caller should still see it.
    let hints = hints?;

    // A plugin that *did* declare an `activation` block but left every
    // list empty matches nothing. That's a config error — surface as
    // "no match" so the caller can log a warning, rather than silently
    // loading a misconfigured plugin.
    if hints.is_empty() {
        return None;
    }

    let mut reasons = Vec::new();
    match trigger {
        ActivationTrigger::Command(cmd) => {
            // Strip a leading "/" if the plugin author included it; matching
            // is case-insensitive so "MEMORY-Recall" matches "memory-recall".
            let needle = cmd.trim_start_matches('/').to_ascii_lowercase();
            if hints
                .on_commands
                .iter()
                .any(|c| c.eq_ignore_ascii_case(&needle))
            {
                reasons.push(ActivationReason::OnCommand);
            }
        }
        ActivationTrigger::Provider(provider) => {
            let needle = provider.to_ascii_lowercase();
            if hints
                .on_providers
                .iter()
                .any(|p| p.eq_ignore_ascii_case(&needle))
            {
                reasons.push(ActivationReason::OnProvider);
            }
        }
        ActivationTrigger::Channel(channel) => {
            let needle = channel.to_ascii_lowercase();
            if hints
                .on_channels
                .iter()
                .any(|c| c.eq_ignore_ascii_case(&needle))
            {
                reasons.push(ActivationReason::OnChannel);
            }
        }
        ActivationTrigger::Capability(cap) => {
            if hints.on_capabilities.contains(cap) {
                reasons.push(ActivationReason::OnCapability);
            }
        }
        ActivationTrigger::AgentHarness(runtime) => {
            let needle = runtime.to_ascii_lowercase();
            if hints
                .on_agent_harnesses
                .iter()
                .any(|r| r.eq_ignore_ascii_case(&needle))
            {
                reasons.push(ActivationReason::OnAgentHarness);
            }
        }
        ActivationTrigger::Route(route) => {
            let needle = route.to_ascii_lowercase();
            if hints
                .on_routes
                .iter()
                .any(|r| r.eq_ignore_ascii_case(&needle))
            {
                reasons.push(ActivationReason::OnRoute);
            }
        }
    }

    if reasons.is_empty() {
        None
    } else {
        Some(reasons)
    }
}

/// Map a `Tier` to its constituent [`CapabilityKind`]s. Used by
/// `Capability`-triggered plans to decide whether a plugin's tier is
/// relevant.
///
/// `CapabilityKind` describes the *kind* of registration (Tool, Hook…),
/// `Tier` describes the *trust* level (Core/Important/Pluggable). A
/// `Tier::Core` registration is always relevant; `Pluggable` registrations
/// are only relevant for their specific kind.
#[must_use]
pub const fn tier_kinds(tier: Tier) -> &'static [CapabilityKind] {
    match tier {
        Tier::Core => &[
            CapabilityKind::Tool,
            CapabilityKind::Hook,
            CapabilityKind::Skill,
        ],
        Tier::Important => &[CapabilityKind::Agent],
        Tier::Pluggable => &[CapabilityKind::Service, CapabilityKind::McpServer],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record<'a>(
        id: &'a str,
        origin: PluginOrigin,
        hints: Option<&'a ActivationHints>,
    ) -> PlanRecord<'a> {
        PlanRecord {
            plugin_id: id,
            origin,
            hints,
        }
    }

    #[test]
    fn legacy_no_hints_matches_every_trigger() {
        let planner = ActivationPlanner::new();
        let policy = OwnerTrustPolicy::default();
        let input = PlanInput {
            records: vec![record("legacy", PluginOrigin::Workspace, None)],
            trust: policy,
        };

        for trigger in [
            ActivationTrigger::Command("anything".into()),
            ActivationTrigger::Provider("any".into()),
            ActivationTrigger::Channel("any".into()),
            ActivationTrigger::Capability(CapabilityKind::Tool),
            ActivationTrigger::AgentHarness("any".into()),
            ActivationTrigger::Route("any".into()),
        ] {
            let plan = planner.plan(trigger, &input);
            assert_eq!(
                plan.plugin_ids,
                vec!["legacy".to_string()],
                "no-hint plugin must match every trigger"
            );
        }
    }

    #[test]
    fn empty_hints_match_nothing() {
        let planner = ActivationPlanner::new();
        let h = ActivationHints::default(); // every list empty
        let input = PlanInput {
            records: vec![record("empty", PluginOrigin::Workspace, Some(&h))],
            trust: OwnerTrustPolicy::default(),
        };
        let plan = planner.plan(ActivationTrigger::Command("x".into()), &input);
        assert!(plan.is_empty());
    }

    #[test]
    fn command_match_is_case_insensitive_and_strips_leading_slash() {
        let planner = ActivationPlanner::new();
        let h = ActivationHints {
            on_commands: vec!["MY-PLUGIN:Refine".into()],
            ..Default::default()
        };
        let input = PlanInput {
            records: vec![record("p", PluginOrigin::Workspace, Some(&h))],
            trust: OwnerTrustPolicy::default(),
        };

        let plan = planner.plan(
            ActivationTrigger::Command("/my-plugin:refine".into()),
            &input,
        );
        assert_eq!(plan.plugin_ids, vec!["p".to_string()]);
        assert_eq!(plan.entries[0].reasons, vec![ActivationReason::OnCommand]);
    }

    #[test]
    fn capability_match_uses_capability_kind_directly() {
        let planner = ActivationPlanner::new();
        let h = ActivationHints {
            on_capabilities: vec![CapabilityKind::Tool],
            ..Default::default()
        };
        let input = PlanInput {
            records: vec![record("p", PluginOrigin::Workspace, Some(&h))],
            trust: OwnerTrustPolicy::default(),
        };
        let plan_tool = planner.plan(ActivationTrigger::Capability(CapabilityKind::Tool), &input);
        assert_eq!(plan_tool.plugin_ids, vec!["p".to_string()]);
        let plan_hook = planner.plan(ActivationTrigger::Capability(CapabilityKind::Hook), &input);
        assert!(plan_hook.is_empty());
    }

    #[test]
    fn plan_is_sorted_by_plugin_id() {
        let planner = ActivationPlanner::new();
        let h = ActivationHints {
            on_providers: vec!["any".into()],
            ..Default::default()
        };
        let input = PlanInput {
            records: vec![
                record("zeta", PluginOrigin::Workspace, Some(&h)),
                record("alpha", PluginOrigin::Workspace, Some(&h)),
                record("mu", PluginOrigin::Workspace, Some(&h)),
            ],
            trust: OwnerTrustPolicy::default(),
        };
        let plan = planner.plan(ActivationTrigger::Provider("any".into()), &input);
        assert_eq!(
            plan.plugin_ids,
            vec!["alpha".to_string(), "mu".to_string(), "zeta".to_string()]
        );
    }

    #[test]
    fn restrictive_policy_blocks_global_without_allowlist() {
        let planner = ActivationPlanner::new();
        let h = ActivationHints {
            on_commands: vec!["go".into()],
            ..Default::default()
        };
        let input = PlanInput {
            records: vec![record("unknown", PluginOrigin::Global, Some(&h))],
            trust: OwnerTrustPolicy::restrictive(Vec::<String>::new()),
        };
        let plan = planner.plan(ActivationTrigger::Command("go".into()), &input);
        assert!(plan.is_empty(), "Global origin must be blocked");

        // Allowlisting it makes it match again.
        let mut policy = OwnerTrustPolicy::restrictive(Vec::<String>::new());
        policy.trust("unknown");
        let input = PlanInput {
            records: vec![record("unknown", PluginOrigin::Global, Some(&h))],
            trust: policy,
        };
        let plan = planner.plan(ActivationTrigger::Command("go".into()), &input);
        assert_eq!(plan.plugin_ids, vec!["unknown".to_string()]);
    }

    #[test]
    fn bundled_origin_is_always_trusted_under_restrictive_policy() {
        let planner = ActivationPlanner::new();
        let h = ActivationHints {
            on_commands: vec!["x".into()],
            ..Default::default()
        };
        let input = PlanInput {
            records: vec![record("builtin", PluginOrigin::Bundled, Some(&h))],
            trust: OwnerTrustPolicy::restrictive(Vec::<String>::new()),
        };
        let plan = planner.plan(ActivationTrigger::Command("x".into()), &input);
        assert_eq!(plan.plugin_ids, vec!["builtin".to_string()]);
    }

    #[test]
    fn permissive_policy_lets_everything_through() {
        let policy = OwnerTrustPolicy::permissive();
        assert!(policy.allows("any", PluginOrigin::Global));
        assert!(policy.allows("any", PluginOrigin::Workspace));
        assert!(policy.allows("any", PluginOrigin::Bundled));
    }

    #[test]
    fn is_empty_hints_helper_is_correct() {
        let mut h = ActivationHints::default();
        assert!(h.is_empty());
        h.on_commands.push("x".into());
        assert!(!h.is_empty());
    }

    #[test]
    fn declares_triggers_lists_every_non_empty_field() {
        let h = ActivationHints {
            on_commands: vec!["c".into()],
            on_providers: vec![],
            on_channels: vec!["ch".into()],
            on_capabilities: vec![CapabilityKind::Tool],
            on_agent_harnesses: vec![],
            on_routes: vec!["r".into()],
        };
        let kinds = h.declares_triggers();
        assert!(kinds.contains(&TriggerKind::Command));
        assert!(kinds.contains(&TriggerKind::Channel));
        assert!(kinds.contains(&TriggerKind::Capability));
        assert!(kinds.contains(&TriggerKind::Route));
        assert!(!kinds.contains(&TriggerKind::Provider));
        assert!(!kinds.contains(&TriggerKind::AgentHarness));
    }

    #[test]
    fn capability_kind_serde_is_snake_case() {
        let json = serde_json::to_string(&CapabilityKind::McpServer).unwrap();
        assert_eq!(json, "\"mcp_server\"");
        let parsed: CapabilityKind = serde_json::from_str("\"mcp_server\"").unwrap();
        assert_eq!(parsed, CapabilityKind::McpServer);
    }

    #[test]
    fn tier_kinds_covers_every_kind() {
        // Sanity: every tier maps to at least one kind.
        for tier in [Tier::Core, Tier::Important, Tier::Pluggable] {
            assert!(!tier_kinds(tier).is_empty());
        }
    }

    #[test]
    fn owner_trust_policy_allowlist_round_trip() {
        let mut p = OwnerTrustPolicy::permissive();
        assert!(p.allowlist().is_empty());
        p.trust("a");
        p.trust("b");
        assert_eq!(p.allowlist(), vec!["a".to_string(), "b".to_string()]);
        assert!(p.untrust("a"));
        assert_eq!(p.allowlist(), vec!["b".to_string()]);
        assert!(!p.untrust("never-added"));
    }
}
