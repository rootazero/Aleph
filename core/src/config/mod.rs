//! Configuration module for Aether
//!
//! This module provides the configuration system for Aether, including:
//! - `Config`: The main configuration struct with load/save/validate methods
//! - `FullConfig`: FFI-compatible version for UniFFI
//! - Type definitions in the `types` submodule
//!
//! Phase 1: Stub implementation with basic fields.
//! Phase 4: Added memory configuration support.
//! Phase 5: Added AI provider configuration support.
//! Phase 6: Added Keychain integration and file watching support.
//! Phase 8: Added config file loading from ~/.config/aether/config.toml

use crate::error::{AetherError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

// Submodules
pub mod types;
pub mod watcher;

// Re-export types for backward compatibility
pub use types::*;

#[allow(unused_imports)]
pub use watcher::ConfigWatcher;

// =============================================================================
// Config
// =============================================================================

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Legacy hotkey field (deprecated, use trigger.replace_hotkey/append_hotkey instead)
    /// Kept for backward compatibility with old config files
    #[serde(default = "types::general::default_hotkey")]
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
    /// Trigger configuration (hotkey system refactor)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerConfig>,
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
    /// Typo correction configuration (quick double-space correction)
    #[serde(default)]
    pub typo_correction: TypoCorrectionConfig,
}

// =============================================================================
// FullConfig (UniFFI)
// =============================================================================

/// Full configuration exposed through UniFFI
/// This wraps Config with a flattened provider list
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub trigger: Option<TriggerConfig>,
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
            trigger: config.trigger,
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
            trigger: Some(TriggerConfig::default()),
            smart_flow: SmartFlowConfig::default(),
            smart_matching: SmartMatchingConfig::default(),
            dispatcher: DispatcherConfigToml::default(),
            agent: CoworkConfigToml::default(),
            policies: PoliciesConfig::default(),
            generation: GenerationConfig::default(),
            orchestrator: OrchestratorConfig::default(),
            typo_correction: TypoCorrectionConfig::default(),
        }
    }
}

