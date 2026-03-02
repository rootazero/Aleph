//! Thinker - LLM decision-making layer for Agent Loop
//!
//! This module provides the thinking layer that:
//! - Builds observations from state
//! - Filters tools based on context
//! - Routes to appropriate models
//! - Constructs prompts
//! - Calls LLM and parses decisions
//!
//! # Architecture
//!
//! ```text
//! LoopState + Tools
//!       │
//!       ▼
//! ┌─────────────────────────────────────┐
//! │            Thinker                  │
//! │  ┌─────────────────────────────┐    │
//! │  │ 1. Build Observation        │    │
//! │  │ 2. Filter Tools             │    │
//! │  │ 3. Route Model              │    │
//! │  │ 4. Build Prompt             │    │
//! │  │ 5. Call LLM                 │    │
//! │  │ 6. Parse Decision           │    │
//! │  └─────────────────────────────┘    │
//! └─────────────────────────────────────┘
//!       │
//!       ▼
//!    Thinking { reasoning, decision }
//! ```

pub mod cache;
pub mod channel_behavior;
pub mod context;
pub mod decision_parser;
pub mod identity;
pub mod interaction;
pub mod model_router;
pub mod prompt_builder;
pub mod prompt_hooks;
pub mod prompt_layer;
pub mod prompt_pipeline;
pub mod layers;
pub mod security_context;
pub mod soul;
pub mod prompt_sanitizer;
pub mod protocol_tokens;
pub mod runtime_context;
pub mod streaming;
pub mod tool_filter;
pub mod user_profile;

use crate::sync_primitives::Arc;

pub use cache::{
    AnthropicCacheStrategy, CacheContext, CacheControl, CacheStrategy, CacheableContentBlock,
    GeminiCacheCreateRequest, GeminiCacheCreateResponse, GeminiCacheStrategy, GeminiContent,
    GeminiPart, ProviderType, SystemPromptCache, TransparentCacheStrategy,
    get_cache_strategy, GEMINI_CACHE_TTL_SECS, MIN_CACHE_TOKENS,
};
pub use decision_parser::DecisionParser;
pub use model_router::{ModelId, RoutingCondition, RoutingRule, ThinkerModelSelector};

/// Deprecated alias for backward compatibility
#[deprecated(since = "0.2.0", note = "Use ThinkerModelSelector instead")]
pub type ModelRouter = ThinkerModelSelector;
pub use prompt_builder::{Message, MessageRole, PromptBuilder, PromptConfig};
pub use prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
pub use prompt_pipeline::PromptPipeline;
pub use tool_filter::{IntentFilterConfig, IntentFilterResult, ToolFilter, ToolFilterConfig};
pub use interaction::{Capability, InteractionConstraints, InteractionManifest, InteractionParadigm};
pub use security_context::{
    ElevatedPolicy, SandboxLevel, SecurityContext, ToolPermission, is_network_tool,
};
pub use context::{
    ContextAggregator, DisableReason, DisabledTool, EnvironmentContract, ResolvedContext,
};
pub use soul::{FormattingStyle, RelationshipMode, SoulLoadError, SoulManifest, SoulVoice, Verbosity};
pub use protocol_tokens::ProtocolToken;
pub use runtime_context::RuntimeContext;
pub use identity::{IdentityResolver, IdentitySource, IdentitySourceType};

use crate::agent_loop::{
    CompressionConfig, LoopState, ModelRoutingConfig, Observation, ThinkerTrait, Thinking, ToolInfo,
};
use crate::agents::thinking::ThinkLevel;
use crate::dispatcher::UnifiedTool;
use crate::error::Result;
use crate::providers::AiProvider;

/// Configuration for Thinker
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct ThinkerConfig {
    /// Prompt configuration
    pub prompt: PromptConfig,
    /// Tool filter configuration
    pub tool_filter: ToolFilterConfig,
    /// Model routing configuration
    pub model_routing: ModelRoutingConfig,
    /// Compression configuration (for observation building)
    pub compression: CompressionConfig,
    /// Thinking level for LLM reasoning depth
    pub think_level: ThinkLevel,
    /// Soul manifest for identity injection into prompts.
    /// When set, the Thinker uses `build_system_prompt_with_soul()`
    /// so that identity appears at the top of every system prompt.
    pub soul: Option<soul::SoulManifest>,
    /// Active workspace profile configuration.
    /// When set, provides workspace-specific overrides (model, temperature,
    /// tool whitelist, system_prompt) resolved from the user's active workspace.
    pub active_profile: Option<crate::config::ProfileConfig>,
}


