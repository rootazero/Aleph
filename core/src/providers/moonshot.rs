/// Moonshot AI (Kimi) provider (thin wrapper over OpenAiProvider)
///
/// Moonshot AI provides OpenAI-compatible API for their Kimi models,
/// featuring long context windows and strong Chinese language support.
/// This wrapper pre-configures Moonshot-specific defaults.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: Moonshot API key (from https://platform.moonshot.cn)
/// - `model`: Model name (e.g., "moonshot-v1-8k", "moonshot-v1-32k", "moonshot-v1-128k")
///
/// Optional fields are inherited from OpenAiProvider:
/// - `timeout_seconds`: Request timeout (defaults to 30)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::moonshot::MoonshotProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("sk-...".to_string()),
///     model: "moonshot-v1-8k".to_string(),
///     base_url: None, // Auto-configured to https://api.moonshot.cn/v1
///     ..Default::default()
/// };
///
/// let provider = MoonshotProvider::new("moonshot".to_string(), config)?;
/// let response = provider.process("你好，Kimi!", Some("你是 Kimi 智能助手")).await?;
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::{AiProvider, OpenAiProvider};
use std::future::Future;
use std::pin::Pin;

/// Moonshot AI (Kimi) provider (wrapper)
pub struct MoonshotProvider {
    inner: OpenAiProvider,
}

impl MoonshotProvider {
    /// Create new Moonshot provider with auto-configured endpoint
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (typically "moonshot" or "kimi")
    /// * `config` - Provider configuration (base_url will be auto-set if not provided)
    ///
    /// # Returns
    ///
    /// * `Ok(MoonshotProvider)` - Successfully initialized provider
    /// * `Err(AetherError)` - Configuration validation failed
    pub fn new(name: String, mut config: ProviderConfig) -> Result<Self> {
        // Auto-configure Moonshot base URL if not provided
        if config.base_url.is_none() || config.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            config.base_url = Some("https://api.moonshot.cn/v1".to_string());
        }

        Ok(Self {
            inner: OpenAiProvider::new(name, config)?,
        })
    }
}

// Delegate all AiProvider methods to inner OpenAiProvider
impl AiProvider for MoonshotProvider {
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
        let config = ProviderConfig::test_config("moonshot-v1-8k");
        let provider = MoonshotProvider::new("moonshot".to_string(), config).unwrap();

        // Verify that the provider was created successfully
        assert_eq!(provider.name(), "moonshot");
    }

    #[test]
    fn test_respect_custom_base_url() {
        let mut config = ProviderConfig::test_config("moonshot-v1-8k");
        config.base_url = Some("https://custom.moonshot.cn/v1".to_string());

        let provider = MoonshotProvider::new("moonshot".to_string(), config).unwrap();
        assert_eq!(provider.name(), "moonshot");
    }

    #[test]
    fn test_kimi_alias() {
        let config = ProviderConfig::test_config("moonshot-v1-8k");
        let provider = MoonshotProvider::new("kimi".to_string(), config).unwrap();
        assert_eq!(provider.name(), "kimi");
    }
}
