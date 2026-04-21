//! Prompt builder for Agent Loop
//!
//! This module builds prompts for the LLM thinking step,
//! including system prompts and message history.

mod cache;
pub mod cache_monitor;
mod sections;

#[cfg(test)]
mod tests;

use crate::tools::info::ToolInfo;
use crate::config::ProfileConfig;
use crate::dispatcher::tool_index::HydrationResult;

use crate::agents::AgentDef;

use super::identity_files::IdentityFiles;
use super::inbound_context::InboundContext;
use super::prompt_budget::{PromptResult, TokenBudget};
use super::prompt_layer::{AssemblyPath, LayerInput};
use super::prompt_mode::PromptMode;
use super::prompt_pipeline::PromptPipeline;
use super::soul::SoulManifest;

/// Parent agent's stable prompt snapshot for fork path reuse.
///
/// Contains the output of all `LayerStability::Stable` layers for a given
/// assembly path. Subagents using the fork path prepend this prefix and
/// only rebuild dynamic layers (agent role, session context, memory, etc.).
#[derive(Debug, Clone)]
pub struct PromptSnapshot {
    /// Stable layers assembly output.
    pub stable_prefix: String,
    /// Source assembly path.
    pub path: AssemblyPath,
}

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
    /// Token budget for system prompt assembly.
    pub token_budget: TokenBudget,
    /// Whether native tool_use is enabled (skip ToolsLayer and ResponseFormatLayer)
    ///
    /// When true, tool definitions are passed via the API's native tool_use mechanism
    /// rather than injected into the system prompt. This also means the LLM will
    /// respond with structured tool calls rather than JSON-in-text.
    pub native_tools_enabled: bool,
    /// Eligible skills from SkillSystem v2 snapshot for scope-aware filtering.
    pub eligible_skills: Option<Vec<crate::domain::skill::SkillManifest>>,
    /// Available agent catalog entries for AgentCatalogLayer.
    pub available_agents: Option<Vec<crate::thinker::prompt_layer::AgentCatalogEntry>>,
    /// MCP server instructions for prompt injection.
    /// Collected from connected MCP servers via `McpClient::collect_instructions()`.
    pub mcp_instructions: Option<Vec<crate::thinker::prompt_layer::McpServerInstruction>>,
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
            token_budget: TokenBudget::default(),
            native_tools_enabled: false,
            eligible_skills: None,
            available_agents: None,
            mcp_instructions: None,
        }
    }
}

/// Prompt builder for Agent Loop thinking
pub struct PromptBuilder {
    config: PromptConfig,
    pipeline: PromptPipeline,
    /// Optional agent definition for sub-agent prompt assembly.
    /// When set, `build_system_prompt` will inject agent role via `AgentRoleLayer`.
    agent_def: Option<crate::agents::AgentDef>,
    /// Optional soul manifest for identity/personality.
    /// When set, `build_system_prompt` uses the Soul assembly path.
    soul: Option<SoulManifest>,
    /// Loaded identity files (SOUL.md, IDENTITY.md, MEMORY.md, …) from
    /// `~/.aleph/agents/{agent_id}/`. When set, these are threaded into every
    /// `LayerInput` so `SoulLayer`, `IdentityFilesLayer`, `ProfileLayer`, and
    /// `CustomInstructionsLayer` can read their respective files.
    identity_files: Option<IdentityFiles>,
    /// Pre-rendered memory XML from `MemoryContextProvider::build_memory_user_message`.
    ///
    /// When set, threaded into every `LayerInput` as `memory_user_message` so
    /// `MemoryAugmentationLayer` can inject it verbatim into the system prompt.
    memory_user_message: Option<String>,
}

impl PromptBuilder {
    /// Create a new prompt builder
    pub fn new(config: PromptConfig) -> Self {
        let pipeline = PromptPipeline::default_layers();
        Self {
            config,
            pipeline,
            agent_def: None,
            soul: None,
            identity_files: None,
            memory_user_message: None,
        }
    }