/// Provider registry for model routing
pub trait ProviderRegistry: Send + Sync {
    /// Get provider for a specific model
    fn get(&self, model: &ModelId) -> Option<Arc<dyn AiProvider>>;

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
    fn get(&self, _model: &ModelId) -> Option<Arc<dyn AiProvider>> {
        Some(self.provider.clone())
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        self.provider.clone()
    }
}

/// Thinker - The thinking layer of Agent Loop
///
/// Thinker is responsible for:
/// 1. Building observations from current state
/// 2. Filtering tools based on context
/// 3. Routing to the appropriate model
/// 4. Constructing prompts
/// 5. Calling the LLM
/// 6. Parsing the response into a decision
pub struct Thinker<P: ProviderRegistry> {
    providers: Arc<P>,
    tool_filter: ToolFilter,
    prompt_builder: PromptBuilder,
    model_selector: ThinkerModelSelector,
    decision_parser: DecisionParser,
    config: ThinkerConfig,
}

impl<P: ProviderRegistry> Thinker<P> {
    /// Create a new Thinker
    pub fn new(providers: Arc<P>, config: ThinkerConfig) -> Self {
        Self {
            providers,
            tool_filter: ToolFilter::new(config.tool_filter.clone()),
            prompt_builder: PromptBuilder::new(config.prompt.clone()),
            model_selector: ThinkerModelSelector::new(config.model_routing.clone()),
            decision_parser: DecisionParser::new(),
            config,
        }
    }

    /// Build observation from state
    fn build_observation(&self, state: &LoopState, tools: &[ToolInfo]) -> Observation {
        self.prompt_builder.build_observation(
            state,
            tools,
            self.config.compression.recent_window_size,
        )
    }

    /// Filter tools based on observation
    fn filter_tools(&self, all_tools: &[UnifiedTool], observation: &Observation) -> Vec<ToolInfo> {
        self.tool_filter.filter(all_tools, observation)
    }

    /// Select model based on observation
    fn select_model(&self, observation: &Observation) -> ModelId {
        self.model_selector.select(observation)
    }

    /// Build the complete prompt
    ///
    /// If a soul manifest is configured, identity is injected at the top of the
    /// system prompt via `build_system_prompt_with_soul()`.
    fn build_prompt(&self, state: &LoopState, tools: &[ToolInfo], observation: &Observation) -> (String, Vec<Message>) {
        let system = if let Some(ref soul) = self.config.soul {
            self.prompt_builder.build_system_prompt_with_soul(tools, soul)
        } else {
            self.prompt_builder.build_system_prompt(tools)
        };
        let messages = self.prompt_builder.build_messages(&state.original_request, observation);
        (system, messages)
    }

    /// Build prompt using HydrationResult from semantic tool retrieval
    ///
    /// This is the preferred method when HydrationPipeline is available,
    /// as it provides progressive disclosure of tool schemas based on semantic relevance.
    fn build_prompt_with_hydration(
        &self,
        state: &LoopState,
        hydration: &crate::dispatcher::tool_index::HydrationResult,
        observation: &Observation,
    ) -> (String, Vec<Message>) {
        let system = self.prompt_builder.build_system_prompt_with_hydration(hydration);
        let messages = self.prompt_builder.build_messages(&state.original_request, observation);
        (system, messages)
    }

