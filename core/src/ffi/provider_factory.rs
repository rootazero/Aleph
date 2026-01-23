//! Provider factory for creating AI providers from config
//!
//! This module provides factory functions for creating AI providers
//! from agent configuration.

use std::sync::Arc;

use crate::providers::AiProvider;

/// Create an AI provider from config
pub fn create_provider_from_config(
    config: &crate::agents::RigAgentConfig,
) -> Result<Arc<dyn AiProvider>, String> {
    use crate::config::ProviderConfig;

    let provider_config = ProviderConfig {
        provider_type: Some(config.provider.clone()),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        base_url: config.base_url.clone(),
        color: "#808080".to_string(), // Default gray
        timeout_seconds: config.timeout_seconds,
        enabled: true,
        max_tokens: Some(config.max_tokens),
        temperature: Some(config.temperature),
        top_p: None,
        top_k: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
    };

    // Use provider_name (config key like "t8star") if available, otherwise fall back to provider_type
    let provider_name = config.provider_name.as_deref().unwrap_or(&config.provider);
    crate::providers::create_provider(provider_name, provider_config).map_err(|e| e.to_string())
}
