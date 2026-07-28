/// AI Provider abstraction for Aleph
///
/// This module provides a unified interface for different AI backends.
///
/// # Architecture
///
/// Providers are organized by **protocol** (not vendor), using a registry-based system:
///
/// - **`ProtocolRegistry`**: Central registry for all protocol adapters
///   - Built-in protocols: `OpenAI`, Anthropic, Gemini
///   - Dynamic protocols: Loaded from YAML configurations at runtime
///
/// - **`OpenAI` Protocol**: Handled by `HttpProvider` + `OpenAiProtocol` adapter
///   - Supports: `OpenAI`, `DeepSeek`, Moonshot, Doubao, `T8Star`, and any OpenAI-compatible API
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
/// - **`OpenAI` Responses Protocol**: Handled by `HttpProvider` + `OpenAiResponsesProtocol` adapter
///   - Supports: `OpenAI` /v1/responses API and compatible relay providers (`OpenRouter`, etc.)
///   - Configuration: Use presets (e.g., `openrouter`) or set `protocol: "openai-responses"`
///
/// - **Native Protocols**: Have dedicated implementations
///   - `OllamaProvider` - Local Ollama models
///
/// # Adding New Protocol-Compatible Providers
///
/// To add a new provider that uses an existing protocol:
/// 1. Add a preset to `presets.rs` with `base_url`, protocol, and color
/// 2. That's it! The factory will automatically route to `HttpProvider` via the registry
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::providers::{create_provider, AiProvider};
/// use alephcore::config::ProviderConfig;
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
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

// Sub-modules
pub mod adapter;
pub mod anthropic;
pub mod bridge;
pub mod capability_gate;
pub mod catalog;
pub mod codex;
pub mod default_handle;
pub mod delta;
pub mod failover;
pub mod gemini;
pub mod health;
pub mod http_provider;
pub mod llm_retry;
pub mod load_stats;
pub mod message;
pub mod metadata;
pub mod metering;
pub mod moa;
pub mod mock;
pub mod model_behaviors;
pub mod model_catalog;
pub mod model_override_provider;
pub mod ollama;
pub mod openai;
pub mod presets;
pub mod probe;
pub mod protocols;
#[cfg(any(test, feature = "test-helpers"))]
pub mod recording_mock;
pub mod registry;
pub mod responses;
pub mod retry;
pub mod route_handle;
pub mod route_observe;
pub mod route_policy;
pub mod route_witness;
pub mod session_moa_handle;
pub mod session_model_handle;
pub mod think_level_provider;

// Re-exports
pub use adapter::{
    NativeToolCall, ProtocolAdapter, ProviderResponse, RequestPayload, StopReason, TokenUsage,
};
pub use default_handle::{DefaultProviderHandle, StaticDefault};
pub use delta::{
    response_to_delta_stream, DeltaCollector, DeltaSink, IndexIdTracker, NoopSink, ProviderDelta,
};
pub use failover::{
    FailoverConfig, FailoverHealth, FailoverNode, FailoverProvider, ModelCooldown, ProviderCooldown,
};
pub use health::{ModelInfo, ProviderError};
pub use http_provider::HttpProvider;
pub use load_stats::{InFlightGuard, LoadStats};
pub use metering::MeteringProvider;
pub use mock::{MockError, MockProvider};
pub use model_catalog::{
    capabilities_for, endpoint_kind_for_base_url, infer_vendor, EndpointKind, ModelCapabilities,
};
pub use model_override_provider::ModelOverrideProvider;
pub use ollama::OllamaProvider;
pub use presets::{get_preset, resolve_provider_from_model, ProviderPreset, PRESETS};
pub use protocols::OpenAiProtocol;
pub use registry::ProviderRegistry;
pub use think_level_provider::ThinkLevelProvider;

use crate::config::ProviderConfig;
use crate::error::AlephError;
use crate::sync_primitives::Arc;

