//! Configuration structures
//!
//! This module defines the core configuration structures for Aleph.

use crate::config::types::{
    AcpConfig, AgentsConfig, BehaviorConfig, ContextBudgetToml, ExecutionConfig,
    FallbackProviderToml, FetchConfigInternal, GeneralConfig, GenerationConfig, GroupChatConfig,
    GuardrailsToml, McpConfig, MemoryConfig, PersonaConfig, PoliciesConfig, PrivacyConfig,
    ProfileConfig, PromptSectionConfig, ProviderConfig, RoutingRuleConfig, SearchConfigInternal,
    SecretProviderConfig, SecretsConfig, ShellSecurityConfig, StabilityToml, StopHookConfig,
    TeamBroadcastConfigToml, TeamDispatcherConfigToml, TeamMessagesConfigToml, ToolServiceConfig,
    ToolsConfig, UnifiedToolsConfig, VoiceLocalConfig, VoiceSection,
};
use crate::tasks::cron::CronConfig;
use crate::tasks::heartbeat::config::HeartbeatConfig;
use crate::tasks::shared::reaper::ReaperConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// PluginMarketplaceEntry
// =============================================================================

/// Plugin marketplace entry for config.toml.
///
/// Defined locally to avoid a circular dependency between the config and
/// extension modules.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PluginMarketplaceEntry {
    /// Source: "owner/repo" for GitHub, "/path/to/dir" for local
    pub source: String,
    /// Source type: "github" or "local"
    #[serde(rename = "type", default = "default_marketplace_type")]
    pub source_type: String,
}

fn default_marketplace_type() -> String {
    "github".to_string()
}

fn is_default_session(s: &crate::routing::config::SessionConfig) -> bool {
    s == &crate::routing::config::SessionConfig::default()
}