    /// Call LLM with a specific thinking level
    async fn call_llm_with_level(
        &self,
        provider: Arc<dyn AiProvider>,
        system: &str,
        messages: &[Message],
        think_level: ThinkLevel,
    ) -> Result<String> {
        // Build the full prompt from messages
        let mut prompt_parts = Vec::new();

        for msg in messages {
            match msg.role {
                MessageRole::User => prompt_parts.push(format!("User: {}", msg.content)),
                MessageRole::Assistant => prompt_parts.push(format!("Assistant: {}", msg.content)),
                MessageRole::Tool => prompt_parts.push(format!("Tool Result: {}", msg.content)),
            }
        }

        let full_prompt = prompt_parts.join("\n\n");

        // Use thinking-aware processing if provider supports it and level is not Off
        if think_level != ThinkLevel::Off && provider.supports_thinking() {
            tracing::debug!(
                think_level = %think_level,
                provider = %provider.name(),
                "Using extended thinking"
            );
            provider
                .process_with_thinking(&full_prompt, Some(system), think_level)
                .await
        } else {
            provider
                .process(&full_prompt, Some(system))
                .await
        }
    }

    /// Parse LLM response into Thinking
    ///
    /// In skill mode, uses strict parsing to enforce JSON response format.
    /// In normal mode, allows fallback heuristics for lenient parsing.
    fn parse_response(&self, response: &str) -> Result<Thinking> {
        if self.config.prompt.skill_mode {
            // Skill mode: strict parsing, no fallback heuristics
            // This ensures the agent follows the JSON response format requirement
            self.decision_parser.parse(response)
        } else {
            // Normal mode: lenient parsing with fallback
            self.decision_parser.parse_with_fallback(response)
        }
    }

    /// Internal implementation for hydration-aware thinking
    ///
    /// This method bypasses keyword-based tool filtering in favor of
    /// semantic similarity-based retrieval from HydrationPipeline.
    async fn think_with_hydration_impl(
        &self,
        state: &LoopState,
        hydration: &crate::dispatcher::tool_index::HydrationResult,
        level: ThinkLevel,
    ) -> Result<Thinking> {
        // 1. Build observation (tools come from hydration, not filtering)
        // Use full_schema_tools as they have complete tool info
        let tools_for_observation: Vec<ToolInfo> = hydration
            .full_schema_tools
            .iter()
            .map(|t| ToolInfo {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters_schema: t.cached_schema.clone().unwrap_or_default(),
                category: None, // Hydrated tools don't carry category info
            })
            .collect();

        let observation = self.build_observation(state, &tools_for_observation);

        // 2. Select model
        let model_id = self.select_model(&observation);

        // 3. Build prompt with hydration-based tools
        let (system, messages) = self.build_prompt_with_hydration(state, hydration, &observation);

        // 4. Get provider for model
        let provider = self
            .providers
            .get(&model_id)
            .unwrap_or_else(|| self.providers.default_provider());

        // 5. Call LLM
        let response = self
            .call_llm_with_level(provider, &system, &messages, level)
            .await?;

        tracing::debug!(
            response_len = response.len(),
            response_preview = %response.chars().take(500).collect::<String>(),
            think_level = %level,
            hydration_tool_count = hydration.total_count(),
            "LLM response with hydrated tools (preview)"
        );

        // 6. Parse response
        let thinking = self.parse_response(&response)?;

        // 7. Validate decision
        self.decision_parser.validate(&thinking.decision)?;

        Ok(thinking)
    }
}

