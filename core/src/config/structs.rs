//! Configuration structures
//!
//! This module defines the core configuration structures for Aleph.

use crate::config::types::*;
use crate::cron::CronConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Config
// =============================================================================

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// Legacy hotkey field (deprecated, use trigger.replace_hotkey/append_hotkey instead)
    /// Kept for backward compatibility with old config files
    #[serde(default = "crate::config::types::general::default_hotkey")]
    pub default_hotkey: String,
    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,
    /// Memory module configuration
    #[serde(default)]
    pub memory: MemoryConfig,
    /// AI provider configurations (Phase 5)
    /// Note: Not exposed through UniFFI dictionary, managed via separate methods
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, ProviderConfig>,
    /// Routing rules for smart AI provider selection (Phase 5)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RoutingRuleConfig>,
    /// Shortcuts configuration (Phase 6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcuts: Option<ShortcutsConfig>,
    /// Behavior configuration (Phase 6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<BehaviorConfig>,
    /// Search configuration (Search Capability Integration)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchConfigInternal>,
    /// Video transcript configuration (YouTube transcript extraction)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoConfig>,
    /// Skills configuration (Claude Agent Skills standard)
    #[serde(default)]
    pub skills: SkillsConfig,
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
    /// Smart conversation flow configuration
    #[serde(default)]
    pub smart_flow: SmartFlowConfig,
    /// Smart matching configuration (semantic detection system)
    #[serde(default)]
    pub smart_matching: SmartMatchingConfig,
    /// Dispatcher Layer configuration (intelligent tool routing)
    #[serde(default)]
    pub dispatcher: DispatcherConfigToml,
    /// Agent task orchestration configuration (renamed from cowork)
    /// Supports both [agent] and [cowork] sections for backward compatibility
    #[serde(default, alias = "cowork")]
    pub agent: CoworkConfigToml,
    /// Policies configuration (mechanism-policy separation)
    /// Contains configurable behavioral parameters extracted from mechanism code
    #[serde(default)]
    pub policies: PoliciesConfig,
    /// Generation providers configuration (image, speech, audio, video)
    #[serde(default)]
    pub generation: GenerationConfig,
    /// Orchestrator configuration (Three-Layer Control architecture)
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    /// Sub-agent synchronization configuration
    #[serde(default)]
    pub subagent: SubAgentConfig,
    /// Task routing decision layer configuration
    #[serde(default)]
    pub task_routing: TaskRoutingConfig,
    /// Group chat configuration (multi-agent persona orchestration)
    #[serde(default)]
    pub group_chat: GroupChatConfig,
    /// Cron job scheduling configuration
    #[serde(default)]
    pub cron: CronConfig,
    /// Preset persona definitions for group chat
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personas: Vec<PersonaConfig>,
    /// Skill evolution configuration (Skill Compiler - Phase 10)
    #[serde(default)]
    pub evolution: EvolutionConfig,
    /// Privacy and PII filtering configuration
    #[serde(default)]
    pub privacy: PrivacyConfig,
    /// Workspace profiles configuration (Anti-Gravity Architecture)
    /// Profiles define the "Physics" of workspaces: model binding, tool whitelist, system prompt
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, ProfileConfig>,
    /// Secret provider backends (e.g., local_vault, 1password, bitwarden)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub secret_providers: HashMap<String, SecretProviderConfig>,
    /// Logical secret name -> provider mapping
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub secrets: HashMap<String, SecretMapping>,
    /// Top-level secrets subsystem settings
    #[serde(default)]
    pub secrets_config: SecretsConfig,
    /// Prompt customization (extra files injection, etc.)
    #[serde(default)]
    pub prompt: PromptSectionConfig,
    /// Channel configurations (runtime channel control)
    /// Each key is a channel name (e.g. "telegram", "discord"), value is channel-specific config.
    /// This uses opaque JSON values since each channel has a different schema.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub channels: HashMap<String, serde_json::Value>,
    /// Agent definitions for multi-agent configuration
    /// Defines available agents, their workspaces, profiles, and capabilities
    #[serde(default)]
    pub agents: AgentsConfig,
    /// Channel → Agent routing bindings
    /// Maps channel/peer patterns to specific agents using RouteBinding
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<crate::routing::config::RouteBinding>,
    /// Presets override loaded from ~/.aleph/presets.toml
    /// Not serialized to config.toml — lives in its own file
    #[serde(skip)]
    pub presets_override: crate::config::presets_override::PresetsOverride,
    /// Prompts override loaded from ~/.aleph/prompts.toml
    /// Not serialized to config.toml — lives in its own file
    #[serde(skip)]
    pub prompts_override: crate::config::prompts_override::PromptsOverride,
    /// Defaults override loaded from ~/.aleph/defaults.toml
    /// Not serialized to config.toml — lives in its own file
    /// Must be loaded BEFORE config.toml parsing so serde default functions can read it
    #[serde(skip)]
    pub defaults_override: crate::config::defaults_override::DefaultsOverride,
}

// =============================================================================
// ChannelInstanceConfig
// =============================================================================