// =============================================================================
// Config
// =============================================================================

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,
    /// Memory module configuration
    #[serde(default)]
    pub memory: MemoryConfig,
    /// AI provider configurations (Phase 5)
    /// Note: Not exposed through `UniFFI` dictionary, managed via separate methods
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, ProviderConfig>,
    /// Routing rules for smart AI provider selection (Phase 5)
    ///
    /// Emitted even when empty, for the reason spelled out on
    /// [`Config::plugin_marketplaces`]: a section that skips itself when empty
    /// cannot be *cleared* through `save_incremental`, and `routing_rules.rs`'s
    /// delete handler persists exactly this section. Deleting the last rule
    /// used to report success and change nothing.
    #[serde(default)]
    pub rules: Vec<RoutingRuleConfig>,
    /// Behavior configuration (Phase 6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<BehaviorConfig>,
    /// Search configuration (Search Capability Integration)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchConfigInternal>,
    /// Fetch (URL→markdown) provider configuration. Parallel to `search`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch: Option<FetchConfigInternal>,
    /// System Tools configuration (Tier 1: native Rust tools)
    #[serde(default)]
    pub tools: ToolsConfig,
    /// MCP (Model Context Protocol) configuration (Tier 2: external servers)
    #[serde(default)]
    pub mcp: McpConfig,
    /// Unified tools configuration (Phase 1 refactor: combines tools + mcp)
    /// If present, takes precedence over legacy [tools] and [mcp] sections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified_tools: Option<UnifiedToolsConfig>,
    /// Phase 2 `ToolService` runtime tunables (timeouts, per-tool overrides)
    #[serde(default)]
    pub tool_service: ToolServiceConfig,
    /// Phase 3 Sandbox runtime tunables (workspace root, timeout, output cap,
    /// enabled toggle). Exec-class tools route through the sandbox built
    /// from this config at boot.
    #[serde(default)]
    pub sandbox: crate::sandbox::SandboxConfig,
    /// Agent task orchestration configuration (renamed from cowork)
    /// Supports both [agent] and [cowork] sections for backward compatibility
    // (removed: `[agent]` section — see config-002 in the 2026-08-17 wire audit.
    //  CoworkConfigToml/FileOpsConfigToml/CodeExecConfigToml were parsed and
    //  validated but had no production consumer; the validator's only check
    //  was that `planner_provider` existed as a `[providers.*]` key, which
    //  consumed zero of the field's value. Capability boundaries are now
    //  owned by `AgentDefinition.subagents`, `AgentDef.allowed_tools`, and the
    //  exec-tier gate in `src/tools/scoped/`. Existing `config.toml` keeps
    //  parsing because `Config` does not `deny_unknown_fields`.)
    /// Policies configuration (mechanism-policy separation)
    /// Contains configurable behavioral parameters extracted from mechanism code
    #[serde(default)]
    pub policies: PoliciesConfig,
    /// Generation providers configuration (image, speech, audio, video)
    #[serde(default)]
    pub generation: GenerationConfig,
    /// Local voice ([voice.local]) — BYO OpenAI-compatible STT/TTS endpoint.
    #[serde(default, rename = "voice")]
    pub voice_local: VoiceSection,
    /// Local-vs-cloud failover routing mode. `Auto` (default) is a no-op:
    /// failover candidate order is unchanged. `AlwaysLocal`/`AlwaysCloud`
    /// shape the chain by endpoint tier (see `[route]`).
    #[serde(default)]
    pub route: crate::config::types::ModelRouteConfig,
    /// Group chat configuration (multi-agent persona orchestration)
    #[serde(default)]
    pub group_chat: GroupChatConfig,
    /// Cron job scheduling configuration
    #[serde(default)]
    pub cron: CronConfig,
    /// Heartbeat monitoring configuration
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
    /// Periodic task-history reaper daemon (cleans `cron_job_runs`,
    /// `heartbeat_runs`, and `heartbeat_dedup` on a single cadence).
    #[serde(default, alias = "task_reaper")]
    pub tasks_reaper: ReaperConfig,
    /// Preset persona definitions for group chat
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personas: Vec<PersonaConfig>,
    /// Privacy and PII filtering configuration
    #[serde(default)]
    pub privacy: PrivacyConfig,
    /// Shell security configuration (custom risk patterns)
    #[serde(default)]
    pub security: ShellSecurityConfig,
    /// SSRF protection configuration
    #[serde(default)]
    pub ssrf: crate::security::ssrf::SsrfPolicy,
    /// Workspace profiles configuration (Anti-Gravity Architecture)
    /// Profiles define the "Physics" of workspaces: model binding, tool whitelist, system prompt
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, ProfileConfig>,
    /// Secret provider backends (e.g., `local_vault`, 1password, bitwarden)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub secret_providers: HashMap<String, SecretProviderConfig>,
    /// Top-level secrets subsystem settings
    #[serde(default)]
    pub secrets_config: SecretsConfig,
    /// Prompt customization (extra files injection, etc.)
    #[serde(default)]
    pub prompt: PromptSectionConfig,
    /// Channel configurations (runtime channel control)
    /// Each key is a channel name (e.g. "telegram", "discord"), value is channel-specific config.
    /// This uses opaque JSON values since each channel has a different schema.
    ///
    /// Emitted even when empty, for the reason spelled out on
    /// [`Config::plugin_marketplaces`]: `channel.rs`'s delete handler persists
    /// this section, and a section that skips itself when empty cannot be
    /// cleared — removing the last channel reported success and left it on
    /// disk to come back at the next load.
    #[serde(default)]
    pub channels: HashMap<String, serde_json::Value>,
    /// A2A protocol configuration
    #[serde(default)]
    pub a2a: crate::a2a::config::A2AConfig,
    /// ACP (Agent Communication Protocol) harness configuration
    #[serde(default)]
    pub acp: AcpConfig,
    /// Execution engine configuration (timeout, iteration limits)
    #[serde(default)]
    pub execution: ExecutionConfig,
    /// Agent definitions for multi-agent configuration
    /// Defines available agents, their workspaces, profiles, and capabilities
    #[serde(default)]
    pub agents: AgentsConfig,
    /// Channel → Agent routing bindings
    /// Maps channel/peer patterns to specific agents using `RouteBinding`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<crate::routing::config::RouteBinding>,
    /// DM scope. A single-user setup with `dm_scope = "main"` collapses
    /// every channel's DM into the same agent's Main session (one brain,
    /// many devices, continuous context). Default `per-peer` (safe for
    /// multi-user deployments).
    #[serde(default, skip_serializing_if = "is_default_session")]
    pub session: crate::routing::config::SessionConfig,
    /// Plugin marketplace registrations.
    ///
    /// Deliberately **not** `skip_serializing_if = "HashMap::is_empty"`, unlike
    /// its neighbours. `save_incremental` persists a named section by finding
    /// it in the serialised current config; a section that skips itself when
    /// empty simply is not there, and `merge_sections` treats "not there" as
    /// "the caller did not mean this one" — it warns and leaves the file
    /// alone. That fail-soft is deliberate (the guards in `save.rs` exist
    /// because a partially-populated `Config` once erased on-disk embedding
    /// providers), so the fix belongs here: emitting the empty table makes
    /// "no marketplaces" a value the merge can write rather than an absence it
    /// has to interpret.
    ///
    /// Removing the *last* registration is the case that breaks. It reported
    /// success, logged a `warn!` nobody reads, and left the entry on disk — on
    /// both paths that persist it (`plugin.marketplace.remove` and
    /// `aleph-server plugin marketplace remove`), which is why this is fixed on
    /// the field rather than at either call site.
    #[serde(default)]
    pub plugin_marketplaces: HashMap<String, PluginMarketplaceEntry>,
    /// Stop-hook entries (Phase 6b Task 10).
    /// Each entry runs a shell command before the agent loop is allowed to
    /// terminate; exit code 2 blocks the stop with stdout as the reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_hooks: Vec<StopHookConfig>,
    /// Phase-6 wiring (#12) — single-switch guardrails section. When `Some`
    /// and `enabled = true`, the orchestrator wires `PiiSecretsGuardrail`
    /// onto Input + Output + `ToolCall` surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardrails: Option<GuardrailsToml>,
    /// Phase-6 wiring (#12) — P0 rescue knobs (stall / consecutive failure
    /// cap / per-turn timeout). Each sub-field is independently optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stability: Option<StabilityToml>,
    /// Phase-6 wiring (#12) — Stage 5b single-step fallback provider. Refers
    /// to an existing `[providers.<key>]` entry by toml key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_provider: Option<FallbackProviderToml>,
    /// Opt-in mid-run context-window management. When `Some` and
    /// `enabled = true`, the orchestrator builds a per-run `ContextBudget` +
    /// `ContextCompactor` so long Think→Act runs compact history instead of
    /// hard-failing on a provider context-length error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_budget: Option<ContextBudgetToml>,
    /// Opt-in (default **on**) strategic-planner welding for the three long-task
    /// flows (`/goal`, `/loop`, `/workflow`). When `Some` and `enabled = true`
    /// (the default when the section is present), the start path builds a
    /// one-shot planner that welds a short `Strategy` into the cacheable
    /// system-prompt prefix. Absent section ⇒ `None` ⇒ planner uses the executor
    /// provider with the feature defaulting on; `enabled = false` is the
    /// off-switch. Fully fail-soft: any failure leaves prompts byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<crate::config::types::phase6_wiring::StrategyToml>,
    /// MoA (Mixture of Agents) continuous-advisory presets (`[moa]`). When
    /// present and a session activates MoA, run construction wraps the brain
    /// in a `MoaProvider` facade (advisors consult in parallel; the preset's
    /// aggregator acts). Absent ⇒ feature dormant, zero cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moa: Option<crate::config::types::moa::MoaToml>,
    /// Team `TeamDispatcher` tunables (`[team_dispatcher]`): retry budget,
    /// backoff, zombie TTL, concurrency, per-owner fairness cap. Absent ⇒
    /// `teams::dispatcher::DispatcherConfig::default()` at the boot site
    /// (byte-identical prior behaviour). Distinct from the L3-Cortex
    /// `dispatcher` field above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_dispatcher: Option<TeamDispatcherConfigToml>,
    /// Group-chat broadcast storm-prevention guards (`[team_broadcast]`): chain
    /// depth, fan-out width, total-activation cap, transcript token budget.
    /// Absent ⇒ `teams::broadcast::BroadcastConfig::default()` at the boot site
    /// (byte-identical prior behaviour). The broadcast-side parallel to
    /// `team_dispatcher` above (§4.5 ↔ §4.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_broadcast: Option<TeamBroadcastConfigToml>,
    /// Team message-router thread-escalation tunables (`[team_messages]`):
    /// per-thread message threshold + on/off switch for the leader nudge.
    /// Absent ⇒ `teams::messages::EscalationRule::default()` at the boot site
    /// (byte-identical prior behaviour). The third teams storm/escalation guard
    /// alongside `team_dispatcher` (§4.4) and `team_broadcast` (§4.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_messages: Option<TeamMessagesConfigToml>,
    /// Mid-run trajectory resume — boot-scan auto-resume of interrupted runs.
    #[serde(default)]
    pub resume: crate::config::types::ResumeConfig,
    /// Project-selection filesystem scope — drives the cross-platform
    /// `<DirectoryBrowser />` (Panel) and `fs.*` RPCs. The roots listed here
    /// are the only directories a paired client can traverse / mkdir into.
    #[serde(default)]
    pub projects: crate::config::types::ProjectsConfig,
    /// Desktop daemon-consumer settings (FEATURE_LOCATOR §7.6) — presence
    /// broadcaster + mic-level meter. **Both default to off.** This doc used to
    /// say "presence on @30s"; the code, its own default, and its test have
    /// always said `false`, and `PresenceConfig`'s doc explains why (the
    /// snapshot carries the host's name and the OS username).
    #[serde(default)]
    pub desktop: crate::config::types::desktop::DesktopDaemonConfig,
    /// Presets override loaded from ~/.aleph/presets.toml
    /// Not serialized to config.toml — lives in its own file
    #[serde(skip)]
    pub presets_override: crate::config::presets_override::PresetsOverride,
    /// Defaults override loaded from ~/.aleph/defaults.toml
    /// Not serialized to config.toml — lives in its own file
    /// Must be loaded BEFORE config.toml parsing so serde default functions can read it
    #[serde(skip)]
    pub defaults_override: crate::config::defaults_override::DefaultsOverride,
}