    /// Attach an agent definition for sub-agent prompt assembly.
    ///
    /// When set, `build_system_prompt` injects the agent's role header
    /// and protocol sections via `AgentRoleLayer`.
    pub fn with_agent(mut self, agent_def: crate::agents::AgentDef) -> Self {
        self.agent_def = Some(agent_def);
        self
    }

    /// Attach a soul manifest for identity/personality injection.
    ///
    /// When set, `build_system_prompt` uses the Soul assembly path,
    /// injecting soul content at the top of the prompt.
    pub fn with_soul(mut self, soul: SoulManifest) -> Self {
        self.soul = Some(soul);
        self
    }

    /// Attach identity files loaded from the agent identity directory.
    ///
    /// Identity files (SOUL.md, IDENTITY.md, AGENTS.md, MEMORY.md, etc.)
    /// live under `~/.aleph/agents/{agent_id}/` — NOT under
    /// `~/.aleph/workspaces/{agent_id}/` (which is the runtime work directory).
    ///
    /// When set, `build_system_prompt` threads these files into the layer
    /// input so `SoulLayer`, `IdentityFilesLayer`, and related layers can
    /// render them into the system prompt.
    pub fn with_identity_files(mut self, files: IdentityFiles) -> Self {
        self.identity_files = Some(files);
        self
    }

    /// Attach a pre-rendered memory XML string for prompt injection.
    ///
    /// When set, `build_system_prompt` threads this into `LayerInput::memory_user_message`
    /// so `MemoryAugmentationLayer` injects it verbatim into the system prompt.
    pub fn with_memory_user_message(mut self, text: String) -> Self {
        self.memory_user_message = Some(text);
        self
    }

