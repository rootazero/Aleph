//! Configuration validation logic
//!
//! This module handles validation of configuration values.

use crate::config::Config;
use crate::error::{AlephError, Result};
use chrono::NaiveTime;
use tracing::{debug, error, info, warn};

/// Load-time normalization: when `general.default_provider` names a provider
/// that does not exist in `providers`, fall back to a deterministically-chosen
/// available provider rather than failing validation. This keeps the daemon
/// bootable when a provider section is removed but its default reference is
/// forgotten — a common config drift that otherwise leaves the desktop shell
/// stuck on the splash screen while the daemon exits on every launch.
///
/// If no providers are configured at all, the fallback is not applied; the
/// existing validation error is preserved so the operator is told to add one.
pub fn normalize_default_provider(cfg: &mut Config) {
    let Some(ref current) = cfg.general.default_provider else {
        return;
    };
    if cfg.providers.contains_key(current) {
        return;
    }
    if cfg.providers.is_empty() {
        return;
    }
    let fallback = match cfg.providers.keys().min() {
        Some(k) => k.clone(),
        None => return,
    };
    warn!(
        default_provider = %current,
        fallback_provider = %fallback,
        "Default provider not found; falling back to available provider"
    );
    cfg.general.default_provider = Some(fallback);
}

impl Config {
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

        self.validate_default_provider()?;
        self.validate_provider_configs()?;
        self.validate_rules()?;
        self.validate_memory_config()?;
        self.validate_language_preference();
        self.validate_search_config()?;
        self.validate_group_chat_and_personas()?;
        self.validate_policies()?;

        info!(
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Config validation completed successfully"
        );