// =============================================================================
// ChannelInstanceConfig
// =============================================================================

/// A resolved channel instance from the channels config `HashMap`.
#[derive(Debug, Clone)]
pub struct ChannelInstanceConfig {
    /// Instance identifier (the `HashMap` key)
    pub id: String,
    /// Channel platform type (e.g. "telegram", "discord")
    pub channel_type: String,
    /// Remaining config with `type` field stripped
    pub config: serde_json::Value,
}

impl Config {
    /// Parse the `channels` `HashMap` into resolved channel instances.
    ///
    /// Type resolution rules:
    /// 1. If value has a `type` string field -> use it as `channel_type`
    /// 2. If no `type` field and key is a known platform name -> infer type = key
    /// 3. Otherwise -> warn and skip
    ///
    /// Step 2 reads [`aleph_protocol::channels::CONFIGURABLE_CHANNEL_TYPES`]
    /// rather than a local list. It used to be a private `KNOWN_CHANNEL_TYPES`
    /// copy, deleted 2026-08-18 — it was the *third* spelling of "which
    /// channels exist" (after the factory table and the Panel's `ALL_CHANNELS`)
    /// and it had drifted: `line` and `wechat` register factories and are fully
    /// configurable, but were absent here, so `[channels.line]` was skipped
    /// unless the user also wrote `type = "line"`. The skip warns at `warn!`,
    /// which lands in the file log and never on stdout, so from the console it
    /// looked like the section had simply been ignored.
    pub fn resolved_channels(&self) -> Vec<ChannelInstanceConfig> {
        let mut instances = Vec::new();
        for (key, value) in &self.channels {
            let channel_type = if let Some(t) = value.get("type").and_then(|v| v.as_str()) {
                t.to_string()
            } else if aleph_protocol::channels::CONFIGURABLE_CHANNEL_TYPES.contains(&key.as_str()) {
                key.clone()
            } else {
                tracing::warn!(
                    "Channel '{}' has no 'type' field and is not a known platform name, skipping",
                    key
                );
                continue;
            };

            let config = match value {
                serde_json::Value::Object(map) => {
                    let mut map = map.clone();
                    map.remove("type");
                    serde_json::Value::Object(map)
                }
                other => other.clone(),
            };

            instances.push(ChannelInstanceConfig {
                id: key.clone(),
                channel_type,
                config,
            });
        }
        instances.sort_by(|a, b| a.id.cmp(&b.id));
        instances
    }
}

