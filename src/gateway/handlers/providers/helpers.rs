//! Provider handler helper functions.

use crate::config::presets_override::PresetsOverride;
use crate::config::{Config, ProviderConfig};
use crate::gateway::security::SharedTokenManager;
use crate::providers::presets::get_merged_preset;

use super::types::ProviderConfigJson;

/// Vault key for AI provider API keys (`ai:<name>`) — single definition
/// shared with the diagnostics connectivity check.
pub(super) fn vault_key(provider_name: &str) -> String {
    crate::providers::probe::provider_vault_key(provider_name)
}

/// Resolve the actual API key from vault (for internal use like test/create-provider, never serialized to responses)
pub(super) fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    crate::gateway::handlers::resolve_vault_secret(&vault_key(name), vault)
}

pub(super) use crate::gateway::handlers::normalize_optional_string;

pub(super) fn save_config(cfg: &Config, sections: &[&str]) -> Result<(), String> {
    // api_key is #[serde(skip)] — never persisted to disk
    cfg.save_incremental(sections).map_err(|e| e.to_string())
}

/// Build the `ProviderConfig` to persist for a create/update RPC.
///
/// `prior` is the currently-stored entry on `providers.update`, and `None` on
/// `providers.create`. It exists because `ProviderConfigJson` is a lossy view
/// of `ProviderConfig`: roughly a dozen fields have no DTO representation at
/// all. Rebuilding the struct from the DTO alone therefore *erased* every one
/// of them whenever the user changed anything else about the provider — a
/// model id, a timeout, a colour — both in memory and, via `save_config`, on
/// disk.
///
/// `cache_retention` is the one that costs money: an operator sets
/// `cache_retention = "long"` for the 1-hour prompt-cache TTL, later edits an
/// unrelated field from the Panel, and silently drops back to the 5-minute
/// default. Long-idle agents (cron fires, subscription loops, overnight
/// sessions) that used to land inside the window then re-pay full-price
/// `cache_creation` on every wake. Nothing logs it and the watchdog stays
/// quiet, because 5-minute caching still reports activity — the only symptom
/// is the bill. `stop_sequences`, `thinking_level`, `service_tier`, `effort`,
/// `seed` and the rest were eaten the same way.
///
/// Starting from `prior` and overwriting only the DTO-expressible fields also
/// means a field added to `ProviderConfig` later is carried forward by
/// default, rather than silently reset until someone notices.
pub(super) fn build_provider_config_for_persistence(
    provider_name: &str,
    params: ProviderConfigJson,
    presets_override: &PresetsOverride,
    prior: Option<&ProviderConfig>,
) -> ProviderConfig {
    // api_key is runtime-only (#[serde(skip)]) — kept in memory, never persisted
    let api_key = normalize_optional_string(params.api_key);

    // Restore preset defaults for empty base_url / model (merged with user overrides)
    let preset = get_merged_preset(provider_name, presets_override);

    let base_url = match normalize_optional_string(params.base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().map(|p| p.base_url.clone()),
    };

    let models = {
        // Filter empty models from the input
        let non_empty: Vec<String> = params
            .models
            .iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if non_empty.is_empty() {
            // Fall back to preset default model
            preset
                .as_ref()
                .filter(|p| !p.default_model.is_empty())
                .map(|p| vec![p.default_model.clone()])
                .unwrap_or_default()
        } else {
            non_empty
        }
    };

    // Ensure protocol is always persisted (prevents "openai" default on reload)
    let protocol = params
        .protocol
        .or_else(|| preset.as_ref().map(|p| p.protocol.clone()));

    // Baseline: the stored entry when updating (so DTO-inexpressible fields
    // survive), otherwise a fresh all-unset config.
    let mut cfg = match prior {
        Some(existing) => existing.clone(),
        None => ProviderConfig {
            protocol: None,
            api_key: None,
            models: Vec::new(),
            base_url: None,
            color: String::new(),
            timeout_seconds: 300,
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
            stream_idle_timeout_secs: None,
            cache_retention: None,
            response_format: None,
            parallel_tool_calls: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            metadata_user_id: None,
            effort: None,
        },
    };

    // Everything the DTO can express is authoritative and overwrites the
    // baseline. Everything it cannot is left as the baseline carried it.
    cfg.protocol = protocol;
    cfg.api_key = api_key;
    cfg.models = models;
    cfg.base_url = base_url;
    cfg.color = params.color.unwrap_or_else(|| "#808080".to_string());
    cfg.timeout_seconds = params.timeout_seconds.unwrap_or(300);
    cfg.enabled = params.enabled;
    cfg.max_tokens = params.max_tokens;
    cfg.context_window = params.context_window;
    cfg.temperature = params.temperature;
    cfg.top_p = params.top_p;
    cfg.top_k = params.top_k;
    // Any edit invalidates a previous connectivity check.
    cfg.verified = false;

    cfg
}
