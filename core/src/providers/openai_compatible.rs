/// Custom OpenAI-compatible provider (thin wrapper over OpenAiProvider)
///
/// This wrapper is for custom OpenAI-compatible API endpoints that are not
/// pre-configured in Aether. Unlike preset providers (OpenAI, DeepSeek, Doubao, etc.),
/// this requires users to explicitly provide a `base_url`.
///
/// # Use Cases
///
/// - Self-hosted OpenAI-compatible servers (e.g., vLLM, Text Generation WebUI)
/// - Proxy/relay services (e.g., OpenRouter, custom proxies)
/// - Enterprise API gateways
/// - Any other OpenAI-compatible endpoint not covered by preset providers
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: API key (if required by the endpoint)
/// - `model`: Model name supported by the endpoint
/// - `base_url`: **REQUIRED** - The base URL of the API endpoint
///
/// Optional fields:
/// - `timeout_seconds`: Request timeout (defaults to 30)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::openai_compatible::OpenAiCompatibleProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// // Example 1: OpenRouter
/// let config = ProviderConfig {
///     api_key: Some("sk-or-...".to_string()),
///     model: "anthropic/claude-3-5-sonnet".to_string(),
///     base_url: Some("https://openrouter.ai/api/v1".to_string()), // Required!
///     ..Default::default()
/// };
/// let provider = OpenAiCompatibleProvider::new("openrouter".to_string(), config)?;
///
/// // Example 2: Self-hosted vLLM
/// let config = ProviderConfig {
///     api_key: None, // May not need API key for local server
///     model: "meta-llama/Llama-2-7b-chat-hf".to_string(),
///     base_url: Some("http://localhost:8000/v1".to_string()), // Required!
///     ..Default::default()
/// };
/// let provider = OpenAiCompatibleProvider::new("vllm".to_string(), config)?;
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::{AiProvider, OpenAiProvider};
use std::future::Future;
use std::pin::Pin;

/// Custom OpenAI-compatible provider (wrapper)
#[derive(Debug)]
pub struct OpenAiCompatibleProvider {
    inner: OpenAiProvider,
}

impl OpenAiCompatibleProvider {
    /// Create new custom OpenAI-compatible provider
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (user-defined)
    /// * `config` - Provider configuration (**base_url is required**)
    ///
    /// # Returns
    ///
    /// * `Ok(OpenAiCompatibleProvider)` - Successfully initialized provider
    /// * `Err(AetherError::InvalidConfig)` - Missing or empty base_url
    ///
    /// # Errors
    ///
    /// Returns error if `base_url` is not provided or is empty.
    pub fn new(name: String, config: ProviderConfig) -> Result<Self> {
        // Validate that base_url is provided and not empty
        let base_url = config
            .base_url
            .as_ref()
            .ok_or_else(|| {
                AetherError::invalid_config(format!(
                    "Custom OpenAI-compatible provider '{}' requires base_url. \
                     Please set base_url in config (e.g., 'https://api.example.com/v1')",
                    name
                ))
            })?;

        if base_url.is_empty() {
            return Err(AetherError::invalid_config(format!(
                "Custom OpenAI-compatible provider '{}' has empty base_url. \
                 Please provide a valid URL (e.g., 'https://api.example.com/v1')",
                name
            )));
        }

        // Validate that model is not empty
        if config.model.is_empty() {
            return Err(AetherError::invalid_config(format!(
                "Custom OpenAI-compatible provider '{}' requires model name",
                name
            )));
        }

        Ok(Self {
            inner: OpenAiProvider::new(name, config)?,
        })
    }
}

// Delegate all AiProvider methods to inner OpenAiProvider
impl AiProvider for OpenAiCompatibleProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        self.inner.process(input, system_prompt)
    }

    fn process_with_image(
        &self,
        input: &str,
        image: Option<&crate::clipboard::ImageData>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        self.inner.process_with_image(input, image, system_prompt)
    }

    fn supports_vision(&self) -> bool {
        self.inner.supports_vision()
    }

    fn process_with_attachments(
        &self,
        input: &str,
        attachments: Option<&[crate::core::MediaAttachment]>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        self.inner.process_with_attachments(input, attachments, system_prompt)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn color(&self) -> &str {
        self.inner.color()
    }

    fn process_with_mode(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        force_standard_mode: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        self.inner.process_with_mode(input, system_prompt, force_standard_mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_require_base_url() {
        let config = ProviderConfig::test_config("custom-model");
        // Should fail because base_url is None
        let result = OpenAiCompatibleProvider::new("custom".to_string(), config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires base_url"));
    }

    #[test]
    fn test_reject_empty_base_url() {
        let mut config = ProviderConfig::test_config("custom-model");
        config.base_url = Some("".to_string());
        // Should fail because base_url is empty
        let result = OpenAiCompatibleProvider::new("custom".to_string(), config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty base_url"));
    }

    #[test]
    fn test_valid_configuration() {
        let mut config = ProviderConfig::test_config("custom-model");
        config.base_url = Some("https://custom.api.com/v1".to_string());

        let provider = OpenAiCompatibleProvider::new("custom".to_string(), config).unwrap();
        assert_eq!(provider.name(), "custom");
    }

    #[test]
    fn test_openrouter_example() {
        let mut config = ProviderConfig::test_config("anthropic/claude-3-5-sonnet");
        config.base_url = Some("https://openrouter.ai/api/v1".to_string());

        let provider = OpenAiCompatibleProvider::new("openrouter".to_string(), config).unwrap();
        assert_eq!(provider.name(), "openrouter");
    }

    #[test]
    fn test_local_vllm_example() {
        let mut config = ProviderConfig::test_config("meta-llama/Llama-2-7b-chat-hf");
        config.base_url = Some("http://localhost:8000/v1".to_string());
        config.api_key = None; // Local server may not need API key

        let provider = OpenAiCompatibleProvider::new("vllm".to_string(), config).unwrap();
        assert_eq!(provider.name(), "vllm");
    }
}