// =============================================================================
// Config Default
// =============================================================================

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            memory: MemoryConfig::default(),
            providers: HashMap::new(),
            // AI-first: no builtin rules, user defines custom rules in config.toml
            rules: vec![],
            behavior: Some(BehaviorConfig::default()),
            search: None,
            fetch: None,
            tools: ToolsConfig::default(),
            mcp: McpConfig::default(),
            unified_tools: None, // Use legacy tools + mcp by default for backward compatibility
            tool_service: ToolServiceConfig::default(),
            sandbox: crate::sandbox::SandboxConfig::default(),
            policies: PoliciesConfig::default(),
            generation: GenerationConfig::default(),
            voice_local: VoiceSection::default(),
            route: crate::config::types::ModelRouteConfig::default(),
            group_chat: GroupChatConfig::default(),
            cron: CronConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            tasks_reaper: ReaperConfig::default(),
            personas: Vec::new(),
            privacy: PrivacyConfig::default(),
            security: ShellSecurityConfig::default(),
            ssrf: crate::security::ssrf::SsrfPolicy::default(),
            profiles: HashMap::new(),
            secret_providers: HashMap::new(),
            secrets_config: SecretsConfig::default(),
            prompt: PromptSectionConfig::default(),
            channels: HashMap::new(),
            a2a: crate::a2a::config::A2AConfig::default(),
            acp: AcpConfig::default(),
            execution: ExecutionConfig::default(),
            agents: AgentsConfig::default(),
            bindings: Vec::new(),
            session: crate::routing::config::SessionConfig::default(),
            plugin_marketplaces: HashMap::new(),
            stop_hooks: Vec::new(),
            guardrails: None,
            stability: None,
            fallback_provider: None,
            context_budget: None,
            strategy: None,
            moa: None,
            team_dispatcher: None,
            team_broadcast: None,
            team_messages: None,
            resume: crate::config::types::ResumeConfig::default(),
            projects: crate::config::types::ProjectsConfig::default(),
            desktop: crate::config::types::desktop::DesktopDaemonConfig::default(),
            presets_override: crate::config::presets_override::PresetsOverride::default(),
            defaults_override: crate::config::defaults_override::DefaultsOverride::default(),
        }
    }
}

