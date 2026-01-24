//! Configuration validation logic
//!
//! This module handles validation of configuration values.

use crate::config::Config;
use crate::error::{AetherError, Result};
use tracing::{debug, error, info, warn};

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
}