/// Create a mock provider for testing
///
/// Returns an Arc<dyn AiProvider> wrapping a `MockProvider` with a default response.
/// This is useful for testing services that require an `AiProvider`.
#[must_use]
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
/// 2. Apply preset defaults (`base_url`, protocol)
/// 3. Route to appropriate provider based on protocol
///
/// # Supported Protocols
///
/// - `"openai"` - `OpenAI` and OpenAI-compatible APIs (via `HttpProvider`)
/// - `"claude"` / `"anthropic"` - Anthropic Claude API (native)
/// - `"gemini"` - Google Gemini API (native)
/// - `"codex"` - Codex Responses API / `ChatGPT` subscription backend (via OAuth)
/// - `"openai-responses"` - `OpenAI` Responses API (via `HttpProvider`), for `OpenRouter` etc.
/// - `"ollama"` - Local Ollama models (native)
pub fn create_provider(name: &str, mut config: ProviderConfig) -> Result<Arc<dyn AiProvider>> {
    // Get the global protocol registry (built-in protocols registered at init via Lazy)
    use crate::providers::protocols::ProtocolRegistry;
    let registry = ProtocolRegistry::global();

    let name_lower = name.to_lowercase();

    // 1. Apply preset configuration if available
    if let Some(preset) = presets::get_preset(&name_lower) {
        // Set base_url if not provided
        if config.base_url.is_none() || config.base_url.as_ref().is_some_and(|s| s.is_empty()) {
            config.base_url = Some(preset.base_url.to_string());
        }
        // Set protocol if not provided
        if config.protocol.is_none() {
            config.protocol = Some(preset.protocol.to_string());
        }
        // Set color if default
        if config.color == "#808080" {
            config.color = preset.color.to_string();
        }
    }

    // 2. Determine protocol name
    let protocol_name = config.protocol();

    // 3. Special case: Ollama still uses native implementation
    if protocol_name == "ollama" {
        return Ok(Arc::new(OllamaProvider::new(name.to_string(), config)?));
    }

    // Special case: Mock provider for testing
    if protocol_name == "mock" {
        return Ok(Arc::new(MockProvider::new("Mock response".to_string())));
    }

    // 4. Get protocol adapter from registry
    let adapter = registry.get(&protocol_name).ok_or_else(|| {
        AlephError::invalid_config(format!(
            "Unknown protocol: '{}'. Available: {:?}",
            protocol_name,
            registry.list_protocols()
        ))
    })?;

    // 5. Use HttpProvider with dynamic protocol
    let provider = HttpProvider::new(name.to_string(), config, adapter)?;
    Ok(Arc::new(provider))
}

/// Unified interface for AI providers.
///
/// All AI backends (`OpenAI`, Claude, Ollama, etc.) implement this trait
/// to provide a consistent API for processing requests.
///
/// # Architecture
///
/// Single `process()` method accepts a `RequestPayload` containing structured
/// `UnifiedMessage` history. Protocol adapters convert these to native formats.
pub trait AiProvider: Send + Sync {
    /// Core method — process a request and return structured response
    fn process<'a>(
        &'a self,
        payload: adapter::RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>;

    /// Provider name
    fn name(&self) -> &str;

    /// Provider brand color
    fn color(&self) -> &str;

    /// Whether this provider supports native `tool_use`
    fn supports_native_tools(&self) -> bool {
        false
    }

