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

pub mod decision_parser;
pub mod model_router;
pub mod prompt_builder;
pub mod tool_filter;

use std::sync::Arc;

pub use decision_parser::DecisionParser;
pub use model_router::{ModelId, ModelRouter, RoutingCondition, RoutingRule};
pub use prompt_builder::{Message, MessageRole, PromptBuilder, PromptConfig};
pub use tool_filter::{ToolFilter, ToolFilterConfig};

use crate::agent_loop::{
    CompressionConfig, LoopState, ModelRoutingConfig, Observation, ThinkerTrait, Thinking, ToolInfo,
};
use crate::dispatcher::UnifiedTool;
use crate::error::Result;
use crate::providers::AiProvider;

/// Configuration for Thinker
#[derive(Debug, Clone)]
pub struct ThinkerConfig {
    /// Prompt configuration
    pub prompt: PromptConfig,
    /// Tool filter configuration
    pub tool_filter: ToolFilterConfig,
    /// Model routing configuration
    pub model_routing: ModelRoutingConfig,
    /// Compression configuration (for observation building)
    pub compression: CompressionConfig,
}

impl Default for ThinkerConfig {
    fn default() -> Self {
        Self {
            prompt: PromptConfig::default(),
            tool_filter: ToolFilterConfig::default(),
            model_routing: ModelRoutingConfig::default(),
            compression: CompressionConfig::default(),
        }
    }
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
    model_router: ModelRouter,
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
            model_router: ModelRouter::new(config.model_routing.clone()),
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
        self.model_router.select(observation)
    }

    /// Build the complete prompt
    fn build_prompt(&self, state: &LoopState, tools: &[ToolInfo], observation: &Observation) -> (String, Vec<Message>) {
        let system = self.prompt_builder.build_system_prompt(tools);
        let messages = self.prompt_builder.build_messages(&state.original_request, observation);
        (system, messages)
    }

    /// Call LLM and get response
    async fn call_llm(
        &self,
        provider: Arc<dyn AiProvider>,
        system: &str,
        messages: &[Message],
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

        provider
            .process(&full_prompt, Some(system))
            .await
    }

    /// Parse LLM response into Thinking
    fn parse_response(&self, response: &str) -> Result<Thinking> {
        self.decision_parser.parse_with_fallback(response)
    }
}

#[async_trait::async_trait]
impl<P: ProviderRegistry + 'static> ThinkerTrait for Thinker<P> {
    async fn think(&self, state: &LoopState, tools: &[UnifiedTool]) -> Result<Thinking> {
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

        // 7. Call LLM
        let response = self.call_llm(provider, &system, &messages).await?;

        // 8. Parse response
        let thinking = self.parse_response(&response)?;

        // 9. Validate decision
        self.decision_parser.validate(&thinking.decision)?;

        Ok(thinking)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_loop::RequestContext;
    use std::sync::Mutex;

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
