//! Thinker - Prompt construction, identity, and provider registry
//!
//! This module provides:
//! - Prompt building (system prompts via layered pipeline)
//! - Identity resolution (soul, workspace files)
//! - Provider registry (model routing at the registry level)
//! - Security context and interaction paradigms

pub mod context;
pub mod identity_files;
pub mod identity_profile;
pub mod interaction;
pub mod layers;
pub mod memory_context_provider;
pub mod nudges;
pub mod project_instructions;
pub mod prompt_budget;
pub mod prompt_builder;
/// Cross-layer contract guards (reachability, byte ratchet, no duplicate
/// sentences). Test-only — see the module docs for why these live outside any
/// single layer's own tests.
#[cfg(test)]
mod prompt_contract;
pub mod prompt_layer;
pub mod prompt_mode;
pub mod prompt_pipeline;
pub mod prompt_sanitizer;
pub mod protocol_tokens;
pub mod runtime_context;
pub mod security_context;
pub mod soul;
pub mod soul_archetypes;
pub(crate) mod xml_util;

use crate::sync_primitives::Arc;

pub use context::{ContextAggregator, TurnEnvelope};
pub use interaction::{
    Capability, InteractionConstraints, InteractionManifest, InteractionParadigm,
};
pub use memory_context_provider::{MemoryContextConfig, MemoryContextProvider};
pub use prompt_budget::TokenBudget;
pub use prompt_builder::{PromptBuilder, PromptConfig};
pub use prompt_layer::{LayerInput, PromptLayer};
pub use soul::{SoulManifest, SoulVoice};

use crate::providers::AiProvider;

/// Provider registry for model routing.
///
/// Name → provider lookup and nothing more. It deliberately carries **no health
/// state**: a second breaker used to live here, predicting a candidate before
/// each request, and it decided no dial — the one that gates dialing is
/// `FailoverProvider`'s, fed by the walk that actually reaches the wire.
pub trait ProviderRegistry: Send + Sync {
    /// Get provider for a specific model
    fn get(&self, model: &str) -> Option<Arc<dyn AiProvider>>;

    /// Get default provider
    fn default_provider(&self) -> Arc<dyn AiProvider>;

    /// List all registered provider names
    fn list_providers(&self) -> Vec<String> {
        vec![]
    }
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
    provider: crate::sync_primitives::RwLock<Arc<dyn AiProvider>>,
}

impl SwappableProviderRegistry {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self {
            provider: crate::sync_primitives::RwLock::new(provider),
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
        Some(
            self.provider
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        self.provider
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod swappable_registry_tests {
    use super::*;

    struct TaggedProvider {
        tag: String,
    }
    impl AiProvider for TaggedProvider {
        fn process(
            &self,
            _payload: crate::providers::adapter::RequestPayload<'_>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Ok(crate::providers::adapter::ProviderResponse::text_only(
                    String::new(),
                ))
            })
        }
        fn name(&self) -> &str {
            &self.tag
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    #[test]
    fn test_swappable_registry_returns_initial_provider() {
        let provider = Arc::new(TaggedProvider {
            tag: "initial".into(),
        });
        let registry = SwappableProviderRegistry::new(provider);

        assert_eq!(registry.default_provider().name(), "initial");
    }

    #[test]
    fn test_swappable_registry_swap_changes_provider() {
        let p1 = Arc::new(TaggedProvider {
            tag: "provider-a".into(),
        });
        let p2: Arc<dyn AiProvider> = Arc::new(TaggedProvider {
            tag: "provider-b".into(),
        });

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
    /// Insertion order of provider names, used for fallback when default is removed.
    provider_order: Vec<String>,
    default_name: String,
}

/// Multi-provider registry: routes by provider name, supports runtime mutation and fallback.
pub struct MultiProviderRegistry {
    state: crate::sync_primitives::RwLock<RegistryState>,
}

impl MultiProviderRegistry {
    pub fn new(name: String, provider: Arc<dyn AiProvider>) -> Self {
        let mut providers = HashMap::new();
        providers.insert(name.clone(), provider);
        Self {
            state: crate::sync_primitives::RwLock::new(RegistryState {
                providers,
                provider_order: vec![name.clone()],
                default_name: name,
            }),
        }
    }

    pub fn register(&self, name: String, provider: Arc<dyn AiProvider>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if !state.providers.contains_key(&name) {
            state.provider_order.push(name.clone());
        }
        state.providers.insert(name, provider);
    }

    pub fn remove(&self, name: &str) -> crate::error::Result<Option<Arc<dyn AiProvider>>> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if state.providers.len() <= 1 && state.providers.contains_key(name) {
            return Err(crate::error::AlephError::provider(
                "Cannot remove the last provider",
            ));
        }
        let removed = state.providers.remove(name);
        state.provider_order.retain(|n| n != name);
        if state.default_name == name {
            if let Some(first) = state.provider_order.first() {
                state.default_name = first.clone();
            }
        }
        Ok(removed)
    }

    pub fn set_default(&self, name: &str) -> crate::error::Result<()> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if !state.providers.contains_key(name) {
            return Err(crate::error::AlephError::provider(format!(
                "Provider '{name}' not found in registry"
            )));
        }
        state.default_name = name.to_string();
        Ok(())
    }

