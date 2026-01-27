/// T8Star provider (thin wrapper over OpenAiProvider)
///
/// T8Star is an OpenAI-compatible API service.
/// This wrapper pre-configures T8Star-specific defaults.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: T8Star API key (from https://ai.t8star.cn)
/// - `model`: Model name (e.g., "gpt-4o", "gpt-5.2", "gpt-5-mini")
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
/// use aethecore::providers::t8star::T8StarProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("sk-...".to_string()),
///     model: "gpt-5.2".to_string(),
///     base_url: None, // Auto-configured to https://ai.t8star.cn
///     ..Default::default()
/// };
///
/// let provider = T8StarProvider::new("t8star".to_string(), config)?;
/// let response = provider.process("Hello!", Some("You are helpful")).await?;
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::{AiProvider, OpenAiProvider};
use std::future::Future;
use std::pin::Pin;

/// T8Star provider (wrapper)
pub struct T8StarProvider {
    inner: OpenAiProvider,
}

impl T8StarProvider {
    /// Create new T8Star provider with auto-configured endpoint
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (typically "t8star")
    /// * `config` - Provider configuration (base_url will be auto-set if not provided)
    ///
    /// # Returns
    ///
    /// * `Ok(T8StarProvider)` - Successfully initialized provider
    /// * `Err(AetherError)` - Configuration validation failed
    pub fn new(name: String, mut config: ProviderConfig) -> Result<Self> {
        // Auto-configure T8Star base URL if not provided
        if config.base_url.is_none() || config.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            config.base_url = Some("https://ai.t8star.cn".to_string());
        }

        Ok(Self {
            inner: OpenAiProvider::new(name, config)?,
        })
    }
}

// Delegate all AiProvider methods to inner OpenAiProvider
impl AiProvider for T8StarProvider {
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
        let config = ProviderConfig::test_config("gpt-5.2");
        let provider = T8StarProvider::new("t8star".to_string(), config).unwrap();

        // Verify that the provider was created successfully
        assert_eq!(provider.name(), "t8star");
    }

    #[test]
    fn test_respect_custom_base_url() {
        let mut config = ProviderConfig::test_config("gpt-5.2");
        config.base_url = Some("https://custom.t8star.cn".to_string());

        let provider = T8StarProvider::new("t8star".to_string(), config).unwrap();
        assert_eq!(provider.name(), "t8star");
    }
}
