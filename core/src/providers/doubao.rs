/// Volcengine Doubao provider (thin wrapper over OpenAiProvider)
///
/// Doubao (豆包) is ByteDance's LLM service, providing OpenAI-compatible API
/// through Volcengine ARK platform. This wrapper pre-configures Volcengine-specific
/// defaults including the v3 API endpoint.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: Volcengine API key in UUID format (from https://console.volcengine.com/ark)
/// - `model`: Model endpoint ID (e.g., "doubao-1-5-pro-32k", not the display name)
///
/// Optional fields are inherited from OpenAiProvider:
/// - `timeout_seconds`: Request timeout (defaults to 30)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
///
/// # Important Notes
///
/// - Volcengine uses **v3 API** (not v1 like OpenAI)
/// - API key format is UUID (e.g., "bc4184d8-eb4d-418c-9752-190d2e4eca6d")
/// - Model name should be the endpoint ID from Volcengine console
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::doubao::DoubaoProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("bc4184d8-eb4d-418c-9752-190d2e4eca6d".to_string()),
///     model: "doubao-1-5-pro-32k".to_string(),
///     base_url: None, // Auto-configured to https://ark.cn-beijing.volces.com/api/v3
///     ..Default::default()
/// };
///
/// let provider = DoubaoProvider::new("doubao".to_string(), config)?;
/// let response = provider.process("你好!", Some("你是豆包助手")).await?;
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::{AiProvider, OpenAiProvider};
use std::future::Future;
use std::pin::Pin;
use tracing::info;

/// Volcengine Doubao provider (wrapper)
pub struct DoubaoProvider {
    inner: OpenAiProvider,
}

impl DoubaoProvider {
    /// Create new Doubao provider with auto-configured v3 endpoint
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (typically "doubao", "volcengine", or "ark")
    /// * `config` - Provider configuration (base_url will be auto-set if not provided)
    ///
    /// # Returns
    ///
    /// * `Ok(DoubaoProvider)` - Successfully initialized provider
    /// * `Err(AetherError)` - Configuration validation failed
    ///
    /// # Notes
    ///
    /// - Automatically sets base_url to Volcengine ARK v3 API endpoint
    /// - Validates API key format (should be UUID-like)
    pub fn new(name: String, mut config: ProviderConfig) -> Result<Self> {
        // Auto-configure Volcengine/Doubao base URL if not provided
        // Use v3 API endpoint (not v1 like OpenAI)
        if config.base_url.is_none() || config.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            config.base_url = Some("https://ark.cn-beijing.volces.com/api/v3".to_string());
            info!(
                provider = %name,
                endpoint = "https://ark.cn-beijing.volces.com/api/v3",
                "Auto-configured Volcengine ARK v3 endpoint"
            );
        }

        // Optional: Validate API key format (UUID-like)
        if let Some(ref api_key) = config.api_key {
            if !api_key.is_empty() && !api_key.contains('-') {
                tracing::warn!(
                    provider = %name,
                    "Volcengine API key should be in UUID format (e.g., 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx'). Current format may be incorrect."
                );
            }
        }

        Ok(Self {
            inner: OpenAiProvider::new(name, config)?,
        })
    }
}

// Delegate all AiProvider methods to inner OpenAiProvider
impl AiProvider for DoubaoProvider {
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
        let config = ProviderConfig::test_config("doubao-1-5-pro-32k");
        let provider = DoubaoProvider::new("doubao".to_string(), config).unwrap();

        // Verify that the provider was created successfully
        assert_eq!(provider.name(), "doubao");
    }

    #[test]
    fn test_respect_custom_base_url() {
        let mut config = ProviderConfig::test_config("doubao-1-5-pro-32k");
        config.base_url = Some("https://custom.volces.com/api/v3".to_string());

        let provider = DoubaoProvider::new("doubao".to_string(), config).unwrap();
        assert_eq!(provider.name(), "doubao");
    }

    #[test]
    fn test_volcengine_alias() {
        let config = ProviderConfig::test_config("doubao-1-5-pro-32k");
        let provider = DoubaoProvider::new("volcengine".to_string(), config).unwrap();
        assert_eq!(provider.name(), "volcengine");
    }

    #[test]
    fn test_ark_alias() {
        let config = ProviderConfig::test_config("doubao-1-5-pro-32k");
        let provider = DoubaoProvider::new("ark".to_string(), config).unwrap();
        assert_eq!(provider.name(), "ark");
    }
}