// =============================================================================
// Config Implementation
// =============================================================================

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Get effective tools configuration (unified format)
    ///
    /// This method provides a unified view of tools configuration:
    /// - If `unified_tools` is present, it takes precedence
    /// - Otherwise, creates unified config from legacy `tools` + `mcp` sections
    ///
    /// This enables gradual migration from legacy config format to unified format.
    pub fn get_effective_tools_config(&self) -> UnifiedToolsConfig {
        if let Some(unified) = &self.unified_tools {
            unified.clone()
        } else {
            UnifiedToolsConfig::from_legacy(&self.tools, &self.mcp)
        }
    }

    /// Check if using new unified tools configuration
    pub fn is_using_unified_tools(&self) -> bool {
        self.unified_tools.is_some()
    }

    /// Get the default config path using unified directory
    ///
    /// Returns unified path for all platforms:
    /// - All platforms: ~/.config/aether/config.toml
    pub fn default_path() -> PathBuf {
        crate::utils::paths::get_config_dir()
            .map(|d| d.join("config.toml"))
            .unwrap_or_else(|_| PathBuf::from("config.toml"))
    }

    /// Load configuration from a TOML file
    ///
    /// # Arguments
    /// * `path` - Path to the config file
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config
    /// * `Err(AetherError::ConfigNotFound)` - File doesn't exist
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load_from_file("config.toml").unwrap();
    /// ```
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        debug!(path = %path.display(), "Attempting to load config from file");

        // Check if file exists
        if !path.exists() {
            error!(path = %path.display(), "Config file not found");
            return Err(AetherError::invalid_config(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        // Read file contents
        let contents = fs::read_to_string(path).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to read config file");
            AetherError::invalid_config(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        debug!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config file read successfully, parsing TOML"
        );

        // Pre-process TOML: Migrate [mcp.builtin] to [tools] if needed
        let contents = Self::migrate_mcp_builtin_in_toml(&contents)?;

        // Parse TOML
        let mut config: Config = toml::from_str(&contents).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to parse config TOML");
            AetherError::invalid_config(format!(
                "Failed to parse config file {}: {}",
                path.display(),
                e
            ))
        })?;

        debug!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            "Config parsed successfully, merging builtin rules"
        );

        // Merge builtin rules with user rules
        // Builtin rules (/search, /mcp, /skill) should be prepended to user rules
        // unless user has defined a rule with the same regex pattern
        config.merge_builtin_rules();

        debug!(
            path = %path.display(),
            rules_count = config.rules.len(),
            "Builtin rules merged, checking for migrations"
        );

        // Migrate PII config from behavior to search (integrate-search-registry)
        let pii_migrated = config.migrate_pii_config();
        if pii_migrated {
            info!("Migrated PII config from behavior.pii_scrubbing_enabled to search.pii.enabled");
        }

        // Migrate input_mode to trigger config (hotkey-system-refactor)
        let trigger_migrated = config.migrate_trigger_config();
        if trigger_migrated {
            info!("Migrated input_mode config to new trigger config");
        }

        // Auto-save if any migration was performed
        // IMPORTANT: Use incremental save to preserve user's existing config
        // This only updates the migrated sections without overwriting providers, rules, etc.
        if pii_migrated || trigger_migrated {
            let mut sections_to_save: Vec<&str> = Vec::new();

            if pii_migrated {
                sections_to_save.push("search");
                sections_to_save.push("behavior");
            }
            if trigger_migrated {
                sections_to_save.push("trigger");
            }

            if let Err(e) = config.save_incremental(&sections_to_save) {
                warn!(error = %e, "Failed to auto-save migrated config (incremental)");
                // Don't fall back to full save - that would overwrite user config
            } else {
                debug!(
                    sections = ?sections_to_save,
                    "Migration saved incrementally"
                );
            }
        }

        // Validate config
        config.validate()?;

        info!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            memory_enabled = config.memory.enabled,
            "Config loaded and validated successfully"
        );

        Ok(config)
    }

    /// Load configuration from default path (~/.config/aether/config.toml)
    /// Falls back to default config if file doesn't exist
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config or default config
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```rust,ignore
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load().unwrap();
    /// ```
    pub fn load() -> Result<Self> {
        let path = Self::default_path();

        debug!(path = %path.display(), "Loading config from default path");

        if path.exists() {
            info!(path = %path.display(), "Found config file, loading");
            Self::load_from_file(&path)
        } else {
            info!(
                path = %path.display(),
                "Config file not found, using default configuration"
            );
            Ok(Self::default())
        }
    }

    /// Process user-defined routing rules (AI-first architecture)
    ///
    /// In AI-first mode, there are no builtin rules. This method is kept
    /// for backward compatibility but does minimal processing.
    fn merge_builtin_rules(&mut self) {
        // AI-first: no builtin rules to merge, just log user rules count
        debug!(
            user_rules_count = self.rules.len(),
            "Processing user-defined routing rules (AI-first mode)"
        );
    }

    /// Validate configuration
    ///
    /// Checks:
    /// - Provider references in rules exist in providers map
    /// - Default provider exists (if specified)
    /// - API keys are present for cloud providers
    /// - Regex patterns are valid
    pub fn validate(&self) -> Result<()> {
        debug!(
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Starting config validation"
        );

        // Warn if no default provider is configured
        if self.general.default_provider.is_none() {
            warn!(
                "No default_provider configured. \
                 Requests will fail if no routing rule matches. \
                 Recommendation: Set general.default_provider in config"
            );
        }

        // Warn if no routing rules are configured
        if self.rules.is_empty() {
            warn!(
                "No routing rules configured. \
                 All requests will use default_provider (if set). \
                 Recommendation: Add routing rules to enable context-aware routing"
            );
        }

        // Validate default provider exists (if configured)
        if let Some(ref default_provider) = self.general.default_provider {
            if !self.providers.contains_key(default_provider) {
                error!(default_provider = %default_provider, "Default provider not found");
                return Err(AetherError::invalid_config(format!(
                    "Default provider '{}' not found in providers",
                    default_provider
                )));
            }
            debug!(default_provider = %default_provider, "Default provider validated");
        }

        // Validate provider configurations
        for (name, provider) in &self.providers {
            let provider_type = provider.infer_provider_type(name);

            // Check API key for cloud providers (not required for Ollama)
            if (provider_type == "openai" || provider_type == "claude" || provider_type == "gemini")
                && provider.api_key.is_none()
            {
                error!(provider = %name, provider_type = %provider_type, "Provider missing API key");
                return Err(AetherError::invalid_config(format!(
                    "Provider '{}' requires an API key",
                    name
                )));
            }

            // Validate timeout
            if provider.timeout_seconds == 0 {
                error!(provider = %name, "Provider timeout is zero");
                return Err(AetherError::invalid_config(format!(
                    "Provider '{}' timeout must be greater than 0",
                    name
                )));
            }

            // Validate temperature if specified (provider-specific ranges)
            if let Some(temp) = provider.temperature {
                let (min, max, provider_name): (f32, f32, &str) = match provider_type.as_str() {
                    "claude" => (0.0, 1.0, "Claude"),
                    "openai" => (0.0, 2.0, "OpenAI"),
                    "gemini" => (0.0, 2.0, "Gemini"),
                    "ollama" => (0.0, f32::MAX, "Ollama"),
                    _ => (0.0, 2.0, "Custom"),
                };

                if !(min..=max).contains(&temp) {
                    error!(provider = %name, temperature = temp, "Invalid temperature for {}", provider_name);
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' ({}) temperature must be between {} and {}, got {}",
                        name, provider_name, min, max, temp
                    )));
                }
            }

            // Validate max_tokens if specified
            if let Some(max_tokens) = provider.max_tokens {
                if max_tokens == 0 {
                    error!(provider = %name, max_tokens = max_tokens, "Invalid max_tokens");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' max_tokens must be greater than 0, got {}",
                        name, max_tokens
                    )));
                }
            }

            // Validate top_p if specified
            if let Some(top_p) = provider.top_p {
                if !(0.0..=1.0).contains(&top_p) {
                    error!(provider = %name, top_p = top_p, "Invalid top_p");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' top_p must be between 0.0 and 1.0, got {}",
                        name, top_p
                    )));
                }
            }

            // Validate top_k if specified
            if let Some(top_k) = provider.top_k {
                if top_k == 0 {
                    error!(provider = %name, top_k = top_k, "Invalid top_k");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' top_k must be greater than 0, got {}",
                        name, top_k
                    )));
                }
            }

            // Validate OpenAI-specific parameters
            if provider_type == "openai" {
                if let Some(freq_pen) = provider.frequency_penalty {
                    if !(-2.0..=2.0).contains(&freq_pen) {
                        error!(provider = %name, frequency_penalty = freq_pen, "Invalid frequency_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' frequency_penalty must be between -2.0 and 2.0, got {}",
                            name, freq_pen
                        )));
                    }
                }

                if let Some(pres_pen) = provider.presence_penalty {
                    if !(-2.0..=2.0).contains(&pres_pen) {
                        error!(provider = %name, presence_penalty = pres_pen, "Invalid presence_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' presence_penalty must be between -2.0 and 2.0, got {}",
                            name, pres_pen
                        )));
                    }
                }
            }

            // Validate Gemini-specific parameters
            if provider_type == "gemini" {
                if let Some(ref thinking_level) = provider.thinking_level {
                    if thinking_level != "LOW" && thinking_level != "HIGH" {
                        error!(provider = %name, thinking_level = %thinking_level, "Invalid thinking_level");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' thinking_level must be 'LOW' or 'HIGH', got '{}'",
                            name, thinking_level
                        )));
                    }
                }

                if let Some(ref media_res) = provider.media_resolution {
                    if media_res != "LOW" && media_res != "MEDIUM" && media_res != "HIGH" {
                        error!(provider = %name, media_resolution = %media_res, "Invalid media_resolution");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' media_resolution must be 'LOW', 'MEDIUM', or 'HIGH', got '{}'",
                            name, media_res
                        )));
                    }
                }
            }

            // Validate Ollama-specific parameters
            if provider_type == "ollama" {
                if let Some(repeat_pen) = provider.repeat_penalty {
                    if repeat_pen < 0.0 {
                        error!(provider = %name, repeat_penalty = repeat_pen, "Invalid repeat_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' repeat_penalty must be >= 0.0, got {}",
                            name, repeat_pen
                        )));
                    }
                }
            }

            debug!(
                provider = %name,
                provider_type = %provider_type,
                timeout_seconds = provider.timeout_seconds,
                "Provider validated"
            );
        }

        // Validate routing rules
        for (idx, rule) in self.rules.iter().enumerate() {
            let rule_type = rule.get_rule_type();

            // Command rules require a provider (skip for builtin rules which use default_provider)
            if rule.is_command_rule() && !rule.is_builtin {
                match &rule.provider {
                    Some(provider) => {
                        if !self.providers.contains_key(provider) {
                            error!(
                                rule_index = idx + 1,
                                provider = %provider,
                                "Command rule references unknown provider"
                            );
                            return Err(AetherError::invalid_config(format!(
                                "Command rule #{} references unknown provider '{}'",
                                idx + 1,
                                provider
                            )));
                        }
                    }
                    None => {
                        error!(
                            rule_index = idx + 1,
                            regex = %rule.regex,
                            "Command rule missing provider"
                        );
                        return Err(AetherError::invalid_config(format!(
                            "Command rule #{} (regex: '{}') requires a provider",
                            idx + 1,
                            rule.regex
                        )));
                    }
                }
            }

            // Keyword rules require a system_prompt
            if rule.is_keyword_rule() && rule.system_prompt.is_none() {
                warn!(
                    rule_index = idx + 1,
                    regex = %rule.regex,
                    "Keyword rule missing system_prompt - rule will have no effect"
                );
            }

            debug!(
                rule_index = idx + 1,
                rule_type = %rule_type,
                regex = %rule.regex,
                is_builtin = rule.is_builtin,
                "Validating rule"
            );

            // Validate regex pattern
            if let Err(e) = regex::Regex::new(&rule.regex) {
                error!(
                    rule_index = idx + 1,
                    regex = %rule.regex,
                    error = %e,
                    "Invalid regex pattern"
                );
                return Err(AetherError::invalid_config(format!(
                    "Rule #{} has invalid regex '{}': {}",
                    idx + 1,
                    rule.regex,
                    e
                )));
            }
        }

        // Validate memory config
        if self.memory.max_context_items == 0 {
            error!("Memory max_context_items is zero");
            return Err(AetherError::invalid_config(
                "memory.max_context_items must be greater than 0",
            ));
        }

        if !(0.0..=1.0).contains(&self.memory.similarity_threshold) {
            error!(
                threshold = self.memory.similarity_threshold,
                "Invalid similarity threshold"
            );
            return Err(AetherError::invalid_config(format!(
                "memory.similarity_threshold must be between 0.0 and 1.0, got {}",
                self.memory.similarity_threshold
            )));
        }

        debug!(
            memory_enabled = self.memory.enabled,
            max_context_items = self.memory.max_context_items,
            similarity_threshold = self.memory.similarity_threshold,
            "Memory config validated"
        );

        // Validate language preference
        if let Some(ref language) = self.general.language {
            // List of supported language codes (must match .lproj directory names)
            let supported_languages = vec!["en", "zh-Hans"];

            if !supported_languages.contains(&language.as_str()) {
                tracing::warn!(
                    language = %language,
                    supported = ?supported_languages,
                    "Invalid language code '{}', falling back to system language. Supported languages: {:?}",
                    language,
                    supported_languages
                );
            } else {
                debug!(language = %language, "Language preference validated");
            }
        }

        // Validate search configuration
        if let Some(ref search_config) = self.search {
            if search_config.enabled {
                // Validate default provider exists
                if !search_config
                    .backends
                    .contains_key(&search_config.default_provider)
                {
                    error!(
                        default_provider = %search_config.default_provider,
                        "Search default provider not found in backends"
                    );
                    return Err(AetherError::invalid_config(format!(
                        "Search default provider '{}' not found in backends",
                        search_config.default_provider
                    )));
                }

                // Validate fallback providers exist
                if let Some(ref fallback_providers) = search_config.fallback_providers {
                    for provider_name in fallback_providers {
                        if !search_config.backends.contains_key(provider_name) {
                            error!(
                                fallback_provider = %provider_name,
                                "Search fallback provider not found in backends"
                            );
                            return Err(AetherError::invalid_config(format!(
                                "Search fallback provider '{}' not found in backends",
                                provider_name
                            )));
                        }
                    }
                }

                // Validate max_results is reasonable
                if search_config.max_results == 0 {
                    error!("Search max_results cannot be 0");
                    return Err(AetherError::invalid_config(
                        "Search max_results must be greater than 0".to_string(),
                    ));
                }

                if search_config.max_results > 100 {
                    warn!(
                        max_results = search_config.max_results,
                        "Search max_results is very high (>100), this may impact performance"
                    );
                }

                // Validate timeout is reasonable
                if search_config.timeout_seconds == 0 {
                    error!("Search timeout cannot be 0");
                    return Err(AetherError::invalid_config(
                        "Search timeout_seconds must be greater than 0".to_string(),
                    ));
                }

                // Validate each backend configuration
                for (backend_name, backend_config) in &search_config.backends {
                    let provider_type = backend_config.provider_type.as_str();

                    match provider_type {
                        "tavily" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Tavily backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Tavily) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "brave" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Brave backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Brave) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "google" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Google backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Google) requires an API key",
                                    backend_name
                                )));
                            }
                            if backend_config.engine_id.is_none() {
                                error!(backend = %backend_name, "Google backend requires engine_id");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Google) requires an engine_id",
                                    backend_name
                                )));
                            }
                        }
                        "bing" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Bing backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Bing) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "exa" => {
                            if backend_config.api_key.is_none() {
                                error!(backend = %backend_name, "Exa backend requires API key");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (Exa) requires an API key",
                                    backend_name
                                )));
                            }
                        }
                        "searxng" => {
                            if backend_config.base_url.is_none() {
                                error!(backend = %backend_name, "SearXNG backend requires base_url");
                                return Err(AetherError::invalid_config(format!(
                                    "Search backend '{}' (SearXNG) requires a base_url",
                                    backend_name
                                )));
                            }
                        }
                        _ => {
                            warn!(
                                backend = %backend_name,
                                provider_type = %provider_type,
                                "Unknown search provider type"
                            );
                        }
                    }

                    debug!(
                        backend = %backend_name,
                        provider_type = %provider_type,
                        "Search backend validated"
                    );
                }

                debug!(
                    enabled = search_config.enabled,
                    default_provider = %search_config.default_provider,
                    backends_count = search_config.backends.len(),
                    "Search config validated"
                );
            }
        }

        // Validate Agent config
        if let Err(e) = self.agent.validate() {
            error!(error = %e, "Agent config validation failed");
            return Err(AetherError::invalid_config(e));
        }

        // Validate planner_provider exists if specified
        if let Some(ref provider_name) = self.agent.planner_provider {
            if !self.providers.contains_key(provider_name) {
                error!(
                    provider = %provider_name,
                    "Agent planner_provider not found in providers"
                );
                return Err(AetherError::invalid_config(format!(
                    "Agent planner_provider '{}' not found in providers",
                    provider_name
                )));
            }
        }

        debug!(
            require_confirmation = self.agent.require_confirmation,
            max_parallelism = self.agent.max_parallelism,
            "Agent config validated"
        );

        info!(
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Config validation completed successfully"
        );

        Ok(())
    }

    /// Save configuration to a TOML file with atomic write
    ///
    /// This method uses atomic write operation to prevent corruption:
    /// 1. Write to temporary file (.tmp suffix)
    /// 2. fsync() to ensure data is on disk
    /// 3. Atomic rename to target path
    ///
    /// This ensures that the config file is never in a partially written state,
    /// even if the application crashes or loses power during the write.
    ///
    /// # Arguments
    /// * `path` - Target path for config file
    ///
    /// # Errors
    /// * `AetherError::InvalidConfig` - Failed to serialize or write config
    ///
    /// # Example
    /// ```rust,ignore
    /// let config = Config::default();
    /// config.save_to_file("config.toml")?;
    /// ```
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        debug!(
            path = %path.display(),
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Attempting to save config"
        );

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                error!(directory = %parent.display(), error = %e, "Failed to create config directory");
                AetherError::invalid_config(format!(
                    "Failed to create config directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
            debug!(directory = %parent.display(), "Config directory ensured");
        }

        // Serialize to TOML
        let contents = toml::to_string_pretty(self).map_err(|e| {
            error!(error = %e, "Failed to serialize config to TOML");
            AetherError::invalid_config(format!("Failed to serialize config: {}", e))
        })?;

        debug!(
            size_bytes = contents.len(),
            lines = contents.lines().count(),
            "Config serialized to TOML"
        );

        // Create temporary file in the same directory (atomic rename requirement)
        let temp_path = path.with_extension("tmp");

        // Write to temp file
        fs::write(&temp_path, &contents).map_err(|e| {
            error!(temp_path = %temp_path.display(), error = %e, "Failed to write temp file");
            AetherError::invalid_config(format!(
                "Failed to write temp config file {}: {}",
                temp_path.display(),
                e
            ))
        })?;

        debug!(temp_path = %temp_path.display(), "Wrote config to temp file");

        // fsync the temp file to ensure data is on disk
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| {
                    error!(temp_path = %temp_path.display(), error = %e, "Failed to open temp file for fsync");
                    AetherError::invalid_config(format!(
                        "Failed to open temp file for fsync: {}",
                        e
                    ))
                })?;

            // Sync file data and metadata
            file.sync_all().map_err(|e| {
                error!(temp_path = %temp_path.display(), error = %e, "Failed to fsync temp file");
                AetherError::invalid_config(format!("Failed to fsync temp file: {}", e))
            })?;

            debug!(temp_path = %temp_path.display(), "Fsynced temp file to disk");
        }

        // Atomic rename (overwrites target if exists)
        fs::rename(&temp_path, path).map_err(|e| {
            error!(
                temp_path = %temp_path.display(),
                target_path = %path.display(),
                error = %e,
                "Failed to atomically rename temp file"
            );
            // Clean up temp file on error
            let _ = fs::remove_file(&temp_path);
            AetherError::invalid_config(format!(
                "Failed to rename temp config to {}: {}",
                path.display(),
                e
            ))
        })?;

        // Set file permissions to 600 (owner read/write only) for security
        // This protects API keys stored in the config file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)
                .map_err(|e| {
                    error!(path = %path.display(), error = %e, "Failed to get file metadata");
                    AetherError::invalid_config(format!("Failed to get file metadata: {}", e))
                })?
                .permissions();
            perms.set_mode(0o600); // Owner read/write only
            fs::set_permissions(path, perms).map_err(|e| {
                error!(path = %path.display(), error = %e, "Failed to set file permissions to 600");
                AetherError::invalid_config(format!("Failed to set file permissions: {}", e))
            })?;
            debug!(path = %path.display(), "Set file permissions to 600 (owner read/write only)");
        }

        info!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config saved successfully with atomic write"
        );

        Ok(())
    }

    /// Save configuration to default path with atomic write
    ///
    /// This is a convenience method that saves to ~/.config/aether/config.toml
    /// using atomic write operation.
    ///
    /// # Example
    /// ```rust,ignore
    /// let mut config = Config::default();
    /// config.default_hotkey = "Command+Shift+A".to_string();
    /// config.save()?;
    /// ```
    pub fn save(&self) -> Result<()> {
        self.save_to_file(Self::default_path())
    }

    /// Save only specific sections to the config file (incremental update)
    ///
    /// This method preserves existing user configuration and only adds/updates
    /// the specified sections. This is used for migrations to avoid overwriting
    /// user's custom settings like providers and rules.
    ///
    /// # Arguments
    /// * `sections` - List of section names to update (e.g., ["trigger", "search.pii"])
    ///
    /// # How it works
    /// 1. Read existing TOML file as raw toml::Value
    /// 2. Serialize current Config to toml::Value
    /// 3. Only copy specified sections from current to existing
    /// 4. Write back with atomic operation
    pub fn save_incremental(&self, sections: &[&str]) -> Result<()> {
        let path = Self::default_path();

        // If file doesn't exist, do a full save
        if !path.exists() {
            return self.save_to_file(&path);
        }

        debug!(
            sections = ?sections,
            "Performing incremental config save"
        );

        // Read existing file
        let existing_contents = fs::read_to_string(&path).map_err(|e| {
            AetherError::invalid_config(format!(
                "Failed to read config for incremental save: {}",
                e
            ))
        })?;

        // Parse existing as toml::Value
        let mut existing: toml::Value = toml::from_str(&existing_contents).map_err(|e| {
            AetherError::invalid_config(format!("Failed to parse existing config: {}", e))
        })?;

        // Serialize current config to toml::Value
        let current: toml::Value = toml::Value::try_from(self).map_err(|e| {
            AetherError::invalid_config(format!("Failed to serialize current config: {}", e))
        })?;

        // Only update specified sections
        if let (toml::Value::Table(ref mut existing_table), toml::Value::Table(ref current_table)) =
            (&mut existing, &current)
        {
            for section in sections {
                // Handle nested sections like "search.pii"
                let parts: Vec<&str> = section.split('.').collect();

                if parts.len() == 1 {
                    // Top-level section
                    if let Some(value) = current_table.get(parts[0]) {
                        existing_table.insert(parts[0].to_string(), value.clone());
                        debug!(section = %section, "Updated top-level section");
                    }
                } else if parts.len() == 2 {
                    // Nested section (e.g., "search.pii")
                    let parent_key = parts[0];
                    let child_key = parts[1];

                    // Get or create parent table in existing
                    let parent_table = existing_table
                        .entry(parent_key.to_string())
                        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));

                    if let toml::Value::Table(ref mut parent) = parent_table {
                        // Get value from current config
                        #[allow(clippy::collapsible_match)]
                        if let Some(current_parent) = current_table.get(parent_key) {
                            if let toml::Value::Table(ref cp) = current_parent {
                                if let Some(value) = cp.get(child_key) {
                                    parent.insert(child_key.to_string(), value.clone());
                                    debug!(section = %section, "Updated nested section");
                                }
                            }
                        }
                    }
                }
            }
        }

        // Serialize back to TOML string
        let new_contents = toml::to_string_pretty(&existing).map_err(|e| {
            AetherError::invalid_config(format!("Failed to serialize updated config: {}", e))
        })?;

        // Write with atomic operation (same as save_to_file)
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, &new_contents).map_err(|e| {
            AetherError::invalid_config(format!("Failed to write temp config: {}", e))
        })?;

        // fsync on Unix
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| {
                    AetherError::invalid_config(format!(
                        "Failed to open temp file for fsync: {}",
                        e
                    ))
                })?;
            file.sync_all()
                .map_err(|e| AetherError::invalid_config(format!("Failed to fsync: {}", e)))?;
        }

        // Atomic rename
        fs::rename(&temp_path, &path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            AetherError::invalid_config(format!("Failed to rename temp config: {}", e))
        })?;

        // Set permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(&path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&path, perms);
            }
        }

        info!(
            sections = ?sections,
            "Incremental config save completed"
        );

        Ok(())
    }

    /// Migrate PII config from behavior to search (integrate-search-registry)
    ///
    /// NOTE: This migration is now a no-op as BehaviorConfig has been deprecated
    /// and the pii_scrubbing_enabled field no longer exists. Old config files
    /// with this field will have it silently ignored by serde.
    ///
    /// # Returns
    /// * `false` - Always returns false (migration no longer applicable)
    fn migrate_pii_config(&mut self) -> bool {
        // BehaviorConfig deprecated - pii_scrubbing_enabled field removed
        // Old configs will have the field ignored by serde
        false
    }

    /// Migrate from old config to new trigger config
    ///
    /// Sets default replace/append hotkeys if trigger config doesn't exist.
    ///
    /// Returns true if migration was performed
    fn migrate_trigger_config(&mut self) -> bool {
        // Check if migration is needed
        if self.trigger.is_some() {
            return false;
        }

        debug!("Migrating to new trigger config with default hotkeys");

        // Create trigger config with defaults
        self.trigger = Some(TriggerConfig {
            replace_hotkey: types::general::default_replace_hotkey(),
            append_hotkey: types::general::default_append_hotkey(),
        });

        true
    }

    /// Migrate [mcp.builtin] to [tools] in raw TOML
    ///
    /// This is a pre-parsing migration that handles the rename-builtin-to-system-tools
    /// proposal. If the old [mcp.builtin] section exists but [tools] doesn't,
    /// the old section is copied to [tools].
    ///
    /// # Arguments
    /// * `contents` - Raw TOML string
    ///
    /// # Returns
    /// * Modified TOML string with migration applied
    fn migrate_mcp_builtin_in_toml(contents: &str) -> Result<String> {
        // Parse as raw TOML value
        let mut value: toml::Value = toml::from_str(contents).map_err(|e| {
            AetherError::invalid_config(format!("Failed to parse TOML for migration: {}", e))
        })?;

        // Check if migration is needed
        let needs_migration = {
            let has_mcp_builtin = value
                .get("mcp")
                .and_then(|mcp| mcp.get("builtin"))
                .is_some();
            let has_tools = value.get("tools").is_some();

            has_mcp_builtin && !has_tools
        };

        if !needs_migration {
            return Ok(contents.to_string());
        }

        // Perform migration
        warn!("Migrating deprecated [mcp.builtin] section to [tools]");

        // Extract mcp.builtin
        let builtin = value.get("mcp").and_then(|mcp| mcp.get("builtin")).cloned();

        if let Some(builtin_value) = builtin {
            // Add as [tools]
            if let toml::Value::Table(ref mut table) = value {
                table.insert("tools".to_string(), builtin_value);

                // Remove [mcp.builtin]
                if let Some(toml::Value::Table(ref mut mcp)) = table.get_mut("mcp") {
                    mcp.remove("builtin");
                }
            }

            info!("Successfully migrated [mcp.builtin] to [tools]");
        }

        // Serialize back to TOML
        toml::to_string_pretty(&value).map_err(|e| {
            AetherError::invalid_config(format!("Failed to serialize migrated TOML: {}", e))
        })
    }

    /// Get the default provider if it exists and is enabled
    ///
    /// Returns None if:
    /// - No default provider is configured
    /// - Default provider does not exist in providers map
    /// - Default provider is disabled
    ///
    /// # Returns
    /// * `Some(String)` - The name of the enabled default provider
    /// * `None` - No valid default provider
    pub fn get_default_provider(&self) -> Option<String> {
        self.general.default_provider.as_ref().and_then(|name| {
            self.providers.get(name).and_then(|config| {
                if config.enabled {
                    Some(name.clone())
                } else {
                    None
                }
            })
        })
    }

    /// Set the default provider with validation
    ///
    /// Validates that:
    /// - Provider exists in providers map
    /// - Provider is enabled
    ///
    /// # Arguments
    /// * `name` - The name of the provider to set as default
    ///
    /// # Returns
    /// * `Ok(())` - Successfully set default provider
    /// * `Err(AetherError::InvalidConfig)` - Provider not found or disabled
    pub fn set_default_provider(&mut self, name: &str) -> Result<()> {
        match self.providers.get(name) {
            Some(config) if config.enabled => {
                debug!(provider = %name, "Setting default provider");
                self.general.default_provider = Some(name.to_string());
                Ok(())
            }
            Some(_) => {
                error!(provider = %name, "Cannot set disabled provider as default");
                Err(AetherError::invalid_config(format!(
                    "Provider '{}' is not enabled",
                    name
                )))
            }
            None => {
                error!(provider = %name, "Provider not found in config");
                Err(AetherError::invalid_config(format!(
                    "Provider '{}' not found",
                    name
                )))
            }
        }
    }

    /// Get list of all enabled provider names
    ///
    /// Returns provider names in alphabetical order
    ///
    /// # Returns
    /// * `Vec<String>` - List of enabled provider names
    pub fn get_enabled_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self
            .providers
            .iter()
            .filter(|(_, cfg)| cfg.enabled)
            .map(|(name, _)| name.clone())
            .collect();
        providers.sort();
        providers
    }

    // ROUTING RULE MANAGEMENT METHODS

    /// Add a new routing rule at the top of the list (highest priority)
    ///
    /// New rules are inserted at index 0 to give them the highest priority
    /// in the first-match-stops routing algorithm.
    ///
    /// # Arguments
    /// * `rule` - The routing rule configuration to add
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::{Config, RoutingRuleConfig};
    /// let mut config = Config::default();
    /// config.add_rule_at_top(RoutingRuleConfig {
    ///     regex: r"^\[VSCode\]".to_string(),
    ///     provider: "claude".to_string(),
    ///     system_prompt: Some("You are a coding assistant.".to_string()),
    /// });
    /// // This rule now has highest priority (index 0)
    /// ```
    pub fn add_rule_at_top(&mut self, rule: RoutingRuleConfig) {
        self.rules.insert(0, rule);
        debug!(
            rules_count = self.rules.len(),
            "Added rule at top (highest priority)"
        );
    }

    /// Remove a routing rule by index
    ///
    /// # Arguments
    /// * `index` - Index of the rule to remove (0-based)
    ///
    /// # Returns
    /// * `Ok(())` - Rule removed successfully
    /// * `Err(AetherError::InvalidConfig)` - Index out of bounds
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let mut config = Config::default();
    /// // Assuming rule exists at index 0
    /// config.remove_rule(0)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn remove_rule(&mut self, index: usize) -> Result<()> {
        if index < self.rules.len() {
            let removed = self.rules.remove(index);
            debug!(
                index = index,
                rule_type = %removed.get_rule_type(),
                regex = %removed.regex,
                rules_count = self.rules.len(),
                "Removed routing rule"
            );
            Ok(())
        } else {
            error!(
                index = index,
                max_index = self.rules.len().saturating_sub(1),
                "Rule index out of bounds"
            );
            Err(AetherError::invalid_config(format!(
                "Rule index {} out of bounds (valid range: 0-{})",
                index,
                self.rules.len().saturating_sub(1)
            )))
        }
    }

    /// Move a routing rule from one position to another
    ///
    /// This allows reordering rules to change their priority.
    ///
    /// # Arguments
    /// * `from` - Current index of the rule
    /// * `to` - Target index for the rule
    ///
    /// # Returns
    /// * `Ok(())` - Rule moved successfully
    /// * `Err(AetherError::InvalidConfig)` - Invalid indices
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let mut config = Config::default();
    /// // Move rule from index 2 to index 0 (highest priority)
    /// config.move_rule(2, 0)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn move_rule(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.rules.len() {
            error!(
                from_index = from,
                max_index = self.rules.len().saturating_sub(1),
                "Source rule index out of bounds"
            );
            return Err(AetherError::invalid_config(format!(
                "Source index {} out of bounds (valid range: 0-{})",
                from,
                self.rules.len().saturating_sub(1)
            )));
        }
        if to >= self.rules.len() {
            error!(
                to_index = to,
                max_index = self.rules.len().saturating_sub(1),
                "Target rule index out of bounds"
            );
            return Err(AetherError::invalid_config(format!(
                "Target index {} out of bounds (valid range: 0-{})",
                to,
                self.rules.len().saturating_sub(1)
            )));
        }

        let rule = self.rules.remove(from);
        self.rules.insert(to, rule);
        debug!(from = from, to = to, "Moved routing rule");
        Ok(())
    }

    /// Get a routing rule by index
    ///
    /// # Arguments
    /// * `index` - Index of the rule to retrieve (0-based)
    ///
    /// # Returns
    /// * `Some(&RoutingRuleConfig)` - Reference to the rule if found
    /// * `None` - Index out of bounds
    ///
    /// # Example
    /// ```rust,ignore
    /// # use aethecore::config::Config;
    /// let config = Config::default();
    /// if let Some(rule) = config.get_rule(0) {
    ///     println!("First rule: {}", rule.regex);
    /// }
    /// ```
    pub fn get_rule(&self, index: usize) -> Option<&RoutingRuleConfig> {
        self.rules.get(index)
    }

    /// Get the number of routing rules
    ///
    /// # Returns
    /// * `usize` - Number of routing rules configured
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

#[cfg(test)]
mod tests;
