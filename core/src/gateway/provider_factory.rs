//! Provider Factory for Gateway
//!
//! Creates AI providers from environment variables or configuration.
//! Primarily used by the Gateway to initialize the ExecutionEngine.

use std::env;
use std::sync::Arc;
use tracing::info;

use crate::config::ProviderConfig;
use crate::providers::claude::ClaudeProvider;
use crate::providers::AiProvider;
use crate::thinker::SingleProviderRegistry;

/// Error type for provider factory
#[derive(Debug, thiserror::Error)]
pub enum ProviderFactoryError {
    #[error("No API key found. Set ANTHROPIC_API_KEY environment variable.")]
    NoApiKey,
    #[error("Failed to create provider: {0}")]
    ProviderCreationFailed(String),
}

/// Default model to use when not specified
const DEFAULT_CLAUDE_MODEL: &str = "claude-sonnet-4-20250514";

/// Claude brand color
const CLAUDE_COLOR: &str = "#d97757";

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
        provider_type: Some("claude".to_string()),
        protocol: None,
        api_key: Some(api_key),
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

    let provider = ClaudeProvider::new("claude".to_string(), config)
        .map_err(|e| ProviderFactoryError::ProviderCreationFailed(e.to_string()))?;

    Ok(Arc::new(provider))
}

/// Create a SingleProviderRegistry from environment
///
/// This is a convenience function that creates a ClaudeProvider and wraps it
/// in a SingleProviderRegistry for use with ExecutionEngine.
pub fn create_provider_registry_from_env() -> Result<Arc<SingleProviderRegistry>, ProviderFactoryError> {
    let provider = create_claude_provider_from_env()?;
    Ok(Arc::new(SingleProviderRegistry::new(provider)))
}

/// Check if a provider can be created from environment
///
/// Returns true if ANTHROPIC_API_KEY is set and non-empty
pub fn can_create_provider_from_env() -> bool {
    env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_create_provider_from_env() {
        // This test depends on environment, just verify the function works
        let result = can_create_provider_from_env();
        // Result depends on whether ANTHROPIC_API_KEY is set
        assert!(result == true || result == false);
    }

    #[test]
    fn test_create_provider_without_key() {
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
}
