//! Thinker - Prompt construction, identity, and provider registry
//!
//! This module provides:
//! - Prompt building (system prompts via layered pipeline)
//! - Identity resolution (soul, workspace files)
//! - Provider registry (model routing at the registry level)
//! - Security context and interaction paradigms

pub mod context;
pub mod identity_files;
pub mod interaction;
pub mod layers;
pub mod memory_context_provider;
pub mod nudges;
pub mod project_instructions;
pub mod prompt_budget;
pub mod prompt_builder;
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

pub use context::ContextAggregator;
pub use interaction::{
    Capability, InteractionConstraints, InteractionManifest, InteractionParadigm,
};
pub use memory_context_provider::{MemoryContextConfig, MemoryContextProvider};
pub use prompt_budget::TokenBudget;
pub use prompt_builder::{PromptBuilder, PromptConfig};
pub use prompt_layer::{LayerInput, PromptLayer};
pub use soul::{SoulManifest, SoulVoice};

use crate::providers::health::{ProviderError, ProviderHealth, ResolvedModel};
use crate::providers::AiProvider;

/// Provider registry for model routing
pub trait ProviderRegistry: Send + Sync {
    /// Get provider for a specific model
    fn get(&self, model: &str) -> Option<Arc<dyn AiProvider>>;

    /// Get default provider
    fn default_provider(&self) -> Arc<dyn AiProvider>;

    /// List all registered provider names
    fn list_providers(&self) -> Vec<String> {
        vec![]
    }

    /// Resolve model to a healthy (provider, model) pair along the fallback chain.
    fn resolve_with_fallback(
        &self,
        model: &str,
        _agent_fallbacks: &[String],
    ) -> crate::error::Result<ResolvedModel> {
        // Default: no health tracking, just resolve to default
        let provider = self.get(model).unwrap_or_else(|| self.default_provider());
        Ok(ResolvedModel {
            provider_name: provider.name().to_string(),
            model: model.to_string(),
            is_fallback: false,
            original_model: model.to_string(),
        })
    }

    /// Report request outcome to update provider health
    fn report_outcome(&self, _provider: &str, _result: Result<(), ProviderError>) {}

    /// Reset a provider's health to Healthy
    fn reset_health(&self, _provider: &str) {}
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
    fallbacks: Vec<String>,
    health: HashMap<String, ProviderHealth>,
}

/// Multi-provider registry: routes by provider name, supports runtime mutation and fallback.
pub struct MultiProviderRegistry {
    state: crate::sync_primitives::RwLock<RegistryState>,
}

impl MultiProviderRegistry {
    pub fn new(name: String, provider: Arc<dyn AiProvider>) -> Self {
        let mut providers = HashMap::new();
        providers.insert(name.clone(), provider);
        let mut health = HashMap::new();
        health.insert(name.clone(), ProviderHealth::default());
        Self {
            state: crate::sync_primitives::RwLock::new(RegistryState {
                providers,
                provider_order: vec![name.clone()],
                default_name: name,
                fallbacks: vec![],
                health,
            }),
        }
    }

