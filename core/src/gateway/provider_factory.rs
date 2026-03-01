//! Provider Factory for Gateway
//!
//! Creates AI providers from environment variables or configuration.
//! Primarily used by the Gateway to initialize the ExecutionEngine.

use std::env;
use crate::sync_primitives::Arc;
use tracing::info;

use crate::config::ProviderConfig;
use crate::providers::{create_provider, AiProvider};
use crate::thinker::SingleProviderRegistry;

/// Error type for provider factory
#[derive(Debug, thiserror::Error)]
pub enum ProviderFactoryError {
    #[error("No API key found. Set ANTHROPIC_API_KEY or OPENAI_API_KEY environment variable.")]
    NoApiKey,
    #[error("Failed to create provider: {0}")]
    ProviderCreationFailed(String),
}

/// Default model to use when not specified
const DEFAULT_CLAUDE_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o";

/// Brand colors
const CLAUDE_COLOR: &str = "#d97757";
const OPENAI_COLOR: &str = "#10a37f";

/// Create a ClaudeProvider from environment variables
///
/// Environment variables:
/// - `ANTHROPIC_API_KEY` (required): API key for Anthropic
/// - `ANTHROPIC_MODEL` (optional): Model to use, defaults to claude-sonnet-4-20250514
/// - `ANTHROPIC_BASE_URL` (optional): Custom API endpoint
///
/// # Returns
///
/// A configured ClaudeProvider wrapped in Arc<dyn AiProvider>
pub fn create_claude_provider_from_env() -> Result<Arc<dyn AiProvider>, ProviderFactoryError> {
    // Read API key from environment
    let api_key = env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or(ProviderFactoryError::NoApiKey)?;

    // Read optional model
    let model = env::var("ANTHROPIC_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_CLAUDE_MODEL.to_string());

    // Read optional base URL
    let base_url = env::var("ANTHROPIC_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());

    info!(
        model = %model,
        has_custom_base_url = base_url.is_some(),
        "Creating Claude provider from environment"
    );

    let config = ProviderConfig {
        protocol: Some("anthropic".to_string()),
        api_key: Some(api_key),
        secret_name: None,
        model,
        base_url,
        color: CLAUDE_COLOR.to_string(),
        timeout_seconds: 300,
        enabled: true,
        max_tokens: Some(8192),
        temperature: Some(0.7),
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

    let provider = create_provider("claude", config)
        .map_err(|e| ProviderFactoryError::ProviderCreationFailed(e.to_string()))?;

    Ok(provider)
}

/// Create an OpenAI provider from environment variables
///
/// Environment variables:
/// - `OPENAI_API_KEY` (required): API key for OpenAI
/// - `OPENAI_MODEL` (optional): Model to use, defaults to gpt-4o
/// - `OPENAI_BASE_URL` (optional): Custom API endpoint (for OpenAI-compatible APIs)
///
/// # Returns
///
/// A configured OpenAI provider wrapped in Arc<dyn AiProvider>
pub fn create_openai_provider_from_env() -> Result<Arc<dyn AiProvider>, ProviderFactoryError> {
    // Read API key from environment
    let api_key = env::var("OPENAI_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or(ProviderFactoryError::NoApiKey)?;

    // Read optional model
    let model = env::var("OPENAI_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());

    // Read optional base URL
    let base_url = env::var("OPENAI_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());

    info!(
        model = %model,
        has_custom_base_url = base_url.is_some(),
        "Creating OpenAI provider from environment"
    );

    let config = ProviderConfig {
        protocol: Some("openai".to_string()),
        api_key: Some(api_key),
        secret_name: None,
        model,
        base_url,
        color: OPENAI_COLOR.to_string(),
        timeout_seconds: 300,
        enabled: true,
        max_tokens: Some(4096),
        temperature: Some(0.7),
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

    let provider = create_provider("openai", config)
        .map_err(|e| ProviderFactoryError::ProviderCreationFailed(e.to_string()))?;

    Ok(provider)
}

/// Create a SingleProviderRegistry from environment
///
/// This is a convenience function that creates a provider from environment
/// variables and wraps it in a SingleProviderRegistry for use with ExecutionEngine.
///
/// Tries providers in order:
/// 1. Anthropic (ANTHROPIC_API_KEY)
/// 2. OpenAI (OPENAI_API_KEY)
pub fn create_provider_registry_from_env() -> Result<Arc<SingleProviderRegistry>, ProviderFactoryError> {
    // Try Anthropic first
    if let Ok(provider) = create_claude_provider_from_env() {
        return Ok(Arc::new(SingleProviderRegistry::new(provider)));
    }

    // Try OpenAI second
    if let Ok(provider) = create_openai_provider_from_env() {
        return Ok(Arc::new(SingleProviderRegistry::new(provider)));
    }

    // No provider available
    Err(ProviderFactoryError::NoApiKey)
}

/// Check if a provider can be created from environment
///
/// Returns true if ANTHROPIC_API_KEY or OPENAI_API_KEY is set and non-empty
pub fn can_create_provider_from_env() -> bool {
    env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
        || env::var("OPENAI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .is_some()
}

/// Check which provider is available from environment
///
/// Returns the name of the first available provider, or None if none available
pub fn available_provider_from_env() -> Option<&'static str> {
    if env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        Some("anthropic")
    } else if env::var("OPENAI_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        Some("openai")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_create_provider_from_env() {
        // This test depends on environment, just verify the function works
        let result = can_create_provider_from_env();
        // Result depends on whether ANTHROPIC_API_KEY or OPENAI_API_KEY is set
        let _ = result;
    }

    #[test]
    fn test_create_claude_provider_without_key() {
        // Temporarily unset the key
        let original = env::var("ANTHROPIC_API_KEY").ok();
        env::remove_var("ANTHROPIC_API_KEY");

        let result = create_claude_provider_from_env();
        assert!(matches!(result, Err(ProviderFactoryError::NoApiKey)));

        // Restore original value
        if let Some(key) = original {
            env::set_var("ANTHROPIC_API_KEY", key);
        }
    }

    #[test]
    fn test_create_openai_provider_without_key() {
        // Temporarily unset the key
        let original = env::var("OPENAI_API_KEY").ok();
        env::remove_var("OPENAI_API_KEY");

        let result = create_openai_provider_from_env();
        assert!(matches!(result, Err(ProviderFactoryError::NoApiKey)));

        // Restore original value
        if let Some(key) = original {
            env::set_var("OPENAI_API_KEY", key);
        }
    }

    #[test]
    fn test_available_provider_from_env() {
        let result = available_provider_from_env();
        // Result depends on environment
        assert!(result.is_none() || result == Some("anthropic") || result == Some("openai"));
    }
}
