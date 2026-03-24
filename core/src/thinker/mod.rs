//! Thinker - Prompt construction, identity, and provider registry
//!
//! This module provides:
//! - Prompt building (system prompts via layered pipeline)
//! - Identity resolution (soul, workspace files)
//! - Provider registry (model routing at the registry level)
//! - Cache strategies for prompt caching
//! - Security context and interaction paradigms

pub mod cache;
pub mod channel_behavior;
pub mod context;
pub mod hooks;
pub mod identity;
pub mod inbound_context;
pub mod interaction;
pub mod prompt_budget;
pub mod prompt_builder;
pub mod prompt_hooks;
pub mod prompt_hooks_v2;
pub mod prompt_layer;
pub mod prompt_mode;
pub mod prompt_pipeline;
pub mod layers;
pub mod security_context;
pub mod soul;
pub mod prompt_sanitizer;
pub mod protocol_tokens;
pub mod runtime_context;
pub mod streaming;
pub mod user_profile;
pub mod virtual_tools;
pub mod memory_context;
pub mod memory_context_provider;
pub mod identity_files;

use crate::sync_primitives::Arc;

pub use cache::{
    AnthropicCacheStrategy, CacheContext, CacheControl, CacheStrategy, CacheableContentBlock,
    GeminiCacheCreateRequest, GeminiCacheCreateResponse, GeminiCacheStrategy, GeminiContent,
    GeminiPart, ProviderType, SystemPromptCache, TransparentCacheStrategy,
    get_cache_strategy, GEMINI_CACHE_TTL_SECS, MIN_CACHE_TOKENS,
};
pub use prompt_builder::{PromptBuilder, PromptConfig};
pub use prompt_budget::{PromptResult, TokenBudget, TruncationStat, TruncationWarning};
pub use prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
pub use prompt_mode::PromptMode;
pub use prompt_pipeline::PromptPipeline;
pub use interaction::{Capability, InteractionConstraints, InteractionManifest, InteractionParadigm};
pub use security_context::{
    ElevatedPolicy, SandboxLevel, SecurityContext, ToolPermission, is_network_tool,
};
pub use context::{
    ContextAggregator, DisableReason, DisabledTool, EnvironmentContract, ResolvedContext,
};
pub use soul::{FormattingStyle, RelationshipMode, SoulLoadError, SoulManifest, SoulVoice, Verbosity};
pub use protocol_tokens::ProtocolToken;
pub use memory_context::{MemoryContext, MemorySummary};
pub use memory_context_provider::{MemoryContextProvider, MemoryContextConfig};
pub use runtime_context::RuntimeContext;
pub use identity::{IdentityResolver, IdentitySource, IdentitySourceType};

use crate::providers::AiProvider;

/// Provider registry for model routing
pub trait ProviderRegistry: Send + Sync {
    /// Get provider for a specific model
    fn get(&self, model: &str) -> Option<Arc<dyn AiProvider>>;

    /// Get default provider
    fn default_provider(&self) -> Arc<dyn AiProvider>;

    /// List all registered provider names
    fn list_providers(&self) -> Vec<String> { vec![] }
}

/// Simple provider registry with single provider
pub struct SingleProviderRegistry {
    provider: Arc<dyn AiProvider>,
}

impl SingleProviderRegistry {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }
}

impl ProviderRegistry for SingleProviderRegistry {
    fn get(&self, _model: &str) -> Option<Arc<dyn AiProvider>> {
        Some(self.provider.clone())
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        self.provider.clone()
    }
}

/// Provider registry that supports runtime hot-swapping.
///
/// When the user switches the default provider via the Panel,
/// the new provider is atomically swapped in without restarting the server.
pub struct SwappableProviderRegistry {
    provider: std::sync::RwLock<Arc<dyn AiProvider>>,
}

impl SwappableProviderRegistry {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider: std::sync::RwLock::new(provider),
        }
    }

    /// Atomically swap the underlying provider.
    pub fn swap(&self, new_provider: Arc<dyn AiProvider>) {
        let mut guard = self.provider.write().unwrap_or_else(|e| e.into_inner());
        *guard = new_provider;
    }
}

