//! Prompt builder for Agent Loop
//!
//! This module builds prompts for the LLM thinking step,
//! including system prompts and message history.

mod cache;
mod messages;
mod sections;

#[cfg(test)]
mod tests;

pub use messages::{Message, MessageRole};

use crate::config::ProfileConfig;
use crate::dispatcher::tool_index::HydrationResult;
use crate::agent_loop::ToolInfo;

use super::prompt_layer::{AssemblyPath, LayerInput};
use super::prompt_mode::PromptMode;
use super::prompt_pipeline::PromptPipeline;
use super::soul::SoulManifest;

/// System prompt part with optional cache flag
///
/// When using Anthropic's prompt caching, static content can be cached
/// for improved performance. This struct allows splitting the system
/// prompt into cacheable and non-cacheable parts.
#[derive(Debug, Clone)]
pub struct SystemPromptPart {
    /// The content of this part
    pub content: String,
    /// Whether this part should be cached (for Anthropic)
    pub cache: bool,
}

/// Configuration for prompt building
#[derive(Debug, Clone)]
pub struct PromptConfig {
    /// Assistant persona/name
    pub persona: Option<String>,
    /// Response language
    pub language: Option<String>,
    /// Custom instructions to append
    pub custom_instructions: Option<String>,
    /// Maximum tokens for tool descriptions
    pub max_tool_description_tokens: usize,
    /// Runtime capabilities (pre-formatted prompt text)
    /// Describes available runtimes (Python, Node.js, FFmpeg, etc.)
    pub runtime_capabilities: Option<String>,
    /// Generation models (pre-formatted prompt text)
    /// Describes available image/video/audio generation models and aliases
    pub generation_models: Option<String>,
    /// Tool index for smart tool discovery (pre-formatted markdown)
    /// When set, enables two-stage tool discovery mode:
    /// - Tools passed to `build_system_prompt` get full schema
    /// - Additional tools are listed in this index (name + summary only)
    /// - LLM can call `get_tool_schema` to get full schema for indexed tools
    pub tool_index: Option<String>,
    /// Skill execution mode - when true, enforces strict workflow completion
    /// The agent MUST complete all steps specified in the skill instructions
    /// and generate all required output files before calling `complete`
    pub skill_mode: bool,
    /// Enable thinking transparency guidance
    /// When true, adds guidance for structured reasoning output
    /// (Observation -> Analysis -> Planning -> Decision)
    pub thinking_transparency: bool,
    /// Skill instructions injected from SkillSystem snapshot (XML format)
    /// When set, these are appended to the system prompt to inform the LLM
    /// about available skills from the SkillSystem v2
    pub skill_instructions: Option<String>,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            persona: None,
            language: None,
            custom_instructions: None,
            max_tool_description_tokens: 2000,
            runtime_capabilities: None,
            generation_models: None,
            tool_index: None,
            skill_mode: false,
            thinking_transparency: false,
            skill_instructions: None,
        }
    }
}

/// Prompt builder for Agent Loop thinking
pub struct PromptBuilder {
    config: PromptConfig,
    pipeline: PromptPipeline,
}

impl PromptBuilder {
    /// Create a new prompt builder
    pub fn new(config: PromptConfig) -> Self {
        let pipeline = PromptPipeline::default_layers();
        Self { config, pipeline }
    }

    /// Build the system prompt
    pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
        let input = LayerInput::basic(&self.config, tools);
        self.pipeline.execute(AssemblyPath::Basic, &input)
    }

    /// Build system prompt with hydrated tools from semantic retrieval
    ///
    /// This method builds a complete system prompt using HydrationResult
    /// instead of the traditional ToolInfo array, enabling semantic tool
    /// selection based on query relevance.
    pub fn build_system_prompt_with_hydration(&self, hydration: &HydrationResult) -> String {
        let input = LayerInput::hydration(&self.config, hydration);
        self.pipeline.execute(AssemblyPath::Hydration, &input)
    }

    /// Build system prompt with soul section at the top
    ///
    /// This is the primary entry point when using the Embodiment Engine.
    /// Soul content appears at the very top of the prompt for highest priority.
    /// When a workspace profile is provided, its system_prompt is injected
    /// between Soul (priority 50) and Role (priority 100).
    pub fn build_system_prompt_with_soul(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_profile(profile);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }

    /// Build system prompt with hooks applied.
    ///
    /// Hooks are called in order: before_prompt_build on each hook,
    /// then normal prompt building, then after_prompt_build on each hook.
    pub fn build_system_prompt_with_hooks(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
        hooks: &[Box<dyn crate::thinker::prompt_hooks::PromptHook>],
    ) -> String {
        // Clone config so hooks can modify it
        let mut config = self.config.clone();

        // Before hooks
        for hook in hooks {
            if let Err(e) = hook.before_prompt_build(&mut config) {
                tracing::warn!(hook = hook.name(), error = %e, "Prompt hook before_build failed");
            }
        }

        // Build with potentially modified config
        let builder = PromptBuilder::new(config);
        let mut prompt = builder.build_system_prompt_with_soul(tools, soul, profile);

        // After hooks
        for hook in hooks {
            if let Err(e) = hook.after_prompt_build(&mut prompt) {
                tracing::warn!(hook = hook.name(), error = %e, "Prompt hook after_build failed");
            }
        }

        prompt
    }

    /// Build system prompt with explicit mode control.
    pub fn build_system_prompt_with_mode(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
        mode: PromptMode,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_profile(profile)
            .with_mode(mode);
        self.pipeline.execute_with_mode(AssemblyPath::Soul, &input, mode)
    }

    /// Build system prompt using ResolvedContext
    ///
    /// This is the new entry point that uses the two-phase filtered context
    /// from the ContextAggregator. The pipeline layers handle all sections
    /// (runtime context, environment, security, protocol tokens, etc.)
    /// in priority order.
    pub fn build_system_prompt_with_context(&self, ctx: &super::context::ResolvedContext) -> String {
        let input = LayerInput::context(&self.config, ctx);
        self.pipeline.execute(AssemblyPath::Context, &input)
    }
}