#[async_trait::async_trait]
impl<P: ProviderRegistry + 'static> ThinkerTrait for Thinker<P> {
    async fn think(&self, state: &LoopState, tools: &[UnifiedTool]) -> Result<Thinking> {
        self.think_with_level(state, tools, self.config.think_level)
            .await
    }

    async fn think_with_level(
        &self,
        state: &LoopState,
        tools: &[UnifiedTool],
        level: ThinkLevel,
    ) -> Result<Thinking> {
        // 1. Build initial observation (with empty tool list for filtering)
        let initial_observation = self.build_observation(state, &[]);

        // 2. Filter tools based on context
        let filtered_tools = self.filter_tools(tools, &initial_observation);

        // 3. Rebuild observation with filtered tools
        let observation = self.build_observation(state, &filtered_tools);

        // 4. Select model
        let model_id = self.select_model(&observation);

        // 5. Build prompt
        let (system, messages) = self.build_prompt(state, &filtered_tools, &observation);

        // 6. Get provider for model
        let provider = self
            .providers
            .get(&model_id)
            .unwrap_or_else(|| self.providers.default_provider());

        // 7. Call LLM with specified thinking level
        let response = self
            .call_llm_with_level(provider, &system, &messages, level)
            .await?;

        // Log raw LLM response for debugging parse failures
        tracing::debug!(
            response_len = response.len(),
            response_preview = %response.chars().take(500).collect::<String>(),
            think_level = %level,
            "LLM raw response (preview)"
        );

        // 8. Parse response
        let thinking = self.parse_response(&response);

        // Log parse result for debugging
        if thinking.is_err() {
            tracing::warn!(
                response_len = response.len(),
                response_full = %response,
                think_level = %level,
                "Failed to parse LLM response - full content logged"
            );
        }

        let mut thinking = thinking?;

        // 9. Estimate token usage from response length.
        // This is a rough approximation (~4 chars per token) until providers
        // return actual usage metadata. The guard needs a non-zero value to work.
        let input_chars: usize = system.len() + messages.iter().map(|m| m.content.len()).sum::<usize>();
        let estimated_tokens = (input_chars + response.len()) / 4;
        thinking.tokens_used = Some(estimated_tokens);

        // 10. Validate decision
        self.decision_parser.validate(&thinking.decision)?;

        Ok(thinking)
    }

    fn current_think_level(&self) -> ThinkLevel {
        self.config.think_level
    }

    async fn think_with_hydration(
        &self,
        state: &LoopState,
        hydration: &crate::dispatcher::tool_index::HydrationResult,
        _tools: &[UnifiedTool],
        level: ThinkLevel,
    ) -> Result<Thinking> {
        // Use internal hydration-aware thinking method
        self.think_with_hydration_impl(state, hydration, level).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::RequestContext;
    use crate::sync_primitives::Mutex;

    // Mock provider for testing
    struct MockProvider {
        response: Mutex<String>,
    }

    impl MockProvider {
        fn new(response: &str) -> Self {
            Self {
                response: Mutex::new(response.to_string()),
            }
        }
    }

    impl AiProvider for MockProvider {
        fn process(
            &self,
            _input: &str,
            _system_prompt: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
            let response = self.response.lock().unwrap().clone();
            Box::pin(async move { Ok(response) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#808080"
        }
    }

    struct MockProviderRegistry {
        provider: Arc<dyn AiProvider>,
    }

    impl ProviderRegistry for MockProviderRegistry {
        fn get(&self, _model: &ModelId) -> Option<Arc<dyn AiProvider>> {
            Some(self.provider.clone())
        }

        fn default_provider(&self) -> Arc<dyn AiProvider> {
            self.provider.clone()
        }
    }

    #[tokio::test]
    async fn test_thinker_complete_flow() {
        let response = r#"{
            "reasoning": "Task is done",
            "action": {
                "type": "complete",
                "summary": "Successfully completed the task"
            }
        }"#;

        let provider = Arc::new(MockProvider::new(response));
        let registry = Arc::new(MockProviderRegistry { provider });

        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let state = LoopState::new(
            "test-session".to_string(),
            "Test request".to_string(),
            RequestContext::empty(),
        );

        let result = thinker.think(&state, &[]).await;
        assert!(result.is_ok());

        let thinking = result.unwrap();
        assert!(matches!(
            thinking.decision,
            crate::agent_loop::Decision::Complete { .. }
        ));
    }

    #[tokio::test]
    async fn test_thinker_tool_call() {
        let response = r#"{
            "reasoning": "I need to search for information",
            "action": {
                "type": "tool",
                "tool_name": "web_search",
                "arguments": {"query": "rust tutorials"}
            }
        }"#;

        let provider = Arc::new(MockProvider::new(response));
        let registry = Arc::new(MockProviderRegistry { provider });

        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let state = LoopState::new(
            "test-session".to_string(),
            "Search for Rust tutorials".to_string(),
            RequestContext::empty(),
        );

        let result = thinker.think(&state, &[]).await;
        assert!(result.is_ok());

        let thinking = result.unwrap();
        assert!(matches!(
            thinking.decision,
            crate::agent_loop::Decision::UseTool { .. }
        ));
    }
}