// =============================================================================
// Config Basic Methods
// =============================================================================

impl Config {
    /// Create a new config with default values
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience accessor for the local voice (BYO endpoint) config.
    #[must_use]
    pub fn local_voice(&self) -> &VoiceLocalConfig {
        &self.voice_local.local
    }

    /// One-time fold of the legacy `[policies.web_fetch.crawl4ai]` backend into
    /// the new `[fetch]` section. No-op when `[fetch]` is already present (new
    /// config wins) or the legacy backend is unconfigured. The legacy vault key
    /// `web_fetch:crawl4ai` is still read by the fetch registry as a fallback,
    /// so secrets survive without rewrite.
    pub fn migrate_fetch(&mut self) {
        if self.fetch.is_some() {
            return;
        }
        let c4 = &self.policies.web_fetch.crawl4ai;
        if c4.base_url.is_empty() && !c4.enabled {
            return;
        }
        let mut backends = std::collections::HashMap::new();
        backends.insert(
            "crawl4ai".to_string(),
            crate::config::types::FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: None,
                base_url: (!c4.base_url.is_empty()).then(|| c4.base_url.clone()),
                timeout_seconds: Some(c4.timeout_seconds),
                verified: false,
            },
        );
        self.fetch = Some(crate::config::types::FetchConfigInternal {
            enabled: c4.enabled,
            default_provider: "crawl4ai".into(),
            fallback_providers: None,
            backends,
        });
    }
}

#[cfg(test)]
mod session_block_tests {
    use super::Config;
    use crate::routing::session_key::DmScope;

    #[test]
    fn session_block_parses_main() {
        let toml_str = r#"
            [session]
            dm_scope = "main"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.session.dm_scope, DmScope::Main);
    }

    #[test]
    fn session_block_defaults_to_per_peer_when_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.session.dm_scope, DmScope::PerPeer);
    }

    #[test]
    fn default_config_omits_session_block_when_serialized() {
        let cfg = Config::default();
        let toml_str = toml::to_string(&cfg).unwrap();
        assert!(
            !toml_str.contains("[session]"),
            "default config should not emit a [session] block, got:\n{toml_str}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_crawl4ai_migrates_into_fetch_section() {
        let mut cfg = Config::default();
        cfg.policies.web_fetch.crawl4ai.enabled = true;
        cfg.policies.web_fetch.crawl4ai.base_url = "http://10.10.10.3:11235".into();
        cfg.policies.web_fetch.crawl4ai.timeout_seconds = 60;
        assert!(cfg.fetch.is_none());

        cfg.migrate_fetch();

        let f = cfg.fetch.expect("fetch populated");
        assert!(f.enabled);
        assert_eq!(f.default_provider, "crawl4ai");
        let b = &f.backends["crawl4ai"];
        assert_eq!(b.provider_type, "crawl4ai");
        assert_eq!(b.base_url.as_deref(), Some("http://10.10.10.3:11235"));
        assert_eq!(b.timeout_seconds, Some(60));
    }

    #[test]
    fn migrate_is_noop_when_fetch_already_present() {
        let mut cfg = Config {
            fetch: Some(crate::config::types::FetchConfigInternal::default()),
            ..Config::default()
        };
        cfg.policies.web_fetch.crawl4ai.enabled = true;
        cfg.migrate_fetch();
        assert!(
            cfg.fetch.as_ref().unwrap().backends.is_empty(),
            "existing [fetch] wins"
        );
    }
}
