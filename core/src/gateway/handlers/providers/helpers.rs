//! Provider handler helper functions.

use tracing::warn;

use crate::config::{Config, ProviderConfig};
use crate::config::presets_override::PresetsOverride;
use crate::providers::presets::get_merged_preset;
use crate::gateway::security::SharedTokenManager;

use super::types::ProviderConfigJson;

/// Vault key prefix for AI provider API keys
pub(super) fn vault_key(provider_name: &str) -> String {
    format!("ai:{}", provider_name)
}

/// Resolve API key from vault into a ProviderInfo response
pub(super) fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    match vault.get_secret(&vault_key(name)) {
        Ok(Some(secret)) => Some(secret.expose().to_string()),
        Ok(None) => None,
        Err(e) => {
            warn!(provider = %name, error = %e, "Failed to read API key from vault");
            None
        }
    }
}

pub(super) fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub(super) fn save_config(cfg: &Config) -> Result<(), String> {
    // api_key is #[serde(skip)] — never persisted to disk
    cfg.save().map_err(|e| e.to_string())
}

pub(super) fn build_provider_config_for_persistence(
    provider_name: &str,
    params: ProviderConfigJson,
    presets_override: &PresetsOverride,
) -> ProviderConfig {
    // api_key is runtime-only (#[serde(skip)]) — kept in memory, never persisted
    let api_key = normalize_optional_string(params.api_key);

    // Restore preset defaults for empty base_url / model (merged with user overrides)
    let preset = get_merged_preset(provider_name, presets_override);

    let base_url = match normalize_optional_string(params.base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().map(|p| p.base_url.clone()),
    };

    let model = {
        let trimmed = params.model.trim();
        if trimmed.is_empty() {
            preset
                .as_ref()
                .filter(|p| !p.default_model.is_empty())
                .map(|p| p.default_model.clone())
                .unwrap_or_default()
        } else {
            trimmed.to_string()
        }
    };

    // Ensure protocol is always persisted (prevents "openai" default on reload)
    let protocol = params.protocol.or_else(|| {
        preset.as_ref().map(|p| p.protocol.clone())
    });

    ProviderConfig {
        protocol,
        api_key,
        model,
        base_url,
        color: params.color.unwrap_or_else(|| "#808080".to_string()),
        timeout_seconds: params.timeout_seconds.unwrap_or(300),
        enabled: params.enabled,
        max_tokens: params.max_tokens,
        temperature: params.temperature,
        top_p: params.top_p,
        top_k: params.top_k,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
        verified: false,
    }
}