    pub fn register(&self, name: String, provider: Arc<dyn AiProvider>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if !state.providers.contains_key(&name) {
            state.provider_order.push(name.clone());
        }
        state.providers.insert(name.clone(), provider);
        state.health.entry(name).or_default();
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

    pub fn set_fallbacks(&self, chain: Vec<String>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.fallbacks = chain;
    }

    pub fn fallbacks(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.fallbacks.clone()
    }

    /// List all registered provider names (inherent method for direct access)
    pub fn list_providers(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        let mut names: Vec<String> = state.providers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Resolve model → provider name using slash syntax, prefix matching, or default.
    /// Resolve a model string to (`provider_name`, `actual_model_name`).
    ///
    /// For "provider/model" syntax (e.g., "openai/gpt-4o"), strips the prefix
    /// and returns ("openai", "gpt-4o") so native APIs receive the correct model name.
    /// For bare model names (e.g., "claude-sonnet-4"), uses prefix-based resolution.
    fn resolve_model_to_provider_and_model(state: &RegistryState, model: &str) -> (String, String) {
        // 1. Try "provider/model" slash syntax — strip prefix for the actual model name
        if let Some(slash_pos) = model.find('/') {
            let provider_name = &model[..slash_pos];
            if state.providers.contains_key(provider_name) {
                let actual_model = &model[slash_pos + 1..];
                return (provider_name.to_string(), actual_model.to_string());
            }
        }

        // 2. Try prefix-based resolution (bare model name, no stripping needed)
        if let Some(name) = crate::providers::resolve_provider_from_model(model) {
            if state.providers.contains_key(&name) {
                return (name, model.to_string());
            }
        }

        // 3. Fall back to default provider
        (state.default_name.clone(), model.to_string())
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

    fn resolve_with_fallback(
        &self,
        model: &str,
        agent_fallbacks: &[String],
    ) -> crate::error::Result<ResolvedModel> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());

        // Build candidate chain: (provider_name, model_name)
        let mut candidates: Vec<(String, String)> = Vec::new();

        // 1. Primary: resolve model → provider (strips "provider/" prefix if present)
        let (primary_provider, primary_model) =
            Self::resolve_model_to_provider_and_model(&state, model);
        candidates.push((primary_provider, primary_model));

        // 2. Agent-level fallbacks (also strip prefix)
        for fb_model in agent_fallbacks {
            let (provider, actual_model) =
                Self::resolve_model_to_provider_and_model(&state, fb_model);
            candidates.push((provider, actual_model));
        }

        // 3. Global fallbacks — model is empty string (sentinel for "use provider's configured default")
        // When the caller sees an empty model, it should NOT set payload.model,
        // letting the protocol adapter fall back to config.default_model().
        //
        // If fallback_providers is explicitly configured, use that order.
        // Otherwise, auto-fallback: all registered providers except the primary become candidates.
        let global_fallbacks: Vec<String> = if state.fallbacks.is_empty() {
            // Auto-fallback: any registered provider that isn't already a candidate
            let existing: std::collections::HashSet<&str> =
                candidates.iter().map(|(p, _)| p.as_str()).collect();
            state
                .providers
                .keys()
                .filter(|name| !existing.contains(name.as_str()))
                .cloned()
                .collect()
        } else {
            state.fallbacks.clone()
        };

        let empty_model = String::new();
        for fb_provider_name in global_fallbacks {
            if state.providers.contains_key(&fb_provider_name) {
                candidates.push((fb_provider_name, empty_model.clone()));
            }
        }

        // Try each candidate in order, skipping unhealthy ones.
        // Collect degraded candidates for the single-provider fallback.
        let mut degraded_candidates: Vec<(String, String)> = Vec::new();
        for (i, (provider_name, candidate_model)) in candidates.into_iter().enumerate() {
            let health = state.health.get(&provider_name).cloned().unwrap_or_default();
            if !health.is_usable() {
                if matches!(health, ProviderHealth::Degraded { .. }) {
                    degraded_candidates.push((provider_name, candidate_model));
                }
                continue;
            }
            if state.providers.contains_key(&provider_name) {
                return Ok(ResolvedModel {
                    provider_name,
                    model: candidate_model,
                    is_fallback: i > 0,
                    original_model: model.to_string(),
                });
            }
        }

        // Last-resort fallback: if every usable provider is depleted and only
        // one provider is registered, allow a degraded provider (cooldown still
        // active) so a single-provider setup can retry within the same run
        // instead of failing immediately with "no healthy provider". When there
        // are multiple providers we keep the circuit breaker closed so a user
        // with alternatives does not hammer a degraded one. Permanent
        // `Unavailable` providers are always skipped — they require user
        // intervention.
        if state.providers.len() == 1 {
            for (provider_name, candidate_model) in degraded_candidates.into_iter() {
                let health = state.health.get(&provider_name).cloned().unwrap_or_default();
                if matches!(health, ProviderHealth::Degraded { .. })
                    && state.providers.contains_key(&provider_name)
                {
                    tracing::warn!(
                        provider = %provider_name,
                        "No healthy provider; retrying degraded provider as last resort"
                    );
                    return Ok(ResolvedModel {
                        provider_name,
                        model: candidate_model,
                        is_fallback: true,
                        original_model: model.to_string(),
                    });
                }
            }
        }

        Err(crate::error::AlephError::provider(
            "All providers unavailable — no healthy provider found in fallback chain",
        ))
    }

    fn report_outcome(&self, provider_name: &str, result: Result<(), ProviderError>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if let Some(health) = state.health.get_mut(provider_name) {
            match result {
                Ok(()) => health.record_success(),
                Err(ref err) => health.record_failure(err),
            }
        }
    }

    fn reset_health(&self, provider_name: &str) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if let Some(health) = state.health.get_mut(provider_name) {
            health.reset();
        }
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

    // --- resolve_with_fallback tests ---

    #[test]
    fn resolve_returns_requested_model_when_healthy() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));

        let resolved = r.resolve_with_fallback("claude-opus-4-6", &[]).unwrap();
        assert_eq!(resolved.provider_name, "anthropic");
        assert_eq!(resolved.model, "claude-opus-4-6");
        assert!(!resolved.is_fallback);
        assert_eq!(resolved.original_model, "claude-opus-4-6");
    }

    #[test]
    fn resolve_uses_agent_fallback_when_primary_degraded() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));

        // Degrade anthropic
        r.report_outcome(
            "anthropic",
            Err(ProviderError::Transient(
                crate::providers::health::TransientError::Timeout,
            )),
        );

        let resolved = r
            .resolve_with_fallback("claude-opus-4-6", &["gpt-4o".to_string()])
            .unwrap();

        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model, "gpt-4o");
        assert!(resolved.is_fallback);
        assert_eq!(resolved.original_model, "claude-opus-4-6");
    }

    #[test]
    fn resolve_uses_global_fallback_when_agent_fallbacks_exhausted() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        r.register("google".into(), p("google"));
        r.set_fallbacks(vec!["google".into()]);

        // Degrade both primary and agent fallback providers
        r.report_outcome(
            "anthropic",
            Err(ProviderError::Transient(
                crate::providers::health::TransientError::Timeout,
            )),
        );
        r.report_outcome(
            "openai",
            Err(ProviderError::Transient(
                crate::providers::health::TransientError::Timeout,
            )),
        );

        let resolved = r
            .resolve_with_fallback("claude-opus-4-6", &["gpt-4o".to_string()])
            .unwrap();

        assert_eq!(resolved.provider_name, "google");
        assert!(resolved.is_fallback);
        assert!(
            resolved.model.is_empty(),
            "global fallback model should be empty (use provider default)"
        );
        assert_eq!(resolved.original_model, "claude-opus-4-6");
    }

    #[test]
    fn resolve_fails_when_all_unavailable() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));

        // Make both unavailable (permanent error)
        r.report_outcome(
            "openai",
            Err(ProviderError::Permanent(
                crate::providers::health::PermanentError::AuthFailed,
            )),
        );
        r.report_outcome(
            "anthropic",
            Err(ProviderError::Permanent(
                crate::providers::health::PermanentError::AuthFailed,
            )),
        );

        let result = r.resolve_with_fallback("gpt-4o", &["claude-opus-4-6".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_uses_provider_slash_model_syntax() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));

        let resolved = r.resolve_with_fallback("openai/gpt-4o", &[]).unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(
            resolved.model, "gpt-4o",
            "provider prefix should be stripped for native API"
        );
        assert!(!resolved.is_fallback);
    }

    #[test]
    fn report_success_resets_health() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));

        // Degrade first
        r.report_outcome(
            "openai",
            Err(ProviderError::Transient(
                crate::providers::health::TransientError::Timeout,
            )),
        );

        // Should be in cooldown, but report success to reset
        r.report_outcome("openai", Ok(()));

        // Should be usable again as primary
        let resolved = r.resolve_with_fallback("gpt-4o", &[]).unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert!(!resolved.is_fallback);
    }

    #[test]
    fn resolve_unknown_model_uses_default_provider() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));

        let resolved = r
            .resolve_with_fallback("totally-unknown-model", &[])
            .unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model, "totally-unknown-model");
        assert!(!resolved.is_fallback);
    }

    #[test]
    fn resolve_retries_degraded_provider_when_its_the_only_one() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));

        // A transient failure puts the only provider into cooldown.
        r.report_outcome(
            "openai",
            Err(ProviderError::Transient(
                crate::providers::health::TransientError::Timeout,
            )),
        );

        // Without a last-resort degraded fallback, this would error out.
        let resolved = r.resolve_with_fallback("gpt-4o", &[]).unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model, "gpt-4o");
    }

    #[test]
    fn full_fallback_chain_agent_then_global() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        r.register("google".into(), p("google"));
        r.set_fallbacks(vec!["google".into()]);

        // Degrade openai (primary for gpt-4o)
        r.report_outcome(
            "openai",
            Err(ProviderError::Transient(
                crate::providers::health::TransientError::ServerError { status: 500 },
            )),
        );

        // Agent fallback "claude-opus-4-6" → anthropic (healthy) should be picked
        let resolved = r
            .resolve_with_fallback("gpt-4o", &["claude-opus-4-6".to_string()])
            .unwrap();

        assert_eq!(resolved.provider_name, "anthropic");
        assert_eq!(resolved.model, "claude-opus-4-6");
        assert!(resolved.is_fallback);
        assert_eq!(resolved.original_model, "gpt-4o");

        // Now degrade anthropic too — should fall to global fallback "google"
        r.report_outcome(
            "anthropic",
            Err(ProviderError::Transient(
                crate::providers::health::TransientError::Timeout,
            )),
        );

        let resolved = r
            .resolve_with_fallback("gpt-4o", &["claude-opus-4-6".to_string()])
            .unwrap();

        assert_eq!(resolved.provider_name, "google");
        assert!(resolved.is_fallback);
        assert_eq!(resolved.original_model, "gpt-4o");
    }

    #[test]
    fn full_fallback_chain_all_layers() {
        use crate::providers::health::{PermanentError, TransientError};

        // Setup: 3 providers
        let r = MultiProviderRegistry::new("claude".into(), p("claude"));
        r.register("openai".into(), p("openai"));
        r.register("deepseek".into(), p("deepseek"));
        r.set_fallbacks(vec!["deepseek".into()]);

        // Scenario 1: All healthy — primary wins
        let resolved = r
            .resolve_with_fallback("claude-sonnet-4", &["gpt-4o".to_string()])
            .unwrap();
        assert_eq!(resolved.provider_name, "claude");
        assert!(!resolved.is_fallback);

        // Scenario 2: Primary auth fails → agent fallback
        r.report_outcome(
            "claude",
            Err(ProviderError::Permanent(PermanentError::AuthFailed)),
        );
        let resolved = r
            .resolve_with_fallback("claude-sonnet-4", &["gpt-4o".to_string()])
            .unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model, "gpt-4o");
        assert!(resolved.is_fallback);

        // Scenario 3: Primary + agent fallback both down → global fallback
        r.report_outcome(
            "openai",
            Err(ProviderError::Transient(TransientError::Timeout)),
        );
        let resolved = r
            .resolve_with_fallback("claude-sonnet-4", &["gpt-4o".to_string()])
            .unwrap();
        assert_eq!(resolved.provider_name, "deepseek");
        assert!(resolved.is_fallback);
        assert!(resolved.model.is_empty()); // global fallback = empty model sentinel

        // Scenario 4: All down
        r.report_outcome(
            "deepseek",
            Err(ProviderError::Permanent(PermanentError::AuthFailed)),
        );
        let result = r.resolve_with_fallback("claude-sonnet-4", &["gpt-4o".to_string()]);
        assert!(result.is_err());

        // Scenario 5: Reset claude → works again
        r.reset_health("claude");
        let resolved = r
            .resolve_with_fallback("claude-sonnet-4", &["gpt-4o".to_string()])
            .unwrap();
        assert_eq!(resolved.provider_name, "claude");
        assert!(!resolved.is_fallback);
    }

    #[test]
    fn resolve_uses_degraded_provider_after_cooldown_expires() {
        use crate::providers::health::ProviderHealth;
        use std::time::{Duration, Instant};

        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));

        // Manually inject a Degraded state with an expired cooldown
        {
            let mut state = r.state.write().unwrap_or_else(|e| e.into_inner());
            state.health.insert(
                "openai".to_string(),
                ProviderHealth::Degraded {
                    since: Instant::now() - Duration::from_secs(120),
                    cooldown_until: Instant::now() - Duration::from_secs(1), // expired
                    consecutive_failures: 2,
                },
            );
        }

        // openai is Degraded but cooldown expired → should be selected (not fallback)
        let resolved = r
            .resolve_with_fallback("gpt-4o", &["claude-opus-4-6".to_string()])
            .unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model, "gpt-4o");
        assert!(
            !resolved.is_fallback,
            "degraded-past-cooldown provider should be primary, not fallback"
        );
    }
}
