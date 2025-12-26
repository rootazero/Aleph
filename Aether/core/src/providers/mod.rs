/// AI Provider abstraction for Aether
///
/// This module defines the `AiProvider` trait which provides a unified interface
/// for different AI backends (OpenAI, Claude, Ollama, etc.).
///
/// # Architecture
///
/// All AI providers implement the `AiProvider` trait, which defines:
/// - `process()`: Async method to process input and return AI response
/// - `name()`: Provider identifier (e.g., "openai", "claude")
/// - `color()`: Provider brand color for UI (e.g., "#10a37f")
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::providers::AiProvider;
/// use std::sync::Arc;
///
/// async fn example(provider: Arc<dyn AiProvider>) {
///     let response = provider.process(
///         "Hello, AI!",
///         Some("You are a helpful assistant")
///     ).await.unwrap();
///
///     println!("Provider: {}", provider.name());
///     println!("Response: {}", response);
/// }
/// ```
use crate::error::Result;
use async_trait::async_trait;

// Sub-modules
pub mod claude;
pub mod mock;
pub mod ollama;
pub mod openai;
pub mod registry;
pub mod retry;

// Re-exports
pub use claude::ClaudeProvider;
pub use mock::{MockError, MockProvider};
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use registry::ProviderRegistry;
pub use retry::retry_with_backoff;

use crate::config::ProviderConfig;
use crate::error::AetherError;
use std::sync::Arc;

/// Create a provider instance from configuration
///
/// This factory function instantiates the appropriate provider based on
/// the `provider_type` field in the configuration.
///
/// # Arguments
///
/// * `name` - Provider name (e.g., "openai", "deepseek", "claude")
/// * `config` - Provider configuration
///
/// # Returns
///
/// * `Ok(Arc<dyn AiProvider>)` - Successfully created provider
/// * `Err(AetherError)` - Invalid configuration or unknown provider type
///
/// # Supported Provider Types
///
/// - `"openai"` - OpenAI and OpenAI-compatible APIs (DeepSeek, Moonshot, etc.)
/// - `"claude"` - Anthropic Claude API
/// - `"ollama"` - Local Ollama models
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::create_provider;
///
/// # fn example() -> aethecore::error::Result<()> {
/// // Create OpenAI provider
/// let openai_config = ProviderConfig {
///     provider_type: Some("openai".to_string()),
///     api_key: Some("sk-...".to_string()),
///     model: "gpt-4o".to_string(),
///     base_url: None,
///     color: "#10a37f".to_string(),
///     timeout_seconds: 30,
///     max_tokens: Some(4096),
///     temperature: Some(0.7),
/// };
/// let provider = create_provider("openai", openai_config)?;
///
/// // Create custom OpenAI-compatible provider (DeepSeek)
/// let deepseek_config = ProviderConfig {
///     provider_type: Some("openai".to_string()),
///     api_key: Some("sk-...".to_string()),
///     model: "deepseek-chat".to_string(),
///     base_url: Some("https://api.deepseek.com".to_string()),
///     color: "#0066cc".to_string(),
///     timeout_seconds: 30,
///     max_tokens: Some(4096),
///     temperature: Some(0.7),
/// };
/// let deepseek = create_provider("deepseek", deepseek_config)?;
/// # Ok(())
/// # }
/// ```
pub fn create_provider(name: &str, config: ProviderConfig) -> Result<Arc<dyn AiProvider>> {
    let provider_type = config.infer_provider_type(name);

    match provider_type.as_str() {
        "openai" => {
            let provider = OpenAiProvider::new(config)?;
            Ok(Arc::new(provider))
        }
        "claude" => {
            let provider = ClaudeProvider::new(config)?;
            Ok(Arc::new(provider))
        }
        "ollama" => {
            let provider = OllamaProvider::new(config)?;
            Ok(Arc::new(provider))
        }
        "mock" => {
            // Mock provider for testing
            let provider = MockProvider::new("Mock response".to_string());
            Ok(Arc::new(provider))
        }
        unknown => Err(AetherError::invalid_config(format!(
            "Unknown provider type: '{}'. Supported types: openai, claude, ollama, mock",
            unknown
        ))),
    }
}

