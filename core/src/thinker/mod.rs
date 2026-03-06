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
pub mod inbound_context;
pub mod interaction;
pub mod model_router;
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
pub mod tool_filter;
pub mod user_profile;
pub mod virtual_tools;
pub mod workspace_files;

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
pub use prompt_budget::{PromptResult, TokenBudget, TruncationStat, TruncationWarning};
pub use prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
pub use prompt_mode::PromptMode;
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
use crate::agents::thinking::ThinkLevel;
use crate::dispatcher::{ToolDefinition, UnifiedTool};
use crate::error::Result;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
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
    /// Workspace root for bootstrap file loading. When set, enables BootstrapLayer.
    pub bootstrap_workspace: Option<std::path::PathBuf>,
    /// Loaded workspace files (SOUL.md, IDENTITY.md, etc.) for prompt injection.
    pub workspace_files: Option<workspace_files::WorkspaceFiles>,
    /// Per-request inbound context (sender, channel, session metadata).
    pub inbound_context: Option<inbound_context::InboundContext>,
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
            tool_filter: ToolFilter::new(config.tool_filter.clone())
                .with_profile(config.active_profile.clone()),
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

    /// Resolve the effective model, applying workspace profile override if set.
    ///
    /// If the active workspace profile specifies a `model`, it takes precedence
    /// over the model router's selection. This allows workspace-specific model
    /// binding (e.g., a "coding" workspace always uses claude-sonnet).
    ///
    /// Resolve per-request generation parameters from workspace profile.
    ///
    /// Returns (temperature, max_tokens) overrides that take precedence over
    /// provider config values. Used by `call_llm_with_level()` via
    /// `AiProvider::process_with_overrides()`.
    fn resolve_generation_params(&self) -> (Option<f32>, Option<u32>) {
        match &self.config.active_profile {
            Some(profile) => (profile.temperature, profile.max_tokens),
            None => (None, None),
        }
    }

    fn resolve_model(&self, observation: &Observation) -> ModelId {
        let default_model_id = self.select_model(observation);

        if let Some(ref profile) = self.config.active_profile {
            if profile.model.is_some() {
                let overridden = profile.effective_model(default_model_id.as_str());
                tracing::debug!(
                    default_model = %default_model_id.as_str(),
                    profile_model = %overridden,
                    "Workspace profile overrides model selection"
                );
                return ModelId::new(overridden);
            }
        }

        default_model_id
    }

    /// Build the complete prompt
    ///
    /// If a soul manifest is configured, identity is injected at the top of the
    /// system prompt via `build_system_prompt_with_soul()`.
    /// If an active workspace profile is configured, its system_prompt is
    /// injected between Soul (priority 50) and Role (priority 100).
    fn build_prompt(&self, state: &LoopState, tools: &[ToolInfo], observation: &Observation) -> (String, Vec<Message>) {
        let system = if let Some(ref soul) = self.config.soul {
            self.prompt_builder.build_system_prompt_with_full_context(
                tools,
                soul,
                self.config.active_profile.as_ref(),
                self.config.workspace_files.as_ref(),
                self.config.inbound_context.as_ref(),
            )
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

    /// Call LLM with a specific thinking level and workspace generation overrides
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

        // Resolve per-request generation params from workspace profile
        let (temperature, max_tokens) = self.resolve_generation_params();

        // Use process_with_overrides which combines thinking + generation params
        if temperature.is_some() || max_tokens.is_some() {
            tracing::debug!(
                ?temperature,
                ?max_tokens,
                think_level = %think_level,
                provider = %provider.name(),
                "Using generation overrides from workspace profile"
            );
            provider
                .process_with_overrides(
                    &full_prompt,
                    Some(system),
                    think_level,
                    temperature,
                    max_tokens,
                )
                .await
        } else if think_level != ThinkLevel::Off && provider.supports_thinking() {
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

    /// Call LLM with native tool_use support, returning structured ProviderResponse
    ///
    /// This method uses `process_with_payload()` which passes tool definitions
    /// through the API's native tool_use mechanism (e.g., Anthropic tool_use,
    /// OpenAI function calling, Gemini function declarations).
    async fn call_llm_native(
        &self,
        provider: Arc<dyn AiProvider>,
        system: &str,
        messages: &[Message],
        think_level: ThinkLevel,
        tool_defs: Option<&[ToolDefinition]>,
    ) -> Result<ProviderResponse> {
        // Build the full prompt from messages (same as call_llm_with_level)
        let mut prompt_parts = Vec::new();

        for msg in messages {
            match msg.role {
                MessageRole::User => prompt_parts.push(format!("User: {}", msg.content)),
                MessageRole::Assistant => prompt_parts.push(format!("Assistant: {}", msg.content)),
                MessageRole::Tool => prompt_parts.push(format!("Tool Result: {}", msg.content)),
            }
        }

        let full_prompt = prompt_parts.join("\n\n");

        // Resolve per-request generation params from workspace profile
        let (temperature, max_tokens) = self.resolve_generation_params();

        // Build payload with tools
        let payload = RequestPayload::new(&full_prompt)
            .with_system(Some(system))
            .with_think_level(Some(think_level))
            .with_temperature(temperature)
            .with_max_tokens(max_tokens)
            .with_tools(tool_defs);

        provider.process_with_payload(payload).await
    }

    /// Map a native tool call from ProviderResponse to a Decision
    ///
    /// Handles both virtual tools (__complete, __ask_user, __fail) and real tools.
    fn map_native_tool_call_to_decision(
        &self,
        tc: &crate::providers::adapter::NativeToolCall,
    ) -> crate::agent_loop::Decision {
        use crate::agent_loop::Decision;

        match tc.name.as_str() {
            virtual_tools::VIRTUAL_COMPLETE => Decision::Complete {
                summary: tc
                    .arguments
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            },
            virtual_tools::VIRTUAL_ASK_USER => Decision::AskUser {
                question: tc
                    .arguments
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                options: tc
                    .arguments
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    }),
            },
            virtual_tools::VIRTUAL_FAIL => Decision::Fail {
                reason: tc
                    .arguments
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown failure")
                    .to_string(),
            },
            _ => Decision::UseTool {
                tool_name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        }
    }

    /// Build a Thinking result from a native ProviderResponse
    ///
    /// If the response contains tool calls, maps the first tool call to a Decision.
    /// If no tool calls, falls back to DecisionParser on the text content.
    fn build_thinking_from_native_response(
        &self,
        response: ProviderResponse,
    ) -> Result<Thinking> {
        if response.has_tool_calls() {
            let tc = &response.tool_calls[0];
            let decision = self.map_native_tool_call_to_decision(tc);

            // Use thinking content if available, otherwise use text
            let reasoning = response
                .thinking
                .or(response.text)
                .unwrap_or_default();

            // Use actual token count if available
            let tokens_used = response
                .usage
                .map(|u| (u.input_tokens + u.output_tokens) as usize);

            Ok(Thinking {
                reasoning: Some(reasoning),
                decision,
                structured: None,
                tokens_used,
                tool_call_id: Some(tc.id.clone()),
            })
        } else if let Some(ref text) = response.text {
            // Fallback: no tool calls, try DecisionParser on text
            tracing::warn!(
                "Native tool_use provider returned text without tool calls, falling back to DecisionParser"
            );
            self.parse_response(text)
        } else {
            Err(crate::error::AlephError::ProviderError {
                message: "Empty LLM response (no text and no tool calls)".into(),
                suggestion: Some(
                    "The provider returned an empty response. Try again or switch providers."
                        .into(),
                ),
            })
        }
    }

    /// Collect tool definitions for native tool_use mode
    ///
    /// Converts filtered ToolInfo to ToolDefinition and appends virtual tools.
    fn collect_native_tool_defs(&self, filtered_tools: &[ToolInfo]) -> Vec<ToolDefinition> {
        use crate::dispatcher::ToolCategory;

        let mut tool_defs: Vec<ToolDefinition> = filtered_tools
            .iter()
            .map(|t| {
                let params = serde_json::from_str(&t.parameters_schema)
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
                ToolDefinition::new(&t.name, &t.description, params, ToolCategory::Builtin)
            })
            .collect();

        tool_defs.extend(virtual_tools::virtual_tool_definitions());
        tool_defs
    }

    /// Create a PromptConfig with native_tools_enabled set to true
    ///
    /// Clones the current config and sets the flag, causing ToolsLayer and
    /// ResponseFormatLayer to skip their injection.
    fn native_prompt_config(&self) -> PromptConfig {
        let mut config = self.config.prompt.clone();
        config.native_tools_enabled = true;
        config
    }

    /// Build prompt with native tools mode (skips ToolsLayer and ResponseFormatLayer)
    fn build_prompt_native(
        &self,
        state: &LoopState,
        tools: &[ToolInfo],
        observation: &Observation,
    ) -> (String, Vec<Message>) {
        let native_config = self.native_prompt_config();
        let native_builder = PromptBuilder::new(native_config);
        let system = if let Some(ref soul) = self.config.soul {
            native_builder.build_system_prompt_with_full_context(
                tools,
                soul,
                self.config.active_profile.as_ref(),
                self.config.workspace_files.as_ref(),
                self.config.inbound_context.as_ref(),
            )
        } else {
            native_builder.build_system_prompt(tools)
        };
        let messages = native_builder.build_messages(&state.original_request, observation);
        (system, messages)
    }

    /// Build prompt with hydration and native tools mode
    fn build_prompt_with_hydration_native(
        &self,
        state: &LoopState,
        hydration: &crate::dispatcher::tool_index::HydrationResult,
        observation: &Observation,
    ) -> (String, Vec<Message>) {
        let native_config = self.native_prompt_config();
        let native_builder = PromptBuilder::new(native_config);
        let system = native_builder.build_system_prompt_with_hydration(hydration);
        let messages = native_builder.build_messages(&state.original_request, observation);
        (system, messages)
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

        // 2. Select model (with workspace profile override)
        let model_id = self.resolve_model(&observation);

        // 3. Get provider for model
        let provider = self
            .providers
            .get(&model_id)
            .unwrap_or_else(|| self.providers.default_provider());

        // 4. Check if provider supports native tools → dual-path
        if provider.supports_native_tools() {
            // Native tool_use path
            tracing::info!(
                subsystem = "thinker",
                event = "native_tool_use_hydration",
                model = %model_id.as_str(),
                provider = %provider.name(),
                think_level = %level,
                tool_count = tools_for_observation.len(),
                "Using native tool_use path with hydrated tools"
            );

            let tool_defs = self.collect_native_tool_defs(&tools_for_observation);

            // Build prompt with native mode (skips ToolsLayer + ResponseFormatLayer)
            let (system, messages) =
                self.build_prompt_with_hydration_native(state, hydration, &observation);

            let response = self
                .call_llm_native(provider, &system, &messages, level, Some(&tool_defs))
                .await?;

            let thinking = self.build_thinking_from_native_response(response)?;
            self.decision_parser.validate(&thinking.decision)?;
            Ok(thinking)
        } else {
            // Existing JSON-in-text path (unchanged)
            let (system, messages) =
                self.build_prompt_with_hydration(state, hydration, &observation);

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

            let thinking = self.parse_response(&response)?;
            self.decision_parser.validate(&thinking.decision)?;
            Ok(thinking)
        }
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

        tracing::debug!(
            input_tools = tools.len(),
            filtered_tools = filtered_tools.len(),
            "Thinker tool filtering"
        );

        // 3. Rebuild observation with filtered tools
        let observation = self.build_observation(state, &filtered_tools);

        // 4. Select model (with workspace profile override)
        let model_id = self.resolve_model(&observation);

        // 5. Get provider for model
        let provider = self
            .providers
            .get(&model_id)
            .unwrap_or_else(|| self.providers.default_provider());

        tracing::info!(
            subsystem = "thinker",
            event = "provider_selected",
            model = %model_id.as_str(),
            provider = %provider.name(),
            think_level = %level,
            tool_count = filtered_tools.len(),
            native_tools = provider.supports_native_tools(),
            "thinker selected provider for LLM call"
        );

        // 6. Dual-path: native tool_use vs JSON-in-text
        if provider.supports_native_tools() {
            // === Native tool_use path ===
            // Tool definitions are passed via the API; ToolsLayer and
            // ResponseFormatLayer are skipped in the system prompt.
            let tool_defs = self.collect_native_tool_defs(&filtered_tools);

            // Build prompt with native mode (skips ToolsLayer + ResponseFormatLayer)
            let (system, messages) =
                self.build_prompt_native(state, &filtered_tools, &observation);

            let response = self
                .call_llm_native(
                    provider.clone(),
                    &system,
                    &messages,
                    level,
                    Some(&tool_defs),
                )
                .await?;

            tracing::debug!(
                has_tool_calls = response.has_tool_calls(),
                has_text = response.text.is_some(),
                stop_reason = ?response.stop_reason,
                think_level = %level,
                "Native tool_use LLM response"
            );

            let mut thinking = self.build_thinking_from_native_response(response)?;

            // If token usage was not populated by the provider, estimate it
            if thinking.tokens_used.is_none() {
                let input_chars: usize =
                    system.len() + messages.iter().map(|m| m.content.len()).sum::<usize>();
                let reasoning_len = thinking
                    .reasoning
                    .as_ref()
                    .map(|r| r.len())
                    .unwrap_or(0);
                let estimated_tokens = (input_chars + reasoning_len) / 4;
                thinking.tokens_used = Some(estimated_tokens);
            }

            tracing::info!(
                subsystem = "thinker",
                event = "native_response_completed",
                model = %model_id.as_str(),
                tokens_used = ?thinking.tokens_used,
                decision_type = thinking.decision.decision_type(),
                "thinker native tool_use response completed"
            );

            self.decision_parser.validate(&thinking.decision)?;
            Ok(thinking)
        } else {
            // === Existing JSON-in-text path (unchanged) ===

            // Build prompt (includes ToolsLayer and ResponseFormatLayer)
            let (system, messages) = self.build_prompt(state, &filtered_tools, &observation);

            // Call LLM with specified thinking level
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

            // Parse response
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

            // Estimate token usage from response length.
            // This is a rough approximation (~4 chars per token) until providers
            // return actual usage metadata. The guard needs a non-zero value to work.
            let input_chars: usize =
                system.len() + messages.iter().map(|m| m.content.len()).sum::<usize>();
            let estimated_tokens = (input_chars + response.len()) / 4;
            thinking.tokens_used = Some(estimated_tokens);

            tracing::info!(
                subsystem = "thinker",
                event = "response_completed",
                model = %model_id.as_str(),
                estimated_tokens = estimated_tokens,
                response_len = response.len(),
                decision_type = thinking.decision.decision_type(),
                "thinker LLM response completed"
            );

            // Validate decision
            self.decision_parser.validate(&thinking.decision)?;

            Ok(thinking)
        }
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

    #[test]
    fn test_resolve_model_without_profile() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });

        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let observation = crate::agent_loop::Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![],
            current_step: 0,
            total_tokens: 0,
        };

        // Without profile, resolve_model returns the router's selection
        let model_id = thinker.resolve_model(&observation);
        // Default ModelRoutingConfig uses empty strings, so selector returns ""
        let default_id = thinker.select_model(&observation);
        assert_eq!(model_id.as_str(), default_id.as_str());
    }

    #[test]
    fn test_resolve_model_with_profile_override() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });

        let config = ThinkerConfig {
            active_profile: Some(crate::config::ProfileConfig {
                model: Some("gemini-1.5-pro".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let thinker = Thinker::new(registry, config);

        let observation = crate::agent_loop::Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![],
            current_step: 0,
            total_tokens: 0,
        };

        // With profile model set, resolve_model overrides to profile's model
        let model_id = thinker.resolve_model(&observation);
        assert_eq!(model_id.as_str(), "gemini-1.5-pro");
    }

    #[test]
    fn test_resolve_model_with_profile_no_model() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });

        let config = ThinkerConfig {
            active_profile: Some(crate::config::ProfileConfig {
                // Profile exists but model is None — should fall through to router
                model: None,
                temperature: Some(0.3),
                ..Default::default()
            }),
            ..Default::default()
        };

        let thinker = Thinker::new(registry, config);

        let observation = crate::agent_loop::Observation {
            history_summary: String::new(),
            recent_steps: vec![],
            available_tools: vec![],
            attachments: vec![],
            current_step: 0,
            total_tokens: 0,
        };

        // Profile without model should fall through to router's selection
        let model_id = thinker.resolve_model(&observation);
        let default_id = thinker.select_model(&observation);
        assert_eq!(model_id.as_str(), default_id.as_str());
    }

    #[test]
    fn test_resolve_generation_params_with_profile() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });

        let config = ThinkerConfig {
            active_profile: Some(crate::config::ProfileConfig {
                temperature: Some(0.3),
                max_tokens: Some(4096),
                ..Default::default()
            }),
            ..Default::default()
        };

        let thinker = Thinker::new(registry, config);
        let (temp, max) = thinker.resolve_generation_params();
        assert_eq!(temp, Some(0.3));
        assert_eq!(max, Some(4096));
    }

    #[test]
    fn test_resolve_generation_params_without_profile() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });

        let thinker = Thinker::new(registry, ThinkerConfig::default());
        let (temp, max) = thinker.resolve_generation_params();
        assert_eq!(temp, None);
        assert_eq!(max, None);
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

    // =========================================================================
    // Native tool_use decision mapping tests
    // =========================================================================

    #[test]
    fn test_map_native_tool_call_to_decision_complete() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let tc = crate::providers::adapter::NativeToolCall {
            id: "call_1".into(),
            name: "__complete".into(),
            arguments: serde_json::json!({"summary": "Task done successfully"}),
        };

        let decision = thinker.map_native_tool_call_to_decision(&tc);
        match decision {
            crate::agent_loop::Decision::Complete { summary } => {
                assert_eq!(summary, "Task done successfully");
            }
            other => panic!("Expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_map_native_tool_call_to_decision_ask_user() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let tc = crate::providers::adapter::NativeToolCall {
            id: "call_2".into(),
            name: "__ask_user".into(),
            arguments: serde_json::json!({
                "question": "Which format?",
                "options": ["PNG", "JPEG"]
            }),
        };

        let decision = thinker.map_native_tool_call_to_decision(&tc);
        match decision {
            crate::agent_loop::Decision::AskUser { question, options } => {
                assert_eq!(question, "Which format?");
                assert_eq!(options, Some(vec!["PNG".to_string(), "JPEG".to_string()]));
            }
            other => panic!("Expected AskUser, got {:?}", other),
        }
    }

    #[test]
    fn test_map_native_tool_call_to_decision_fail() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let tc = crate::providers::adapter::NativeToolCall {
            id: "call_3".into(),
            name: "__fail".into(),
            arguments: serde_json::json!({"reason": "File not found"}),
        };

        let decision = thinker.map_native_tool_call_to_decision(&tc);
        match decision {
            crate::agent_loop::Decision::Fail { reason } => {
                assert_eq!(reason, "File not found");
            }
            other => panic!("Expected Fail, got {:?}", other),
        }
    }

    #[test]
    fn test_map_native_tool_call_to_decision_real_tool() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let tc = crate::providers::adapter::NativeToolCall {
            id: "call_4".into(),
            name: "web_search".into(),
            arguments: serde_json::json!({"query": "rust async"}),
        };

        let decision = thinker.map_native_tool_call_to_decision(&tc);
        match decision {
            crate::agent_loop::Decision::UseTool {
                tool_name,
                arguments,
            } => {
                assert_eq!(tool_name, "web_search");
                assert_eq!(arguments["query"], "rust async");
            }
            other => panic!("Expected UseTool, got {:?}", other),
        }
    }

    #[test]
    fn test_build_thinking_from_native_response_with_tool_calls() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let response = ProviderResponse {
            text: None,
            tool_calls: vec![crate::providers::adapter::NativeToolCall {
                id: "call_5".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "ls -la"}),
            }],
            thinking: Some("I need to list files".into()),
            stop_reason: crate::providers::adapter::StopReason::ToolUse,
            usage: Some(crate::providers::adapter::TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: None,
            }),
        };

        let thinking = thinker.build_thinking_from_native_response(response).unwrap();
        assert_eq!(thinking.reasoning.as_deref(), Some("I need to list files"));
        assert_eq!(thinking.tokens_used, Some(150));
        assert_eq!(thinking.tool_call_id.as_deref(), Some("call_5"));
        assert!(matches!(
            thinking.decision,
            crate::agent_loop::Decision::UseTool { .. }
        ));
    }

    #[test]
    fn test_build_thinking_from_native_response_text_fallback() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        // Provider returned text without tool calls — should fallback to DecisionParser
        let response = ProviderResponse {
            text: Some(r#"{"reasoning": "done", "action": {"type": "complete", "summary": "All done"}}"#.into()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: crate::providers::adapter::StopReason::EndTurn,
            usage: None,
        };

        let thinking = thinker.build_thinking_from_native_response(response).unwrap();
        assert!(matches!(
            thinking.decision,
            crate::agent_loop::Decision::Complete { .. }
        ));
    }

    #[test]
    fn test_build_thinking_from_native_response_empty_error() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let response = ProviderResponse::default();

        let result = thinker.build_thinking_from_native_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_collect_native_tool_defs_includes_virtual_tools() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let tools = vec![
            crate::agent_loop::ToolInfo {
                name: "bash".into(),
                description: "Run commands".into(),
                parameters_schema: r#"{"type":"object","properties":{"command":{"type":"string"}}}"#.into(),
                category: None,
            },
        ];

        let defs = thinker.collect_native_tool_defs(&tools);
        // Should have 1 real tool + 3 virtual tools
        assert_eq!(defs.len(), 4);
        assert_eq!(defs[0].name, "bash");
        assert_eq!(defs[1].name, "__complete");
        assert_eq!(defs[2].name, "__ask_user");
        assert_eq!(defs[3].name, "__fail");
    }

    #[test]
    fn test_native_prompt_config_sets_flag() {
        let provider = Arc::new(MockProvider::new("{}"));
        let registry = Arc::new(MockProviderRegistry { provider });
        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let config = thinker.native_prompt_config();
        assert!(config.native_tools_enabled);
    }

    #[tokio::test]
    async fn test_thinker_native_tool_use_path() {
        // Mock provider that supports native tools and returns a ProviderResponse with tool calls
        struct NativeMockProvider;

        impl AiProvider for NativeMockProvider {
            fn process(
                &self,
                _input: &str,
                _system_prompt: Option<&str>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<String>> + Send + '_>> {
                Box::pin(async { Ok(String::new()) })
            }

            fn process_with_payload<'a>(
                &'a self,
                _payload: crate::providers::adapter::RequestPayload<'a>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![crate::providers::adapter::NativeToolCall {
                            id: "call_native_1".into(),
                            name: "__complete".into(),
                            arguments: serde_json::json!({"summary": "Native tool use works!"}),
                        }],
                        thinking: Some("Thinking through native path".into()),
                        stop_reason: crate::providers::adapter::StopReason::ToolUse,
                        usage: Some(crate::providers::adapter::TokenUsage {
                            input_tokens: 200,
                            output_tokens: 100,
                            cache_read_tokens: None,
                        }),
                    })
                })
            }

            fn supports_native_tools(&self) -> bool {
                true
            }

            fn name(&self) -> &str {
                "native-mock"
            }

            fn color(&self) -> &str {
                "#00FF00"
            }
        }

        let provider: Arc<dyn AiProvider> = Arc::new(NativeMockProvider);
        let registry = Arc::new(MockProviderRegistry { provider });

        let thinker = Thinker::new(registry, ThinkerConfig::default());

        let state = LoopState::new(
            "test-session".to_string(),
            "Test native tool use".to_string(),
            RequestContext::empty(),
        );

        let result = thinker.think(&state, &[]).await;
        assert!(result.is_ok(), "Native tool_use path should succeed");

        let thinking = result.unwrap();
        assert_eq!(
            thinking.reasoning.as_deref(),
            Some("Thinking through native path")
        );
        assert_eq!(thinking.tokens_used, Some(300));
        match thinking.decision {
            crate::agent_loop::Decision::Complete { summary } => {
                assert_eq!(summary, "Native tool use works!");
            }
            other => panic!("Expected Complete, got {:?}", other),
        }
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
