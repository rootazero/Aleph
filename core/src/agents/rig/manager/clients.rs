//! Provider client creation utilities

use crate::agents::rig::config::RigAgentConfig;
use crate::error::{AetherError, Result};
use rig::providers::{anthropic, openai};
use tracing::debug;

/// Normalize base_url for OpenAI-compatible APIs
///
/// Ensures the URL ends with `/v1` for proper endpoint construction.
/// rig-core appends `/chat/completions` directly to base_url, so we need
/// to ensure the URL includes the `/v1` segment.
///
/// Examples:
/// - `https://ai.t8star.cn` -> `https://ai.t8star.cn/v1`
/// - `https://ai.t8star.cn/v1` -> `https://ai.t8star.cn/v1` (unchanged)
/// - `https://api.openai.com/v1/` -> `https://api.openai.com/v1`
pub fn normalize_openai_base_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    if url.ends_with("/v1") {
        url.to_string()
    } else {
        format!("{}/v1", url)
    }
}

/// Create OpenAI client
pub fn create_openai_client(config: &RigAgentConfig) -> Result<openai::Client> {
    let api_key = config
        .api_key
        .as_deref()
        .ok_or_else(|| AetherError::provider("OpenAI API key not configured"))?;

    if let Some(ref base_url) = config.base_url {
        let normalized_url = normalize_openai_base_url(base_url);
        debug!(
            original_url = %base_url,
            normalized_url = %normalized_url,
            "Normalizing OpenAI base URL"
        );
        openai::Client::builder()
            .api_key(api_key)
            .base_url(&normalized_url)
            .build()
            .map_err(|e| AetherError::provider(format!("Failed to create OpenAI client: {}", e)))
    } else {
        openai::Client::new(api_key)
            .map_err(|e| AetherError::provider(format!("Failed to create OpenAI client: {}", e)))
    }
}

/// Create Anthropic client
pub fn create_anthropic_client(config: &RigAgentConfig) -> Result<anthropic::Client> {
    let api_key = config
        .api_key
        .as_deref()
        .ok_or_else(|| AetherError::provider("Anthropic API key not configured"))?;

    if let Some(ref base_url) = config.base_url {
        anthropic::Client::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|e| AetherError::provider(format!("Failed to create Anthropic client: {}", e)))
    } else {
        anthropic::Client::new(api_key)
            .map_err(|e| AetherError::provider(format!("Failed to create Anthropic client: {}", e)))
    }
}

/// Create custom OpenAI-compatible client
pub fn create_custom_client(config: &RigAgentConfig) -> Result<openai::Client> {
    let api_key = config
        .api_key
        .as_deref()
        .ok_or_else(|| AetherError::provider("API key not configured for provider"))?;

    let base_url = config.base_url.as_deref().ok_or_else(|| {
        AetherError::provider(format!(
            "base_url required for provider '{}'. Please configure it in your settings.",
            config.provider
        ))
    })?;

    let normalized_url = normalize_openai_base_url(base_url);
    debug!(
        original_url = %base_url,
        normalized_url = %normalized_url,
        provider = %config.provider,
        "Normalizing custom provider base URL"
    );

    openai::Client::builder()
        .api_key(api_key)
        .base_url(&normalized_url)
        .build()
        .map_err(|e| AetherError::provider(format!("Failed to create client: {}", e)))
}