    /// Protocol name for model behavior resolution.
    ///
    /// Returns the protocol identifier (e.g., "openai", "anthropic", "gemini", "ollama")
    /// used to select appropriate model behavior directives.
    fn protocol(&self) -> Cow<'_, str> {
        Cow::Borrowed("unknown")
    }

    /// Model behavior override from provider config.
    ///
    /// When set, this takes precedence over the protocol-based auto-mapping.
    /// Used for providers like `OpenRouter` that use one protocol but route to
    /// a different model family.
    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Self-identified governance behavior name derived from the provider's
    /// own endpoint/model (e.g. Kimi/Minimax → `"strict"`). Sits ABOVE the
    /// protocol fallback but BELOW the explicit config `model_behavior`
    /// override in `resolve_behavior`. Default `None` = "no opinion, use the
    /// protocol default". Wrappers delegate; `HttpProvider` computes it.
    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Best-effort id of the model this provider would serve next. Used to key
    /// per-model lookups (context-window resolution for the occupancy gauge,
    /// pricing) when no explicit model directive (session pick / agent hint /
    /// strict brain pin) exists. Wrappers delegate to their live primary, like
    /// [`AiProvider::behavior_hint`]; `HttpProvider` reports its configured
    /// default model. Default `None` = "unknown", callers fall back to the
    /// provider name.
    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// The provider id actually serving this call — the twin of
    /// [`AiProvider::serving_model_hint`], and for the same reason.
    ///
    /// [`AiProvider::name`] is NOT this: on the production path `name()` walks
    /// the decorator stack down to [`failover::FailoverProvider`] and returns
    /// the literal `"failover"`, which is not a provider id any lookup table
    /// knows. Pricing keyed on it silently reports `CostStatus::Unknown` (i.e.
    /// `$0.00`) for every run. Wrappers delegate to their live primary;
    /// `HttpProvider` reports its configured key. Default `None` = "unknown".
    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Downcast to `HttpProvider` for streaming access.
    ///
    /// Returns Some(&HttpProvider) only for `HttpProvider` instances.
    /// Used by `AiProviderBridge` to call `stream_raw()`.
    fn as_http_provider(&self) -> Option<&http_provider::HttpProvider> {
        None
    }

    /// Whether calling [`execute_streaming_dyn`](AiProvider::execute_streaming_dyn)
    /// produces *live* deltas rather than one replayed batch at the end.
    ///
    /// Must be asked of the OUTERMOST provider: a decorator stack is only as
    /// streaming-capable as its weakest link, because a decorator that forgets
    /// to override `execute_streaming_dyn` collapses the call to `process` and
    /// the deltas arrive all at once. Default `false`; a provider opts in only
    /// by genuinely forwarding the sink, and a decorator by delegating.
    ///
    /// This is the honest replacement for gating streaming on
    /// [`as_http_provider`](AiProvider::as_http_provider), which asked "are you
    /// literally an `HttpProvider`" — a question every real production stack
    /// answers "no" (the failover chain sits in the middle and is not one).
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Streaming twin of [`AiProvider::process`]: forward incremental deltas to
    /// `sink` while producing the same structured [`ProviderResponse`].
    ///
    /// `HttpProvider` overrides this to actually stream; the per-run decorators
    /// (`ThinkLevelProvider`, `MeteringProvider`, `ModelOverrideProvider`) and
    /// `FailoverProvider` override it to apply their side effect / walk their
    /// chain and delegate, exactly as they do for `process`. Before this
    /// existed, the harness reached the raw inner `HttpProvider` via
    /// `as_http_provider()` for streaming, silently skipping every decorator
    /// (dropping the declared `think_level`, never emitting `ProviderUsage`).
    ///
    /// The default calls `process` and then **replays** the finished response
    /// through `sink`. Replaying (rather than dropping `sink`) is what makes the
    /// contract usable as a contract: *whoever calls this always sees the
    /// response on the sink*, so a caller that suppresses its own once-per-turn
    /// emit — which is exactly what the harness does — can never end up
    /// delivering nothing because some link in the stack did not override this
    /// method. Overriding it upgrades the delivery from batched to live; it is
    /// not the difference between delivery and silence. There is no double-emit
    /// risk: this path hands `process` no sink, so nothing below ever sees one.
    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: adapter::RequestPayload<'a>,
        sink: &'a dyn DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.process(payload).await?;
            delta::replay_response_to_sink(&response, sink).await;
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;
    use crate::sync_primitives::Arc;

    // Simple test implementation to verify trait can be used as trait object
    struct TestProvider;

    impl AiProvider for TestProvider {
        fn process(
            &self,
            payload: adapter::RequestPayload<'_>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>> {
            let text = UnifiedMessage::extract_all_text(payload.messages);
            let response = format!("Echo: {}", text);
            Box::pin(async move { Ok(ProviderResponse::text_only(response)) })
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

        let msgs = [UnifiedMessage::user("hello")];
        let response = provider
            .process(adapter::RequestPayload::new(&msgs))
            .await
            .unwrap();
        assert_eq!(response.text_content(), "Echo: hello");

        assert_eq!(provider.name(), "test");
        assert_eq!(provider.color(), "#000000");
    }

    #[tokio::test]
    async fn test_provider_with_system_prompt() {
        let provider: Arc<dyn AiProvider> = Arc::new(TestProvider);
        let msgs = [UnifiedMessage::user("input")];
        let response = provider
            .process(adapter::RequestPayload::new(&msgs).with_system(Some("system prompt")))
            .await
            .unwrap();
        assert_eq!(response.text_content(), "Echo: input");
    }

    #[test]
    fn test_provider_is_send_sync() {
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
        config.protocol = Some("anthropic".to_string());

        let provider = create_provider("claude", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "claude");
    }

    #[test]
    fn test_create_gemini_provider() {
        let mut config = ProviderConfig::test_config("gemini-1.5-flash");
        config.protocol = Some("gemini".to_string());

        let provider = create_provider("gemini", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "gemini");
    }

    #[test]
    fn test_create_ollama_provider() {
        let mut config = ProviderConfig::test_config("llama3.2");
        config.protocol = Some("ollama".to_string());
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
        config.protocol = Some("openai".to_string());
        config.base_url = Some("https://api.deepseek.com".to_string());

        let provider = create_provider("deepseek", config);
        assert!(provider.is_ok());
        // OpenAI provider implementation is used for DeepSeek, but keeps custom name
        assert_eq!(provider.unwrap().name(), "deepseek");
    }

    // Tests for provider_type inference removed - this functionality moved to protocol registry

    #[test]
    fn test_create_unknown_protocol() {
        let mut config = ProviderConfig::test_config("model");
        config.protocol = Some("unknown".to_string());

        let result = create_provider("test", config);
        assert!(result.is_err());
        assert!(matches!(result, Err(AlephError::InvalidConfig { .. })));
    }

    #[test]
    fn test_create_openai_responses_provider() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.protocol = Some("openai-responses".to_string());
        config.base_url = Some("https://openrouter.ai/api".to_string());

        let provider = create_provider("openrouter", config);
        assert!(
            provider.is_ok(),
            "Should create openai-responses provider: {:?}",
            provider.err()
        );
        assert_eq!(provider.unwrap().name(), "openrouter");
    }

    #[test]
    fn test_create_openrouter_via_preset() {
        let config = ProviderConfig::test_config("openai/gpt-4o");
        let provider = create_provider("openrouter", config);
        assert!(provider.is_ok());
        assert_eq!(provider.unwrap().name(), "openrouter");
    }

    #[test]
    fn test_multiple_custom_providers() {
        // Simulate configuring multiple custom providers
        let mut deepseek_config = ProviderConfig::test_config("deepseek-chat");
        deepseek_config.protocol = Some("openai".to_string());
        deepseek_config.base_url = Some("https://api.deepseek.com".to_string());

        let mut moonshot_config = ProviderConfig::test_config("kimi-k2-0905-preview");
        moonshot_config.protocol = Some("openai".to_string());
        moonshot_config.base_url = Some("https://api.moonshot.ai/v1".to_string());
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