/// Known platform names for type auto-inference from channel config keys.
const KNOWN_CHANNEL_TYPES: &[&str] = &[
    "telegram", "discord", "whatsapp", "slack", "imessage",
    "email", "matrix", "signal", "mattermost", "irc",
    "webhook", "xmpp", "nostr",
];

/// A resolved channel instance from the channels config HashMap.
#[derive(Debug, Clone)]
pub struct ChannelInstanceConfig {
    /// Instance identifier (the HashMap key)
    pub id: String,
    /// Channel platform type (e.g. "telegram", "discord")
    pub channel_type: String,
    /// Remaining config with `type` field stripped
    pub config: serde_json::Value,
}

impl Config {
    /// Parse the `channels` HashMap into resolved channel instances.
    ///
    /// Type resolution rules:
    /// 1. If value has a `type` string field -> use it as channel_type
    /// 2. If no `type` field and key is a known platform name -> infer type = key
    /// 3. Otherwise -> warn and skip
    pub fn resolved_channels(&self) -> Vec<ChannelInstanceConfig> {
        let mut instances = Vec::new();
        for (key, value) in &self.channels {
            let channel_type = if let Some(t) = value.get("type").and_then(|v| v.as_str()) {
                t.to_string()
            } else if KNOWN_CHANNEL_TYPES.contains(&key.as_str()) {
                key.clone()
            } else {
                tracing::warn!(
                    "Channel '{}' has no 'type' field and is not a known platform name, skipping",
                    key
                );
                continue;
            };

            let config = if let serde_json::Value::Object(mut map) = value.clone() {
                map.remove("type");
                serde_json::Value::Object(map)
            } else {
                value.clone()
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
// FullConfig (UniFFI)
// =============================================================================

/// Full configuration exposed through UniFFI
/// This wraps Config with a flattened provider list
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FullConfig {
    pub default_hotkey: String,
    pub general: GeneralConfig,
    pub memory: MemoryConfig,
    pub providers: Vec<ProviderConfigEntry>,
    pub rules: Vec<RoutingRuleConfig>,
    #[serde(default)]
    pub shortcuts: Option<ShortcutsConfig>,
    #[serde(default)]
    pub behavior: Option<BehaviorConfig>,
    #[serde(default)]
    pub search: Option<SearchConfig>,
    #[serde(default)]
    pub smart_matching: SmartMatchingConfig,
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
    #[serde(default)]
    pub policies: PoliciesConfig,
}

impl From<Config> for FullConfig {
    fn from(config: Config) -> Self {
        let providers = config
            .providers
            .into_iter()
            .map(|(name, config)| ProviderConfigEntry { name, config })
            .collect();

        let search = config.search.map(|s| s.into());

        Self {
            default_hotkey: config.default_hotkey,
            general: config.general,
            memory: config.memory,
            providers,
            rules: config.rules,
            shortcuts: config.shortcuts,
            behavior: config.behavior,
            search,
            smart_matching: config.smart_matching,
            skills: Some(config.skills),
            policies: config.policies,
        }
    }
}

// =============================================================================
// Config Default
// =============================================================================

impl Default for Config {
    fn default() -> Self {
        Self {
            default_hotkey: "Grave".to_string(), // Legacy field, kept for backward compatibility
            general: GeneralConfig::default(),
            memory: MemoryConfig::default(),
            providers: HashMap::new(),
            // AI-first: no builtin rules, user defines custom rules in config.toml
            rules: vec![],
            shortcuts: Some(ShortcutsConfig::default()),
            behavior: Some(BehaviorConfig::default()),
            search: None,
            video: Some(VideoConfig::default()),
            skills: SkillsConfig::default(),
            tools: ToolsConfig::default(),
            mcp: McpConfig::default(),
            unified_tools: None, // Use legacy tools + mcp by default for backward compatibility
            smart_flow: SmartFlowConfig::default(),
            smart_matching: SmartMatchingConfig::default(),
            dispatcher: DispatcherConfigToml::default(),
            agent: CoworkConfigToml::default(),
            policies: PoliciesConfig::default(),
            generation: GenerationConfig::default(),
            orchestrator: OrchestratorConfig::default(),
            subagent: SubAgentConfig::default(),
            task_routing: TaskRoutingConfig::default(),
            group_chat: GroupChatConfig::default(),
            cron: CronConfig::default(),
            personas: Vec::new(),
            evolution: EvolutionConfig::default(),
            privacy: PrivacyConfig::default(),
            profiles: HashMap::new(),
            secret_providers: HashMap::new(),
            secrets: HashMap::new(),
            secrets_config: SecretsConfig::default(),
            prompt: PromptSectionConfig::default(),
            channels: HashMap::new(),
            agents: AgentsConfig::default(),
            bindings: Vec::new(),
            presets_override: crate::config::presets_override::PresetsOverride::default(),
            prompts_override: crate::config::prompts_override::PromptsOverride::default(),
            defaults_override: crate::config::defaults_override::DefaultsOverride::default(),
        }
    }
}

// =============================================================================
// Config Basic Methods
// =============================================================================

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }
}