/// Unified interface for AI providers
///
/// All AI backends (OpenAI, Claude, Ollama, etc.) implement this trait
/// to provide a consistent API for processing user input.
///
/// # Thread Safety
///
/// The trait extends `Send + Sync` to ensure providers can be safely shared
/// across async tasks and stored in `Arc<dyn AiProvider>`.
///
/// # Async Design
///
/// All processing is async to avoid blocking the runtime during API calls
/// or command execution.
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Process input text and return AI-generated response
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to process
    /// * `system_prompt` - Optional system prompt to guide AI behavior
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - The AI-generated response text
    /// * `Err(AetherError)` - Various errors:
    ///   - `NetworkError`: Network connectivity issues
    ///   - `AuthenticationError`: Invalid API key
    ///   - `RateLimitError`: Too many requests
    ///   - `ProviderError`: API returned error response
    ///   - `Timeout`: Request exceeded timeout
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::providers::AiProvider;
    /// # async fn example(provider: &dyn AiProvider) {
    /// let response = provider.process(
    ///     "Translate to French: Hello",
    ///     Some("You are a translator")
    /// ).await.unwrap();
    /// # }
    /// ```
    async fn process(&self, input: &str, system_prompt: Option<&str>) -> Result<String>;

    /// Process input with optional image and return AI-generated response
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to process
    /// * `image` - Optional image data
    /// * `system_prompt` - Optional system prompt to guide AI behavior
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - The AI-generated response text
    /// * `Err(AetherError)` - Various errors (same as `process()`)
    ///
    /// # Default Implementation
    ///
    /// Default implementation calls `process()` and ignores the image.
    /// Vision-capable providers should override this method.
    async fn process_with_image(
        &self,
        input: &str,
        _image: Option<&crate::clipboard::ImageData>,
        system_prompt: Option<&str>,
    ) -> Result<String> {
        // Default: ignore image and call text-only process
        self.process(input, system_prompt).await
    }

    /// Check if provider supports vision/image input
    ///
    /// # Returns
    ///
    /// * `true` if provider can process images (e.g., GPT-4V, Claude 3 Opus)
    /// * `false` if provider only supports text
    ///
    /// # Default Implementation
    ///
    /// Default returns `false`. Vision-capable providers should override.
    fn supports_vision(&self) -> bool {
        false
    }

    /// Get provider name for logging and routing
    ///
    /// # Returns
    ///
    /// Provider identifier (e.g., "openai", "claude", "ollama")
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::providers::AiProvider;
    /// # fn example(provider: &dyn AiProvider) {
    /// assert_eq!(provider.name(), "openai");
    /// # }
    /// ```
    fn name(&self) -> &str;

    /// Get provider brand color for UI theming
    ///
    /// # Returns
    ///
    /// Hex color string (e.g., "#10a37f" for OpenAI green)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::providers::AiProvider;
    /// # fn example(provider: &dyn AiProvider) {
    /// let color = provider.color();
    /// assert!(color.starts_with('#'));
    /// # }
    /// ```
    fn color(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Simple test implementation to verify trait can be used as trait object
    struct TestProvider;

    #[async_trait]
    impl AiProvider for TestProvider {
        async fn process(&self, input: &str, _system_prompt: Option<&str>) -> Result<String> {
            Ok(format!("Echo: {}", input))
        }

        fn name(&self) -> &str {
            "test"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[tokio::test]
    async fn test_provider_trait_object() {
        let provider: Arc<dyn AiProvider> = Arc::new(TestProvider);

        // Test process method
        let response = provider.process("hello", None).await.unwrap();
        assert_eq!(response, "Echo: hello");

        // Test metadata methods
        assert_eq!(provider.name(), "test");
        assert_eq!(provider.color(), "#000000");
    }

    #[tokio::test]
    async fn test_provider_with_system_prompt() {
        let provider: Arc<dyn AiProvider> = Arc::new(TestProvider);
        let response = provider
            .process("input", Some("system prompt"))
            .await
            .unwrap();
        assert_eq!(response, "Echo: input");
    }

    #[test]
    fn test_provider_is_send_sync() {
        // This test ensures AiProvider can be used across threads
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn AiProvider>>();
    }

    // Factory function tests
    #[test]
    fn test_create_openai_provider() {
        let config = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        let provider = create_provider("openai", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "openai");
    }

    #[test]
    fn test_create_claude_provider() {
        let config = ProviderConfig {
            provider_type: Some("claude".to_string()),
            api_key: Some("sk-ant-test".to_string()),
            model: "claude-3-5-sonnet-20241022".to_string(),
            base_url: None,
            color: "#d97757".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        let provider = create_provider("claude", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "claude");
    }

    #[test]
    fn test_create_ollama_provider() {
        let config = ProviderConfig {
            provider_type: Some("ollama".to_string()),
            api_key: None,
            model: "llama3.2".to_string(),
            base_url: None,
            color: "#0000ff".to_string(),
            timeout_seconds: 60,
            max_tokens: None,
            temperature: None,
        };

        let provider = create_provider("ollama", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "ollama");
    }

    #[test]
    fn test_create_custom_openai_compatible_provider() {
        // DeepSeek as example
        let config = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-deepseek-test".to_string()),
            model: "deepseek-chat".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            color: "#0066cc".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        let provider = create_provider("deepseek", config);
        assert!(provider.is_ok());
        // OpenAI provider is used for DeepSeek
        assert_eq!(provider.unwrap().name(), "openai");
    }

    #[test]
    fn test_infer_provider_type_explicit() {
        let config = ProviderConfig {
            provider_type: Some("claude".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        // Explicit provider_type should take precedence
        assert_eq!(config.infer_provider_type("openai"), "claude");
    }

    #[test]
    fn test_infer_provider_type_from_name() {
        let config = ProviderConfig {
            provider_type: None,
            api_key: Some("sk-test".to_string()),
            model: "model".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        };

        // Infer from name
        assert_eq!(config.infer_provider_type("openai"), "openai");
        assert_eq!(config.infer_provider_type("claude"), "claude");
        assert_eq!(config.infer_provider_type("ollama"), "ollama");
        assert_eq!(config.infer_provider_type("deepseek"), "openai");
        assert_eq!(config.infer_provider_type("moonshot"), "openai");
    }

    #[test]
    fn test_infer_provider_type_case_insensitive() {
        let config = ProviderConfig {
            provider_type: None,
            api_key: None,
            model: "model".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        };

        // Case insensitive inference
        assert_eq!(config.infer_provider_type("CLAUDE"), "claude");
        assert_eq!(config.infer_provider_type("Claude"), "claude");
        assert_eq!(config.infer_provider_type("OLLAMA"), "ollama");
    }

    #[test]
    fn test_create_unknown_provider_type() {
        let config = ProviderConfig {
            provider_type: Some("unknown".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "model".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        };

        let result = create_provider("test", config);
        assert!(result.is_err());
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_multiple_custom_providers() {
        // Simulate configuring multiple custom providers
        let deepseek_config = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-deepseek".to_string()),
            model: "deepseek-chat".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            color: "#0066cc".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };

        let moonshot_config = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-moonshot".to_string()),
            model: "moonshot-v1-8k".to_string(),
            base_url: Some("https://api.moonshot.cn/v1".to_string()),
            color: "#ff6b6b".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(8192),
            temperature: Some(0.7),
        };

        let deepseek = create_provider("deepseek", deepseek_config);
        let moonshot = create_provider("moonshot", moonshot_config);

        assert!(deepseek.is_ok());
        assert!(moonshot.is_ok());
        assert_eq!(deepseek.unwrap().name(), "openai");
        assert_eq!(moonshot.unwrap().name(), "openai");
    }
}
