/// DeepSeek AI provider (thin wrapper over OpenAiProvider)
///
/// DeepSeek provides OpenAI-compatible API with competitive pricing and performance.
/// This wrapper pre-configures DeepSeek-specific defaults so users don't need to
/// manually set base_url.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: DeepSeek API key (from https://platform.deepseek.com)
/// - `model`: Model name (e.g., "deepseek-chat", "deepseek-coder")
///
/// Optional fields are inherited from OpenAiProvider:
/// - `timeout_seconds`: Request timeout (defaults to 300)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::deepseek::DeepSeekProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("sk-...".to_string()),
///     model: "deepseek-chat".to_string(),
///     base_url: None, // Auto-configured to https://api.deepseek.com
///     ..Default::default()
/// };
///
/// let provider = DeepSeekProvider::new("deepseek".to_string(), config)?;
/// let response = provider.process("Hello!", Some("You are helpful")).await?;
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::{AiProvider, OpenAiProvider};
use std::future::Future;
use std::pin::Pin;

/// DeepSeek AI provider (wrapper)
pub struct DeepSeekProvider {
    inner: OpenAiProvider,
}

impl DeepSeekProvider {
    /// Create new DeepSeek provider with auto-configured endpoint
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (typically "deepseek")
    /// * `config` - Provider configuration (base_url will be auto-set if not provided)
    ///
    /// # Returns
    ///
    /// * `Ok(DeepSeekProvider)` - Successfully initialized provider
    /// * `Err(AetherError)` - Configuration validation failed
    pub fn new(name: String, mut config: ProviderConfig) -> Result<Self> {
        // Auto-configure DeepSeek base URL if not provided
        if config.base_url.is_none() || config.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            config.base_url = Some("https://api.deepseek.com".to_string());
        }

        Ok(Self {
            inner: OpenAiProvider::new(name, config)?,
        })
    }
}

// Delegate all AiProvider methods to inner OpenAiProvider
impl AiProvider for DeepSeekProvider {
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
    fn test_auto_configure_base_url() {
        let config = ProviderConfig::test_config("deepseek-chat");
        let provider = DeepSeekProvider::new("deepseek".to_string(), config).unwrap();

        // Verify that the provider was created successfully
        assert_eq!(provider.name(), "deepseek");
    }

    #[test]
    fn test_respect_custom_base_url() {
        let mut config = ProviderConfig::test_config("deepseek-chat");
        config.base_url = Some("https://custom.deepseek.com".to_string());

        let provider = DeepSeekProvider::new("deepseek".to_string(), config).unwrap();
        assert_eq!(provider.name(), "deepseek");
    }
}