    /// List all registered provider names (inherent method for direct access)
    pub fn list_providers(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        let mut names: Vec<String> = state.providers.keys().cloned().collect();
        names.sort();
        names
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
        state
            .providers
            .get(&state.default_name)
            .or_else(|| {
                // Deterministic fallback: pick the lexicographically smallest key
                state
                    .providers
                    .keys()
                    .min()
                    .and_then(|k| state.providers.get(k))
            })
            .cloned()
            .unwrap_or_else(|| {
                // This should never happen because MultiProviderRegistry::new
                // always inserts at least one provider, but we return a dummy
                // rather than panicking to keep the system running.
                tracing::error!("MultiProviderRegistry has no providers — returning dummy");
                self.get("dummy").unwrap_or_else(|| {
                    // Last resort: we have no providers at all. Return a
                    // DummyProvider so the caller gets a recoverable error
                    // instead of a process crash.
                    tracing::error!("No providers registered in MultiProviderRegistry");
                    Arc::new(DummyProvider) as Arc<dyn AiProvider>
                })
            })
    }

    fn list_providers(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        let mut names: Vec<String> = state.providers.keys().cloned().collect();
        names.sort();
        names
    }
}

/// Live default-provider resolution: each `current()` call reads through the
/// registry's `RwLock`, so UI-driven `set_default()` swaps are visible on the
/// very next harness dispatch turn (no restart needed).
impl crate::providers::DefaultProviderHandle for MultiProviderRegistry {
    fn current(&self) -> crate::sync_primitives::Arc<dyn crate::providers::AiProvider> {
        <Self as ProviderRegistry>::default_provider(self)
    }

    /// Live snapshot of every registered provider name, so an auto-derived
    /// failover chain reflects runtime `register`/`remove` without a restart.
    fn provider_names(&self) -> Vec<String> {
        self.list_providers()
    }

    /// Exact-name lookup against the live registry (not model resolution), so
    /// auto-derived fallback nodes bind to the current provider instance.
    fn provider_by_name(
        &self,
        name: &str,
    ) -> Option<crate::sync_primitives::Arc<dyn crate::providers::AiProvider>> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.providers.get(name).cloned()
    }
}

/// Dummy provider returned when [`MultiProviderRegistry`] has no registered providers.
///
/// `process()` returns an error so callers get a recoverable failure instead of a panic.
/// This is a last-resort fallback — callers should check [`ProviderRegistry::list_providers()`]
/// before using [`ProviderRegistry::default_provider()`].
struct DummyProvider;

impl crate::providers::AiProvider for DummyProvider {
    fn process<'a>(
        &'a self,
        _payload: crate::providers::adapter::RequestPayload<'a>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            Err(crate::error::AlephError::provider(
                "No providers registered in MultiProviderRegistry",
            ))
        })
    }
    fn name(&self) -> &str {
        "dummy"
    }
    fn color(&self) -> &str {
        "#000"
    }
}

#[cfg(test)]
mod multi_registry_tests {
    use super::*;

    struct NamedProvider {
        tag: String,
    }
    impl AiProvider for NamedProvider {
        fn process(
            &self,
            _: crate::providers::adapter::RequestPayload<'_>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Ok(crate::providers::adapter::ProviderResponse::text_only(
                    String::new(),
                ))
            })
        }
        fn name(&self) -> &str {
            &self.tag
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    fn p(name: &str) -> Arc<dyn AiProvider> {
        Arc::new(NamedProvider { tag: name.into() })
    }

    #[test]
    fn test_default() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        assert_eq!(r.default_provider().name(), "openai");
    }

    #[test]
    fn test_get_by_slash_prefix() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert_eq!(
            r.get("anthropic/claude-opus-4-6").unwrap().name(),
            "anthropic"
        );
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

    /// `DefaultProviderHandle::current()` must reflect a `set_default()` swap
    /// without rebuilding the handle. This is the load-bearing guarantee for
    /// UI-driven default-provider changes (Step 5 hot-reload).
    #[test]
    fn default_handle_reflects_set_default_swap() {
        use crate::providers::DefaultProviderHandle;
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert_eq!(r.current().name(), "openai");
        r.set_default("anthropic").unwrap();
        assert_eq!(r.current().name(), "anthropic");
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
