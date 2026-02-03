/// AI Provider abstraction for Aether
///
/// This module provides a unified interface for different AI backends.
///
/// # Architecture
///
/// Providers are organized by **protocol** (not vendor):
///
/// - **OpenAI Protocol**: Handled by `HttpProvider` + `OpenAiProtocol` adapter
///   - Supports: OpenAI, DeepSeek, Moonshot, Doubao, T8Star, and any OpenAI-compatible API
///   - Configuration: Use presets (e.g., `deepseek`) or provide custom `base_url`
///
/// - **Anthropic Protocol**: Handled by `HttpProvider` + `AnthropicProtocol` adapter
///   - Supports: Claude (all models)
///   - Configuration: Use presets (`claude`, `anthropic`)
///
/// - **Gemini Protocol**: Handled by `HttpProvider` + `GeminiProtocol` adapter
///   - Supports: Google Gemini (all models)
///   - Configuration: Use presets (`gemini`, `google`)
///
/// - **Native Protocols**: Have dedicated implementations
///   - `OllamaProvider` - Local Ollama models
///
/// # Adding New Protocol-Compatible Providers
///
/// To add a new provider that uses an existing protocol:
/// 1. Add a preset to `presets.rs` with base_url, protocol, and color
/// 2. That's it! The factory will automatically route to `HttpProvider`
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::providers::{create_provider, AiProvider};
/// use aethecore::config::ProviderConfig;
///
/// // Create via preset (base_url auto-configured)
/// let config = ProviderConfig::test_config("deepseek-chat");
/// let provider = create_provider("deepseek", config)?;
///
/// // Or with custom base_url
/// let mut config = ProviderConfig::test_config("custom-model");
/// config.base_url = Some("https://my-api.example.com/v1".to_string());
/// let provider = create_provider("my-provider", config)?;
/// ```
use crate::error::Result;
use std::future::Future;
use std::pin::Pin;

// Sub-modules
pub mod auth_profile_registry;
pub mod auth_profiles;
pub mod failover;
pub mod gemini;
pub mod mock;
pub mod ollama;
pub mod openai;
pub mod anthropic;
pub mod profile_config;
pub mod profile_manager;
pub mod registry;
pub mod retry;
pub mod shared;
pub mod adapter;
pub mod http_provider;
pub mod presets;
pub mod protocols;

// Re-exports
pub use auth_profile_registry::{AuthProfileProviderRegistry, AuthProfileRegistryConfig};
pub use auth_profiles::{
    ApiKeyCredential, AuthProfileCredential, AuthProfileFailureReason,
    AuthProfileStore, CooldownConfig, OAuthCredential, ProfileUsageStats,
    TokenCredential, calculate_billing_cooldown_ms, calculate_cooldown_ms,
    clear_profile_cooldown, mark_profile_failure, mark_profile_good,
    mark_profile_used, normalize_provider_id, resolve_profile_order,
};
pub use failover::{FailoverConfig, FailoverProvider, ProviderEntry};
pub use mock::{MockError, MockProvider};
pub use ollama::OllamaProvider;
pub use profile_config::{
    ProfileConfig, ProfileConfigError, ProfileConfigResult, ProfilesConfig, ProfileTier,
};
pub use profile_manager::{
    AgentState, AuthProfileManager, EffectiveProfile, ProfileInfo, ProfileManagerError,
    ProfileManagerResult, ProfileOverride, ProfileUsage, RuntimeStatus,
};
pub use registry::ProviderRegistry;
pub use retry::retry_with_backoff;
pub use adapter::{ProtocolAdapter, RequestPayload};
pub use http_provider::HttpProvider;
pub use presets::{get_preset, ProviderPreset, PRESETS};
pub use protocols::OpenAiProtocol;

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::error::AetherError;
use std::sync::Arc;

/// Create a mock provider for testing
///
/// Returns an Arc<dyn AiProvider> wrapping a MockProvider with a default response.
/// This is useful for testing services that require an AiProvider.
pub fn create_mock_provider() -> Arc<dyn AiProvider> {
    Arc::new(MockProvider::new("Mock LLM response for testing"))
}