    /// Build the system prompt
    pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
        let (path, input) = match &self.soul {
            Some(soul) => (
                AssemblyPath::Soul,
                LayerInput::soul(&self.config, tools, soul),
            ),
            None => (AssemblyPath::Basic, LayerInput::basic(&self.config, tools)),
        };
        let input = input.with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.agent_def {
            Some(agent) => input.with_agent_def(agent),
            None => input,
        };
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = match &self.memory_user_message {
            Some(text) => input.with_memory_user_message(text.clone()),
            None => input,
        };
        self.pipeline.execute_cached(path, &input)
    }

    /// Build system prompt with hydrated tools from semantic retrieval
    ///
    /// This method builds a complete system prompt using HydrationResult
    /// instead of the traditional ToolInfo array, enabling semantic tool
    /// selection based on query relevance.
    pub fn build_system_prompt_with_hydration(&self, hydration: &HydrationResult) -> String {
        let input = LayerInput::hydration(&self.config, hydration);
        self.pipeline
            .execute_cached(AssemblyPath::Hydration, &input)
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
            .with_profile(profile)
            .with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        self.pipeline.execute_cached(AssemblyPath::Soul, &input)
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

        // Build with potentially modified config; carry over identity_files
        // so SoulLayer / IdentityFilesLayer see the agent identity directory.
        let mut builder = PromptBuilder::new(config);
        if let Some(files) = self.identity_files.clone() {
            builder = builder.with_identity_files(files);
        }
        let mut prompt = builder.build_system_prompt_with_soul(tools, soul, profile);

        // After hooks
        for hook in hooks {
            if let Err(e) = hook.after_prompt_build(&mut prompt) {
                tracing::warn!(hook = hook.name(), error = %e, "Prompt hook after_build failed");
            }
        }

        prompt
    }

    /// Build system prompt for a sub-agent.
    ///
    /// Injects the agent's role header and protocol sections via `AgentRoleLayer`.
    /// This replaces the old `PromptBuilder::for_agent()` from the pre-Harness era.
    pub fn build_for_agent(
        &self,
        agent_def: &crate::agents::AgentDef,
        tools: &[ToolInfo],
        soul: &SoulManifest,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_agent_def(agent_def)
            .with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        self.pipeline.execute_cached(AssemblyPath::Soul, &input)
    }

    /// Build system prompt for a sub-agent (basic path, no soul).
    ///
    /// Lighter variant for sub-agents that don't have a SoulManifest.
    pub fn build_for_agent_basic(
        &self,
        agent_def: &crate::agents::AgentDef,
        tools: &[ToolInfo],
    ) -> String {
        let input = LayerInput::basic(&self.config, tools)
            .with_agent_def(agent_def)
            .with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        self.pipeline.execute_cached(AssemblyPath::Basic, &input)
    }

    /// Capture the current stable layers output as a reusable snapshot.
    pub fn capture_snapshot(&self, tools: &[ToolInfo]) -> PromptSnapshot {
        let path = match &self.soul {
            Some(_) => AssemblyPath::Soul,
            None => AssemblyPath::Basic,
        };
        let input = match &self.soul {
            Some(soul) => LayerInput::soul(&self.config, tools, soul),
            None => LayerInput::basic(&self.config, tools),
        };
        let input = input.with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let stable_prefix = self.pipeline.execute_stable_only(path, &input);
        PromptSnapshot {
            stable_prefix,
            path,
        }
    }

    /// Build a sub-agent prompt by reusing the snapshot's stable prefix.
    pub fn build_from_snapshot(
        &self,
        snapshot: &PromptSnapshot,
        agent_def: &AgentDef,
        tools: &[ToolInfo],
    ) -> String {
        let input = match &self.soul {
            Some(soul) => LayerInput::soul(&self.config, tools, soul),
            None => LayerInput::basic(&self.config, tools),
        }
        .with_agent_def(agent_def)
        .with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };

        let dynamic_suffix = self.pipeline.execute_dynamic_only(snapshot.path, &input);
        let mut result = String::with_capacity(snapshot.stable_prefix.len() + dynamic_suffix.len());
        result.push_str(&snapshot.stable_prefix);
        result.push_str(&dynamic_suffix);
        result
    }

    /// Access the underlying config (for reading).
    pub fn config(&self) -> &PromptConfig {
        &self.config
    }

    /// Access the underlying config mutably (for dynamic modifications).
    pub fn config_mut(&mut self) -> &mut PromptConfig {
        &mut self.config
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
            .with_mode(mode)
            .with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        self.pipeline
            .execute_with_mode(AssemblyPath::Soul, &input, mode)
    }

    /// Build system prompt with mode and budget control.
    ///
    /// Combines soul-path assembly, mode filtering, and token budget
    /// enforcement into a single call.  Returns a [`PromptResult`] with
    /// truncation metadata when sections are removed to fit the budget.
    pub fn build_with_budget(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
        mode: PromptMode,
        budget: &TokenBudget,
    ) -> PromptResult {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_profile(profile)
            .with_mode(mode)
            .with_identity_files_opt(self.identity_files.as_ref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        self.pipeline
            .assemble(AssemblyPath::Soul, &input, mode, budget)
    }

    /// Build system prompt with full context (soul + profile + identity files + inbound).
    ///
    /// This is the comprehensive entry point that passes all available context
    /// through to the prompt pipeline via LayerInput.
    pub fn build_system_prompt_with_full_context(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
        identity_files: Option<&IdentityFiles>,
        inbound: Option<&InboundContext>,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_profile(profile)
            .with_identity_files_opt(identity_files)
            .with_inbound_opt(inbound);
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = match &self.memory_user_message {
            Some(text) => input.with_memory_user_message(text.clone()),
            None => input,
        };
        self.pipeline.execute_cached(AssemblyPath::Soul, &input)
    }

    /// Build system prompt using ResolvedContext
    ///
    /// This is the new entry point that uses the two-phase filtered context
    /// from the ContextAggregator. The pipeline layers handle all sections
    /// (runtime context, environment, security, protocol tokens, etc.)
    /// in priority order.
    pub fn build_system_prompt_with_context(
        &self,
        ctx: &super::context::ResolvedContext,
    ) -> String {
        let input = LayerInput::context(&self.config, ctx);
        self.pipeline.execute_cached(AssemblyPath::Context, &input)
    }
}
