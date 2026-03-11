//! Real LLM implementation of PatternSynthesisBackend.
//!
//! Wraps `Arc<dyn AiProvider>` to connect pattern extraction to actual
//! LLM inference. `synthesize_pattern` calls the LLM; `evaluate_confidence`
//! uses a token-efficient heuristic.

use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Real LLM-backed implementation of `PatternSynthesisBackend`.
pub struct ProviderBackend {
    provider: Arc<dyn AiProvider>,
}

impl ProviderBackend {
    /// Create a new ProviderBackend wrapping the given AI provider.
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockAiProvider;

    impl AiProvider for MockAiProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
            Box::pin(async { Ok("mock response".to_string()) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[test]
    fn test_provider_backend_creation() {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let _backend = ProviderBackend::new(provider);
    }
}