/// Create a provider instance from configuration
///
/// This factory function instantiates the appropriate provider based on
/// the protocol and preset configuration.
///
/// # Provider Resolution Order
///
/// 1. Check for preset providers by name (deepseek, moonshot, etc.)
/// 2. Apply preset defaults (base_url, protocol)
/// 3. Route to appropriate provider based on protocol
///
/// # Supported Protocols
///
/// - `"openai"` - OpenAI and OpenAI-compatible APIs (via HttpProvider)
/// - `"claude"` / `"anthropic"` - Anthropic Claude API (native)
/// - `"gemini"` - Google Gemini API (native)
/// - `"ollama"` - Local Ollama models (native)
pub fn create_provider(name: &str, mut config: ProviderConfig) -> Result<Arc<dyn AiProvider>> {
    let name_lower = name.to_lowercase();

    // 1. Apply preset configuration if available
    if let Some(preset) = presets::get_preset(&name_lower) {
        // Set base_url if not provided
        if config.base_url.is_none() || config.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            config.base_url = Some(preset.base_url.to_string());
        }
        // Set protocol if not provided
        if config.protocol.is_none() && config.provider_type.is_none() {
            config.protocol = Some(preset.protocol.to_string());
        }
        // Set color if default
        if config.color == "#808080" {
            config.color = preset.color.to_string();
        }
    }

    // 2. Determine protocol
    let protocol = config.protocol();

    // 3. Route based on protocol
    match protocol.as_str() {
        "openai" => {
            // Use new HttpProvider + OpenAiProtocol
            use std::time::Duration;

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds))
                .build()
                .map_err(|e| AetherError::invalid_config(format!("Failed to build HTTP client: {}", e)))?;

            let adapter = Arc::new(protocols::OpenAiProtocol::new(client));
            let provider = HttpProvider::new(name.to_string(), config, adapter)?;
            Ok(Arc::new(provider))
        }

        "claude" | "anthropic" => {
            // Use HttpProvider + AnthropicProtocol
            use std::time::Duration;

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds))
                .build()
                .map_err(|e| AetherError::invalid_config(format!("Failed to build HTTP client: {}", e)))?;

            let adapter = Arc::new(protocols::AnthropicProtocol::new(client));
            let provider = HttpProvider::new(name.to_string(), config, adapter)?;
            Ok(Arc::new(provider))
        }
        "gemini" => {
            // Use HttpProvider + GeminiProtocol
            use std::time::Duration;

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds))
                .build()
                .map_err(|e| AetherError::invalid_config(format!("Failed to build HTTP client: {}", e)))?;

            let adapter = Arc::new(protocols::GeminiProtocol::new(client));
            let provider = HttpProvider::new(name.to_string(), config, adapter)?;
            Ok(Arc::new(provider))
        }
        "ollama" => {
            let provider = OllamaProvider::new(name.to_string(), config)?;
            Ok(Arc::new(provider))
        }
        "mock" => {
            let provider = MockProvider::new("Mock response".to_string());
            Ok(Arc::new(provider))
        }

        unknown => Err(AetherError::invalid_config(format!(
            "Unknown protocol: '{}'. Supported: openai, claude, anthropic, gemini, ollama, mock.",
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
    /// ```rust,ignore
    /// # use aethecore::providers::AiProvider;
    /// # async fn example(provider: &dyn AiProvider) {
    /// let response = provider.process(
    ///     "Translate to French: Hello",
    ///     Some("You are a translator")
    /// ).await.unwrap();
    /// # }
    /// ```
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>>;

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
    fn process_with_image(
        &self,
        input: &str,
        _image: Option<&crate::clipboard::ImageData>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // Clone the data we need before moving into async block
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            // Default: ignore image and call text-only process
            self.process(&input, system_prompt.as_deref()).await
        })
    }

    /// Process input with MediaAttachment and return AI-generated response
    ///
    /// This is the preferred method for multimodal content as it supports
    /// the new MediaAttachment type from add-multimodal-content-support.
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to process
    /// * `attachments` - Optional media attachments (images, etc.)
    /// * `system_prompt` - Optional system prompt to guide AI behavior
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - The AI-generated response text
    /// * `Err(AetherError)` - Various errors (same as `process()`)
    ///
    /// # Default Implementation
    ///
    /// Default implementation calls `process()` and ignores attachments.
    /// Vision-capable providers should override this method.
    fn process_with_attachments(
        &self,
        input: &str,
        _attachments: Option<&[crate::core::MediaAttachment]>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // Clone the data we need before moving into async block
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            // Default: ignore attachments and call text-only process
            self.process(&input, system_prompt.as_deref()).await
        })
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
    /// ```rust,ignore
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
    /// ```rust,ignore
    /// # use aethecore::providers::AiProvider;
    /// # fn example(provider: &dyn AiProvider) {
    /// let color = provider.color();
    /// assert!(color.starts_with('#'));
    /// # }
    /// ```
    fn color(&self) -> &str;

    /// Process input with explicit mode control for system prompt handling.
    ///
    /// This method allows forcing "standard" mode for system prompts.
    /// When `force_standard_mode` is true, the system prompt is sent as a
    /// separate system role message, regardless of the provider's configured
    /// `system_prompt_mode` setting.
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to process
    /// * `system_prompt` - Optional system prompt to guide AI behavior
    /// * `force_standard_mode` - If true, always use system role message
    ///
    /// # Default Implementation
    ///
    /// Default implementation ignores `force_standard_mode` and calls `process()`.
    fn process_with_mode(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        _force_standard_mode: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        self.process(input, system_prompt)
    }

    /// Process input with thinking level configuration
    ///
    /// This method enables extended thinking/reasoning for supported models.
    /// The actual implementation depends on the provider:
    /// - Anthropic: Uses `thinking` block with `budget_tokens`
    /// - OpenAI: Uses `reasoning_effort` for o1/o3 models
    /// - Gemini: Uses `thinking_config` or `thinking_level`
    /// - Other: Falls back to standard processing
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to process
    /// * `system_prompt` - Optional system prompt to guide AI behavior
    /// * `think_level` - Thinking level to use (Off, Minimal, Low, Medium, High, XHigh)
    ///
    /// # Default Implementation
    ///
    /// Default implementation ignores `think_level` and calls `process()`.
    /// Providers that support thinking should override this method.
    fn process_with_thinking(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        _think_level: ThinkLevel,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        // Default: ignore thinking level and use standard processing
        self.process(input, system_prompt)
    }

    /// Check if provider supports extended thinking
    ///
    /// # Returns
    ///
    /// * `true` if provider supports thinking level control
    /// * `false` if provider only supports standard processing
    ///
    /// # Default Implementation
    ///
    /// Default returns `false`. Providers with thinking support should override.
    fn supports_thinking(&self) -> bool {
        false
    }

    /// Get maximum supported thinking level for this provider/model
    ///
    /// # Returns
    ///
    /// The highest thinking level this provider supports.
    /// Default is `ThinkLevel::Off` (no thinking support).
    fn max_think_level(&self) -> ThinkLevel {
        ThinkLevel::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Simple test implementation to verify trait can be used as trait object
    struct TestProvider;

    impl AiProvider for TestProvider {
        fn process(
            &self,
            input: &str,
            _system_prompt: Option<&str>,
        ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
            let response = format!("Echo: {}", input);
            Box::pin(async move { Ok(response) })
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
        let config = ProviderConfig::test_config("gpt-4o");

        let provider = create_provider("openai", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "openai");
    }

    #[test]
    fn test_create_claude_provider() {
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
        config.provider_type = Some("claude".to_string());

        let provider = create_provider("claude", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "claude");
    }

    #[test]
    fn test_create_gemini_provider() {
        let mut config = ProviderConfig::test_config("gemini-1.5-flash");
        config.provider_type = Some("gemini".to_string());

        let provider = create_provider("gemini", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "gemini");
    }

    #[test]
    fn test_create_ollama_provider() {
        let mut config = ProviderConfig::test_config("llama3.2");
        config.provider_type = Some("ollama".to_string());
        config.api_key = None;
        config.timeout_seconds = 60;

        let provider = create_provider("ollama", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "ollama");
    }

    #[test]
    fn test_create_custom_openai_compatible_provider() {
        // DeepSeek as example
        let mut config = ProviderConfig::test_config("deepseek-chat");
        config.provider_type = Some("openai".to_string());
        config.base_url = Some("https://api.deepseek.com".to_string());

        let provider = create_provider("deepseek", config);
        assert!(provider.is_ok());
        // OpenAI provider implementation is used for DeepSeek, but keeps custom name
        assert_eq!(provider.unwrap().name(), "deepseek");
    }

    #[test]
    fn test_infer_provider_type_explicit() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.provider_type = Some("claude".to_string());

        // Explicit provider_type should take precedence
        assert_eq!(config.infer_provider_type("openai"), "claude");
    }

    #[test]
    fn test_infer_provider_type_from_name() {
        let mut config = ProviderConfig::test_config("model");
        config.provider_type = None;

        // Infer from name
        assert_eq!(config.infer_provider_type("openai"), "openai");
        assert_eq!(config.infer_provider_type("claude"), "claude");
        assert_eq!(config.infer_provider_type("gemini"), "gemini");
        assert_eq!(config.infer_provider_type("google"), "gemini");
        assert_eq!(config.infer_provider_type("ollama"), "ollama");
        assert_eq!(config.infer_provider_type("deepseek"), "openai");
        assert_eq!(config.infer_provider_type("moonshot"), "openai");
    }

    #[test]
    fn test_infer_provider_type_case_insensitive() {
        let mut config = ProviderConfig::test_config("model");
        config.provider_type = None;
        config.api_key = None;

        // Case insensitive inference
        assert_eq!(config.infer_provider_type("CLAUDE"), "claude");
        assert_eq!(config.infer_provider_type("Claude"), "claude");
        assert_eq!(config.infer_provider_type("OLLAMA"), "ollama");
    }

    #[test]
    fn test_create_unknown_provider_type() {
        let mut config = ProviderConfig::test_config("model");
        config.provider_type = Some("unknown".to_string());

        let result = create_provider("test", config);
        assert!(result.is_err());
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_multiple_custom_providers() {
        // Simulate configuring multiple custom providers
        let mut deepseek_config = ProviderConfig::test_config("deepseek-chat");
        deepseek_config.provider_type = Some("openai".to_string());
        deepseek_config.base_url = Some("https://api.deepseek.com".to_string());

        let mut moonshot_config = ProviderConfig::test_config("moonshot-v1-8k");
        moonshot_config.provider_type = Some("openai".to_string());
        moonshot_config.base_url = Some("https://api.moonshot.cn/v1".to_string());
        moonshot_config.max_tokens = Some(8192);

        let deepseek = create_provider("deepseek", deepseek_config);
        let moonshot = create_provider("moonshot", moonshot_config);

        assert!(deepseek.is_ok());
        assert!(moonshot.is_ok());
        // Both use OpenAI provider implementation, but keep their custom names
        assert_eq!(deepseek.unwrap().name(), "deepseek");
        assert_eq!(moonshot.unwrap().name(), "moonshot");
    }
}