        Ok(())
    }

    /// Validate that the configured default provider exists in the providers map.
    fn validate_default_provider(&self) -> Result<()> {
        // Validate default provider exists (if configured)
        if let Some(ref default_provider) = self.general.default_provider {
            if !self.providers.contains_key(default_provider) {
                error!(default_provider = %default_provider, "Default provider not found");
                return Err(AlephError::invalid_config(format!(
                    "Default provider '{default_provider}' not found in providers"
                )));
            }
            debug!(default_provider = %default_provider, "Default provider validated");
        }

        Ok(())
    }

    /// Validate each provider's timeout, sampling parameters, and protocol-specific fields.
    fn validate_provider_configs(&self) -> Result<()> {
        // Validate provider configurations
        for (name, provider) in &self.providers {
            let protocol = provider.protocol();

            // Note: api_key is a runtime-only field populated from the encrypted vault
            // at startup, so we don't validate credentials at config-load time.

            // Validate timeout
            if provider.timeout_seconds == 0 {
                error!(provider = %name, "Provider timeout is zero");
                return Err(AlephError::invalid_config(format!(
                    "Provider '{name}' timeout must be greater than 0"
                )));
            }

            // Validate temperature if specified (provider-specific ranges)
            if let Some(temp) = provider.temperature {
                let (min, max, provider_name): (f32, f32, &str) = match protocol.as_str() {
                    "anthropic" => (0.0, 1.0, "Claude"),
                    "openai" => (0.0, 2.0, "OpenAI"),
                    "gemini" => (0.0, 2.0, "Gemini"),
                    "ollama" => (0.0, 5.0, "Ollama"),
                    _ => (0.0, 2.0, "Custom"),
                };

                if !(min..=max).contains(&temp) {
                    error!(provider = %name, temperature = temp, "Invalid temperature for {}", provider_name);
                    return Err(AlephError::invalid_config(format!(
                        "Provider '{name}' ({provider_name}) temperature must be between {min} and {max}, got {temp}"
                    )));
                }
            }

            // Validate max_tokens if specified
            if let Some(max_tokens) = provider.max_tokens {
                if max_tokens == 0 {
                    error!(provider = %name, max_tokens = max_tokens, "Invalid max_tokens");
                    return Err(AlephError::invalid_config(format!(
                        "Provider '{name}' max_tokens must be greater than 0, got {max_tokens}"
                    )));
                }
            }

            // Validate top_p if specified
            if let Some(top_p) = provider.top_p {
                if !(0.0..=1.0).contains(&top_p) {
                    error!(provider = %name, top_p = top_p, "Invalid top_p");
                    return Err(AlephError::invalid_config(format!(
                        "Provider '{name}' top_p must be between 0.0 and 1.0, got {top_p}"
                    )));
                }
            }

            // Validate top_k if specified
            if let Some(top_k) = provider.top_k {
                if top_k == 0 {
                    error!(provider = %name, top_k = top_k, "Invalid top_k");
                    return Err(AlephError::invalid_config(format!(
                        "Provider '{name}' top_k must be greater than 0, got {top_k}"
                    )));
                }
            }

            // Validate OpenAI-specific parameters
            if protocol == "openai" {
                if let Some(freq_pen) = provider.frequency_penalty {
                    if !(-2.0..=2.0).contains(&freq_pen) {
                        error!(provider = %name, frequency_penalty = freq_pen, "Invalid frequency_penalty");
                        return Err(AlephError::invalid_config(format!(
                            "Provider '{name}' frequency_penalty must be between -2.0 and 2.0, got {freq_pen}"
                        )));
                    }
                }

                if let Some(pres_pen) = provider.presence_penalty {
                    if !(-2.0..=2.0).contains(&pres_pen) {
                        error!(provider = %name, presence_penalty = pres_pen, "Invalid presence_penalty");
                        return Err(AlephError::invalid_config(format!(
                            "Provider '{name}' presence_penalty must be between -2.0 and 2.0, got {pres_pen}"
                        )));
                    }
                }
            }

            // Validate Gemini-specific parameters
            if protocol == "gemini" {
                if let Some(ref thinking_level) = provider.thinking_level {
                    if thinking_level != "LOW" && thinking_level != "HIGH" {
                        error!(provider = %name, thinking_level = %thinking_level, "Invalid thinking_level");
                        return Err(AlephError::invalid_config(format!(
                            "Provider '{name}' thinking_level must be 'LOW' or 'HIGH', got '{thinking_level}'"
                        )));
                    }
                }

                if let Some(ref media_res) = provider.media_resolution {
                    if media_res != "LOW" && media_res != "MEDIUM" && media_res != "HIGH" {
                        error!(provider = %name, media_resolution = %media_res, "Invalid media_resolution");
                        return Err(AlephError::invalid_config(format!(
                            "Provider '{name}' media_resolution must be 'LOW', 'MEDIUM', or 'HIGH', got '{media_res}'"
                        )));
                    }
                }
            }

            // Validate Ollama-specific parameters
            if protocol == "ollama" {
                if let Some(repeat_pen) = provider.repeat_penalty {
                    if repeat_pen < 0.0 {
                        error!(provider = %name, repeat_penalty = repeat_pen, "Invalid repeat_penalty");
                        return Err(AlephError::invalid_config(format!(
                            "Provider '{name}' repeat_penalty must be >= 0.0, got {repeat_pen}"
                        )));
                    }
                }
            }

            debug!(
                provider = %name,
                protocol = %protocol,
                timeout_seconds = provider.timeout_seconds,
                "Provider validated"
            );
        }

        Ok(())
    }

    /// Validate routing rules: provider references, keyword prompts, and regex patterns.
    fn validate_rules(&self) -> Result<()> {
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
                            return Err(AlephError::invalid_config(format!(
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
                        return Err(AlephError::invalid_config(format!(
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

            // Validate regex pattern. `bounded_builder`, not `Regex::new`:
            // the runtime compile site (`pii::rules::custom`) enforces the
            // 1 MiB compiled-size cap, so validating WITHOUT it would both
            // admit a pattern the runtime then rejects and let an expansion
            // bomb (`(a{1000}){1000}{1000}`) exhaust memory during validation
            // itself.
            if let Err(e) = crate::security::safe_regex::bounded_builder(&rule.regex).build() {
                error!(
                    rule_index = idx + 1,
                    regex = %rule.regex,
                    error = %e,
                    "Invalid regex pattern"
                );
                return Err(AlephError::invalid_config(format!(
                    "Rule #{} has invalid regex '{}': {}",
                    idx + 1,
                    rule.regex,
                    e
                )));
            }
        }

        Ok(())
    }

    /// Validate memory thresholds, dreaming schedule, and decay settings.
    fn validate_memory_config(&self) -> Result<()> {
        // Validate memory config
        if !(0.0..=1.0).contains(&self.memory.similarity_threshold) {
            error!(
                threshold = self.memory.similarity_threshold,
                "Invalid similarity threshold"
            );
            return Err(AlephError::invalid_config(format!(
                "memory.similarity_threshold must be between 0.0 and 1.0, got {}",
                self.memory.similarity_threshold
            )));
        }

        if self.memory.dreaming.enabled {
            if self.memory.dreaming.max_duration_seconds == 0 {
                error!("Dreaming max_duration_seconds is zero");
                return Err(AlephError::invalid_config(
                    "memory.dreaming.max_duration_seconds must be greater than 0",
                ));
            }

            if self.memory.dreaming.feedback_distill_max_per_cycle == 0 {
                error!("Dreaming feedback_distill_max_per_cycle is zero");
                return Err(AlephError::invalid_config(
                    "memory.dreaming.feedback_distill_max_per_cycle must be greater than 0",
                ));
            }

            if self.memory.dreaming.feedback_lookback == 0 {
                error!("Dreaming feedback_lookback is zero");
                return Err(AlephError::invalid_config(
                    "memory.dreaming.feedback_lookback must be greater than 0",
                ));
            }

            let start = match NaiveTime::parse_from_str(
                &self.memory.dreaming.window_start_local,
                "%H:%M",
            ) {
                Ok(t) => t,
                Err(_) => {
                    error!(
                        window_start = %self.memory.dreaming.window_start_local,
                        "Invalid dreaming window_start_local"
                    );
                    return Err(AlephError::invalid_config(format!(
                        "memory.dreaming.window_start_local must be HH:MM, got {}",
                        self.memory.dreaming.window_start_local
                    )));
                }
            };

            let end =
                match NaiveTime::parse_from_str(&self.memory.dreaming.window_end_local, "%H:%M") {
                    Ok(t) => t,
                    Err(_) => {
                        error!(
                            window_end = %self.memory.dreaming.window_end_local,
                            "Invalid dreaming window_end_local"
                        );
                        return Err(AlephError::invalid_config(format!(
                            "memory.dreaming.window_end_local must be HH:MM, got {}",
                            self.memory.dreaming.window_end_local
                        )));
                    }
                };

            // Only reject an empty (start == end) window. A `start > end` window
            // is a valid overnight span (e.g. 22:00–06:00) — `is_within_window`
            // explicitly handles wrap-around, and overnight is the most natural
            // idle window. Rejecting `start >= end` made that branch unreachable
            // and forbade the common "after I go to bed" configuration.
            if start == end {
                error!(
                    window_start = %self.memory.dreaming.window_start_local,
                    window_end = %self.memory.dreaming.window_end_local,
                    "Dreaming window start must differ from end"
                );
                return Err(AlephError::invalid_config(format!(
                    "memory.dreaming.window_start_local ({}) must differ from window_end_local ({})",
                    self.memory.dreaming.window_start_local,
                    self.memory.dreaming.window_end_local
                )));
            }
        }

        if self.memory.memory_decay.half_life_days <= 0.0 {
            error!(
                value = self.memory.memory_decay.half_life_days,
                "Invalid memory decay half_life_days"
            );
            return Err(AlephError::invalid_config(format!(
                "memory.memory_decay.half_life_days must be greater than 0, got {}",
                self.memory.memory_decay.half_life_days
            )));
        }

        if !(0.0..=1.0).contains(&self.memory.memory_decay.min_strength) {
            error!(
                value = self.memory.memory_decay.min_strength,
                "Invalid memory decay min_strength"
            );
            return Err(AlephError::invalid_config(format!(
                "memory.memory_decay.min_strength must be between 0.0 and 1.0, got {}",
                self.memory.memory_decay.min_strength
            )));
        }

        let allowed_types = [
            "preference",
            "plan",
            "learning",
            "project",
            "personal",
            "other",
        ];
        for entry in &self.memory.memory_decay.protected_types {
            let lower = entry.to_lowercase();
            if !allowed_types.contains(&lower.as_str()) {
                warn!(
                    protected_type = %entry,
                    "Unknown memory_decay protected_type"
                );
            }
        }

        debug!(
            memory_enabled = self.memory.enabled,
            similarity_threshold = self.memory.similarity_threshold,
            "Memory config validated"
        );

        Ok(())
    }

    /// Advise on unsupported language codes (warn-only; falls back to system language).
    fn validate_language_preference(&self) {
        // Validate language preference
        if let Some(ref language) = self.general.language {
            // List of supported language codes (must match .lproj directory names)
            let supported_languages = ["en", "zh-Hans"];

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
    }

    /// Validate search backends, default/fallback provider references, and limits.
    fn validate_search_config(&self) -> Result<()> {
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
                    return Err(AlephError::invalid_config(format!(
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
                            return Err(AlephError::invalid_config(format!(
                                "Search fallback provider '{provider_name}' not found in backends"
                            )));
                        }
                    }
                }

                // Validate max_results is reasonable
                if search_config.max_results == 0 {
                    error!("Search max_results cannot be 0");
                    return Err(AlephError::invalid_config(
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
                    return Err(AlephError::invalid_config(
                        "Search timeout_seconds must be greater than 0".to_string(),
                    ));
                }

                // Validate each backend configuration.
                //
                // NOTE: api_key is `#[serde(skip)]` and injected from the
                // vault at runtime, so it cannot be validated here. The
                // known `provider_type` set is sourced from
                // `ProviderFactoryRegistry::with_defaults()` so adding a
                // new provider does not require keeping a second allowlist
                // in sync.
                //
                // Per-provider structural prerequisites that can be checked
                // *before* vault injection (e.g. `engine_id` for Google,
                // `base_url` for SearXNG) are still enforced here as hard
                // errors — they're typos in TOML, not missing secrets.
                let factory_registry = crate::search::ProviderFactoryRegistry::with_defaults();
                let known_types = factory_registry.known_provider_types();
                for (backend_name, backend_config) in &search_config.backends {
                    let provider_type = backend_config.provider_type.as_str();

                    if !known_types.contains(&provider_type) {
                        warn!(
                            backend = %backend_name,
                            provider_type = %provider_type,
                            known = ?known_types,
                            "Unknown search provider type (no factory registered)"
                        );
                    }

                    match provider_type {
                        "google" => {
                            if backend_config.engine_id.is_none() {
                                error!(backend = %backend_name, "Google backend requires engine_id");
                                return Err(AlephError::invalid_config(format!(
                                    "Search backend '{backend_name}' (Google) requires an engine_id"
                                )));
                            }
                        }
                        "searxng" if backend_config.base_url.is_none() => {
                            error!(backend = %backend_name, "SearXNG backend requires base_url");
                            return Err(AlephError::invalid_config(format!(
                                "Search backend '{backend_name}' (SearXNG) requires a base_url"
                            )));
                        }
                        _ => {
                            // All other providers: api_key (or no
                            // credentials) is validated post-vault injection
                            // by the provider factory at registry-build time.
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

        Ok(())
    }

    /// Validate group-chat and persona sub-configs.
    fn validate_group_chat_and_personas(&self) -> Result<()> {
        // Validate group-chat section (sub-config `validate` methods are not
        // reachable from `Config::validate` otherwise, so a typo like
        // `max_personas_per_session = 0` would silently disable every session).
        if let Err(e) = self.group_chat.validate() {
            error!(error = %e, "Group-chat config validation failed");
            return Err(AlephError::invalid_config(format!("group_chat.{e}")));
        }
        let mut seen_persona_ids = std::collections::HashSet::new();
        for (idx, persona) in self.personas.iter().enumerate() {
            if let Err(e) = persona.validate() {
                error!(index = idx, persona_id = %persona.id, error = %e, "Persona config validation failed");
                return Err(AlephError::invalid_config(format!(
                    "personas[{idx}] (id={}): {e}",
                    persona.id
                )));
            }
            // Fail fast on duplicate persona ids: `PersonaRegistry::from_configs`
            // silently applies last-wins on a duplicate (an operator who
            // copy-pastes a `[[personas]]` block and forgets to change the id
            // loses the first definition entirely). Startup validation is the
            // right place to catch it (M7 in review/group_chat-statics).
            if !seen_persona_ids.insert(persona.id.as_str()) {
                error!(persona_id = %persona.id, "Duplicate persona id in config");
                return Err(AlephError::invalid_config(format!(
                    "personas[{idx}]: duplicate persona id '{}'",
                    persona.id
                )));
            }
        }

        Ok(())
    }

    /// Refuse `exec_tier = "plan"` as an install-wide default.
    fn validate_policies(&self) -> Result<()> {
        // `plan` deserializes here — it is a real `ExecTier`, just not an
        // INSTALL one: it is a per-conversation posture that ENDS when a human
        // approves a plan, and ending it means falling back to this very
        // setting. A machine-wide `plan` would put every conversation into
        // planning and then hand each approved plan straight back to planning,
        // with the approval spent getting there.
        //
        // Refused rather than normalized: the RPC that writes this key refuses
        // it too (`config::update_tool_permissions`), and a file that quietly
        // ran at a different tier than it names is how a setting comes to
        // "sometimes work".
        if self.policies.exec_tier == crate::config::types::policies::ExecTier::Plan {
            error!("[policies] exec_tier = \"plan\" is not an install-wide default");
            return Err(AlephError::invalid_config(
                "[policies] exec_tier = \"plan\" is not an install-wide default: planning is a \
                 per-conversation posture that ends when you approve a plan, and it would have \
                 nothing to hand back to. Set the install to ask / auto / full, and put a single \
                 conversation into planning from the composer's tier pill (or `/tier plan`).",
            ));
        }

        Ok(())
    }

    /// Emit one-time advisory recommendations about the loaded config.
    ///
    /// These are *not* validation errors — they are startup hints. They live
    /// outside [`Config::validate`] because `validate` runs on every config
    /// reload/patch (and on every programmatic `Config::load`), which would
    /// otherwise spam the log. Call this exactly once from the startup path.
    pub fn log_advisories(&self) {
        if self.general.default_provider.is_none() {
            warn!(
                "No default_provider configured. \
                 Requests will fail if no routing rule matches. \
                 Recommendation: Set general.default_provider in config"
            );
        }

        if self.rules.is_empty() {
            debug!(
                "No routing rules configured. \
                 All requests use default_provider — this is the expected \
                 default for LLM-sovereign routing"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::types::provider::ProviderConfig;
    use crate::config::Config;

    use super::normalize_default_provider;

    /// `plan` parses as an `ExecTier`, so a hand-edited TOML reaches this
    /// field. The three install tiers must still load.
    #[test]
    fn plan_is_refused_as_an_install_wide_tier() {
        use crate::config::types::policies::ExecTier;

        let mut cfg = Config::default();
        cfg.policies.exec_tier = ExecTier::Plan;
        let err = cfg
            .validate()
            .expect_err("a machine-wide plan tier has nothing to hand back to");
        let msg = err.to_string();
        assert!(msg.contains("per-conversation"), "{msg}");

        for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            let mut cfg = Config::default();
            cfg.policies.exec_tier = tier;
            assert!(cfg.validate().is_ok(), "{tier:?} must still load");
        }
    }

    fn test_provider() -> ProviderConfig {
        ProviderConfig {
            protocol: None,
            api_key: None,
            models: vec![],
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 60,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            enabled: true,
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            response_format: None,
            parallel_tool_calls: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            metadata_user_id: None,
            effort: None,
        }
    }

    #[test]
    fn normalize_falls_back_to_available_provider() {
        let mut cfg = Config::default();
        cfg.providers.insert("custom".to_string(), test_provider());
        cfg.general.default_provider = Some("missing".to_string());

        normalize_default_provider(&mut cfg);

        assert_eq!(cfg.general.default_provider.as_deref(), Some("custom"));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn normalize_keeps_valid_default_unchanged() {
        let mut cfg = Config::default();
        cfg.providers.insert("custom".to_string(), test_provider());
        cfg.general.default_provider = Some("custom".to_string());

        normalize_default_provider(&mut cfg);

        assert_eq!(cfg.general.default_provider.as_deref(), Some("custom"));
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn normalize_no_op_when_no_providers_configured() {
        let mut cfg = Config::default();
        cfg.general.default_provider = Some("missing".to_string());

        normalize_default_provider(&mut cfg);

        assert_eq!(cfg.general.default_provider.as_deref(), Some("missing"));
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn normalize_no_op_when_default_unset() {
        let mut cfg = Config::default();
        cfg.providers.insert("custom".to_string(), test_provider());
        cfg.general.default_provider = None;

        normalize_default_provider(&mut cfg);

        assert_eq!(cfg.general.default_provider, None);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn normalize_chooses_fallback_deterministically() {
        let mut cfg = Config::default();
        cfg.providers.insert("beta".to_string(), test_provider());
        cfg.providers.insert("alpha".to_string(), test_provider());
        cfg.providers.insert("gamma".to_string(), test_provider());
        cfg.general.default_provider = Some("missing".to_string());

        normalize_default_provider(&mut cfg);

        // Alphabetical minimum is chosen for deterministic behavior.
        assert_eq!(cfg.general.default_provider.as_deref(), Some("alpha"));
    }
}
