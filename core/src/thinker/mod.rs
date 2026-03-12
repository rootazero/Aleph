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
pub mod workspace_files;

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

/// Format truncation stats into a human-readable warning message.
pub fn format_truncation_warning(stats: &[prompt_budget::TruncationStat]) -> String {
    let parts: Vec<String> = stats.iter().map(|s| {
        if s.fully_removed {
            format!("{} fully removed", s.layer_name)
        } else {
            let pct = if s.original_chars > 0 {
                ((s.original_chars - s.final_chars) as f64 / s.original_chars as f64 * 100.0) as u32
            } else {
                0
            };
            format!("{} {}→{} chars (-{}%)", s.layer_name, s.original_chars, s.final_chars, pct)
        }
    }).collect();
    format!("[System] Context truncated: {}", parts.join(", "))
}

use crate::providers::AiProvider;

/// Provider registry for model routing
pub trait ProviderRegistry: Send + Sync {
    /// Get provider for a specific model
    fn get(&self, model: &str) -> Option<Arc<dyn AiProvider>>;

    /// Get default provider
    fn default_provider(&self) -> Arc<dyn AiProvider>;
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
mod truncation_warning_tests {
    use super::*;
    use prompt_budget::TruncationStat;

    #[test]
    fn format_truncation_warning_message() {
        let stats = vec![
            TruncationStat {
                layer_name: "CONTEXT.md".to_string(),
                original_chars: 45000,
                final_chars: 20000,
                fully_removed: false,
            },
            TruncationStat {
                layer_name: "guidelines".to_string(),
                original_chars: 500,
                final_chars: 0,
                fully_removed: true,
            },
        ];
        let msg = format_truncation_warning(&stats);
        assert!(msg.contains("[System] Context truncated"));
        assert!(msg.contains("CONTEXT.md"));
        assert!(msg.contains("45000"));
        assert!(msg.contains("20000"));
        assert!(msg.contains("guidelines fully removed"));
    }

    #[test]
    fn format_truncation_warning_empty_stats() {
        let stats: Vec<TruncationStat> = vec![];
        let msg = format_truncation_warning(&stats);
        assert_eq!(msg, "[System] Context truncated: ");
    }
}

#[cfg(test)]
mod swappable_registry_tests {
    use super::*;

    struct TaggedProvider { tag: String }
    impl AiProvider for TaggedProvider {
        fn process(
            &self, _input: &str, _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
            Box::pin(async { Ok(String::new()) })
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