impl ProviderRegistry for SwappableProviderRegistry {
    fn get(&self, _model: &str) -> Option<Arc<dyn AiProvider>> {
        Some(self.provider.read().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        self.provider.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod swappable_registry_tests {
    use super::*;

    struct TaggedProvider { tag: String }
    impl AiProvider for TaggedProvider {
        fn process(
            &self, _payload: crate::providers::adapter::RequestPayload<'_>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<crate::providers::adapter::ProviderResponse>> + Send + '_>> {
            Box::pin(async { Ok(crate::providers::adapter::ProviderResponse::text_only(String::new())) })
        }
        fn name(&self) -> &str { &self.tag }
        fn color(&self) -> &str { "#000" }
    }

    #[test]
    fn test_swappable_registry_returns_initial_provider() {
        let provider = Arc::new(TaggedProvider { tag: "initial".into() });
        let registry = SwappableProviderRegistry::new(provider);

        assert_eq!(registry.default_provider().name(), "initial");
    }

    #[test]
    fn test_swappable_registry_swap_changes_provider() {
        let p1 = Arc::new(TaggedProvider { tag: "provider-a".into() });
        let p2: Arc<dyn AiProvider> = Arc::new(TaggedProvider { tag: "provider-b".into() });

        let registry = SwappableProviderRegistry::new(p1);
        assert_eq!(registry.default_provider().name(), "provider-a");

        registry.swap(p2);
        assert_eq!(registry.default_provider().name(), "provider-b");

        // get() should also return the swapped provider
        assert_eq!(registry.get("any-model").unwrap().name(), "provider-b");
    }
}

use std::collections::HashMap;

struct RegistryState {
    providers: HashMap<String, Arc<dyn AiProvider>>,
    default_name: String,
    fallbacks: Vec<String>,
}

/// Multi-provider registry: routes by provider name, supports runtime mutation and fallback.
pub struct MultiProviderRegistry {
    state: std::sync::RwLock<RegistryState>,
}

impl MultiProviderRegistry {
    pub fn new(name: String, provider: Arc<dyn AiProvider>) -> Self {
        let mut providers = HashMap::new();
        providers.insert(name.clone(), provider);
        Self {
            state: std::sync::RwLock::new(RegistryState {
                providers, default_name: name, fallbacks: vec![],
            }),
        }
    }

    pub fn register(&self, name: String, provider: Arc<dyn AiProvider>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.providers.insert(name, provider);
    }

    pub fn remove(&self, name: &str) -> crate::error::Result<Option<Arc<dyn AiProvider>>> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if state.providers.len() <= 1 && state.providers.contains_key(name) {
            return Err(crate::error::AlephError::provider("Cannot remove the last provider"));
        }
        let removed = state.providers.remove(name);
        if state.default_name == name {
            if let Some(first) = state.providers.keys().next() {
                state.default_name = first.clone();
            }
        }
        Ok(removed)
    }

    pub fn set_default(&self, name: &str) -> crate::error::Result<()> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if !state.providers.contains_key(name) {
            return Err(crate::error::AlephError::provider(
                format!("Provider '{}' not found in registry", name),
            ));
        }
        state.default_name = name.to_string();
        Ok(())
    }

    pub fn set_fallbacks(&self, chain: Vec<String>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.fallbacks = chain;
    }

    pub fn fallbacks(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.fallbacks.clone()
    }
}

impl ProviderRegistry for MultiProviderRegistry {
    fn get(&self, model_key: &str) -> Option<Arc<dyn AiProvider>> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        // Try "provider/model" format
        if let Some(provider_name) = model_key.split('/').next() {
            if let Some(p) = state.providers.get(provider_name) {
                return Some(p.clone());
            }
        }
        // Try model name → preset resolution
        if let Some(provider_name) = crate::providers::resolve_provider_from_model(model_key) {
            if let Some(p) = state.providers.get(&provider_name) {
                return Some(p.clone());
            }
        }
        None
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.providers.get(&state.default_name)
            .or_else(|| state.providers.values().next())
            .cloned()
            .expect("registry must have at least one provider")
    }

    fn list_providers(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.providers.keys().cloned().collect()
    }
}

#[cfg(test)]
mod multi_registry_tests {
    use super::*;

    struct NamedProvider { tag: String }
    impl AiProvider for NamedProvider {
        fn process(&self, _: crate::providers::adapter::RequestPayload<'_>)
            -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<crate::providers::adapter::ProviderResponse>> + Send + '_>>
        {
            Box::pin(async { Ok(crate::providers::adapter::ProviderResponse::text_only(String::new())) })
        }
        fn name(&self) -> &str { &self.tag }
        fn color(&self) -> &str { "#000" }
    }

    fn p(name: &str) -> Arc<dyn AiProvider> { Arc::new(NamedProvider { tag: name.into() }) }

    #[test]
    fn test_default() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        assert_eq!(r.default_provider().name(), "openai");
    }

    #[test]
    fn test_get_by_slash_prefix() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert_eq!(r.get("anthropic/claude-opus-4-6").unwrap().name(), "anthropic");
    }

    #[test]
    fn test_get_by_model_prefix() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert_eq!(r.get("claude-opus-4-6").unwrap().name(), "anthropic");
    }

    #[test]
    fn test_unknown_returns_none() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        assert!(r.get("unknown-xyz").is_none());
    }

    #[test]
    fn test_set_default() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        r.set_default("anthropic").unwrap();
        assert_eq!(r.default_provider().name(), "anthropic");
    }

    #[test]
    fn test_remove() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert!(r.remove("anthropic").unwrap().is_some());
        assert!(r.get("anthropic/x").is_none());
    }

    #[test]
    fn test_cannot_remove_last() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        assert!(r.remove("openai").is_err());
    }

    #[test]
    fn test_remove_default_auto_switches() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        r.remove("openai").unwrap();
        assert_eq!(r.default_provider().name(), "anthropic");
    }

    #[test]
    fn test_list_providers() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        let mut list = r.list_providers();
        list.sort();
        assert_eq!(list, vec!["anthropic", "openai"]);
    }
}
