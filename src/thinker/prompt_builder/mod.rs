//! Prompt builder for Agent Loop
//!
//! This module builds prompts for the LLM thinking step,
//! including system prompts and message history.

mod cache;
pub mod cache_monitor;
mod sections;

#[cfg(test)]
mod tests;

use crate::config::ProfileConfig;
use crate::tool_metadata::tool_index::HydrationResult;
use crate::tools::info::ToolInfo;

use crate::agents::AgentDef;

use super::identity_files::IdentityFiles;
use super::inbound_context::InboundContext;
use super::prompt_budget::{PromptResult, TokenBudget};
use super::prompt_layer::{AssemblyPath, LayerInput, LayerStability};
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
    /// Describes available runtimes (Python, Node.js, `FFmpeg`, etc.)
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
    /// Skill instructions injected from `SkillSystem` snapshot (XML format)
    /// When set, these are appended to the system prompt to inform the LLM
    /// about available skills from the `SkillSystem` v2
    pub skill_instructions: Option<String>,
    /// Token budget for system prompt assembly.
    pub token_budget: TokenBudget,
    /// Whether native `tool_use` is enabled (skip `ToolsLayer` and `ResponseFormatLayer`)
    ///
    /// When true, tool definitions are passed via the API's native `tool_use` mechanism
    /// rather than injected into the system prompt. This also means the LLM will
    /// respond with structured tool calls rather than JSON-in-text.
    pub native_tools_enabled: bool,
    /// Eligible skills from `SkillSystem` v2 snapshot for scope-aware filtering.
    pub eligible_skills: Option<Vec<crate::domain::skill::SkillManifest>>,
    /// Budget bounding the injected `<available_skills>` index, sourced from
    /// `SkillsConfig.prompt_budget` via the snapshot. `None` → the layer falls
    /// back to the built-in default budget.
    pub skill_prompt_budget: Option<crate::skill::prompt::SkillPromptBudget>,
    /// Available agent catalog entries for `AgentCatalogLayer`.
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
            skill_prompt_budget: None,
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
    /// Loaded identity files (SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md,
    /// HEARTBEAT.md) from `~/.aleph/agents/{agent_id}/`. When set, these are
    /// threaded into every `LayerInput` so `SoulLayer`, `IdentityFilesLayer`,
    /// `ProfileLayer`, and `CustomInstructionsLayer` can read their respective
    /// files. MEMORY.md is **not** included here — it's owned by the curated
    /// memory module and threaded in via `curated_memory_envelope`.
    identity_files: Option<IdentityFiles>,
    /// Pre-rendered memory XML from `MemoryContextProvider::build_memory_user_message`.
    ///
    /// When set, threaded into every `LayerInput` as `memory_user_message` so
    /// `MemoryAugmentationLayer` can inject it verbatim into the system prompt.
    memory_user_message: Option<String>,
    /// Pre-rendered curated memory envelope (`<CuratedMemory>` + `<UserProfile>`).
    ///
    /// When set, threaded into every `LayerInput` as `curated_memory_envelope`
    /// so `CuratedMemoryLayer` (Stable) can inject it verbatim, preserving the
    /// prompt prefix cache.
    curated_memory_envelope: Option<String>,
    /// Subagent call-chain context. Threaded into every `LayerInput` as
    /// `chain_context` so `ChainContextLayer` can render the delegation
    /// depth + chain id for non-root agents. Root chains and `None` both
    /// produce empty output — the orchestrator path can leave this
    /// unset, while `subagent_spawner` wires the descended child chain in.
    chain_context: Option<crate::harness::chain_context::ChainContext>,
    /// Resolved context (interaction paradigm + security envelope + runtime
    /// context) for the Phase 2 widened layers — `SecurityLayer`,
    /// `OperationalGuidelinesLayer`, `ProtocolTokensLayer`,
    /// `RuntimeContextLayer`. Threaded into every `LayerInput` so those
    /// layers can render content on the Basic / Hydration / Soul paths
    /// without forcing a switch to the dedicated `Context` route.
    /// `build_system_prompt_with_context()` ignores this field — its
    /// `LayerInput::context` constructor already supplies a context.
    resolved_context: Option<super::context::ResolvedContext>,
    /// Wire-protocol family identifier (e.g. "anthropic", "openai",
    /// "gemini", "ollama"). Threaded into every `LayerInput` as
    /// `provider_protocol` so `ProviderGuidanceLayer` can pick the
    /// right per-family operational directives. Sourced by the harness
    /// bridge from `AiProvider::model_behavior_override()` falling back
    /// to `AiProvider::protocol()`.
    provider_protocol: Option<String>,
    /// Static per-run Think→Act iteration cap. Threaded into every
    /// `LayerInput` as `iteration_cap` so `SessionBudgetLayer` can
    /// surface it. Sourced by the harness bridge from
    /// `resolve_max_iterations(...)` (`FlowOverrides.max_iterations`
    /// override winning over the boot-time default).
    iteration_cap: Option<u32>,
    /// User-configured `[prompt.extra_files]` content, loaded size-capped
    /// by the harness bridge. Threaded into every `LayerInput` so
    /// `ExtraFilesLayer` can render each file as a named section.
    extra_files: Option<Vec<crate::thinker::prompt_layer::ExtraPromptFile>>,
    /// Welded strategy `<strategy>` body for the subagent inline prompt seam.
    /// The subagent prompt builds on the `Basic` path with no `ResolvedContext`,
    /// so `StrategyLayer` (which reads `ResolvedContext.strategy`) never fires
    /// here. When set, `build_system_prompt` appends the wrapped block
    /// post-pipeline, byte-for-byte matching the `StrategyLayer` wrap. `None`
    /// leaves the prompt byte-identical to the pre-strategy build.
    strategy: Option<String>,
}

impl PromptBuilder {
    /// Create a new prompt builder
    #[must_use]
    pub fn new(config: PromptConfig) -> Self {
        let pipeline = PromptPipeline::default_layers();
        Self {
            config,
            pipeline,
            agent_def: None,
            soul: None,
            identity_files: None,
            memory_user_message: None,
            curated_memory_envelope: None,
            chain_context: None,
            resolved_context: None,
            provider_protocol: None,
            iteration_cap: None,
            extra_files: None,
            strategy: None,
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
    /// Identity files (SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md,
    /// HEARTBEAT.md) live under `~/.aleph/agents/{agent_id}/` — NOT under
    /// `~/.aleph/workspaces/{agent_id}/` (which is the runtime work directory).
    /// MEMORY.md lives alongside but is loaded by the curated memory module.
    ///
    /// When set, `build_system_prompt` threads these files into the layer
    /// input so `SoulLayer`, `IdentityFilesLayer`, and related layers can
    /// render them into the system prompt.
    pub fn with_identity_files(mut self, files: IdentityFiles) -> Self {
        self.identity_files = Some(files);
        self
    }

    /// Attach user-configured `[prompt.extra_files]` content.
    ///
    /// The harness bridge loads the configured paths (size-capped) and
    /// threads them here so `ExtraFilesLayer` renders each file as a
    /// named section in the system prompt.
    pub fn with_extra_files(
        mut self,
        files: Vec<crate::thinker::prompt_layer::ExtraPromptFile>,
    ) -> Self {
        self.extra_files = Some(files);
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

    /// Attach a pre-rendered curated memory envelope for prompt injection.
    ///
    /// When set, `build_system_prompt` threads this into
    /// `LayerInput::curated_memory_envelope` so `CuratedMemoryLayer` (Stable)
    /// injects it verbatim. Pass `None` to clear a previously attached envelope.
    pub fn with_curated_envelope(mut self, envelope: Option<String>) -> Self {
        self.curated_memory_envelope = envelope;
        self
    }

    /// Attach a subagent call-chain context for `ChainContextLayer`. Root
    /// chains (`depth == 0`) and `None` both leave the delegation section
    /// out of the assembled prompt, so the orchestrator path can safely
    /// leave this unset.
    pub fn with_chain_context(
        mut self,
        chain: crate::harness::chain_context::ChainContext,
    ) -> Self {
        self.chain_context = Some(chain);
        self
    }

    /// Attach a `ResolvedContext` so the Phase 2 widened layers
    /// (`SecurityLayer`, `OperationalGuidelinesLayer`,
    /// `ProtocolTokensLayer`, `RuntimeContextLayer`) can emit content on
    /// the Basic / Hydration / Soul paths. The harness bridge builds a
    /// default `ResolvedContext` from `Background` paradigm + permissive
    /// security; channel-specific paths can override with a stricter one.
    ///
    /// `build_system_prompt_with_context()` ignores this builder field —
    /// it already receives a `ResolvedContext` parameter that the
    /// `LayerInput::context` constructor wires directly.
    pub fn with_resolved_context(mut self, ctx: super::context::ResolvedContext) -> Self {
        self.resolved_context = Some(ctx);
        self
    }

    /// Attach the wire-protocol family identifier reported by the
    /// active provider. `ProviderGuidanceLayer` uses this to dispatch
    /// the right per-family operational guidance block.
    ///
    /// Pass the protocol exactly as `AiProvider::protocol()` returns it
    /// — typically `"anthropic"`, `"openai"`, `"gemini"`, or `"ollama"`.
    /// Providers may override via `model_behavior_override()` (e.g.
    /// `OpenRouter` routing GPT-4 over `OpenAI` protocol); the harness
    /// bridge prefers the override.
    pub fn with_provider_protocol(mut self, protocol: impl Into<String>) -> Self {
        self.provider_protocol = Some(protocol.into());
        self
    }

    /// Attach the resolved Think→Act iteration cap so
    /// `SessionBudgetLayer` can surface it. The harness bridge sources
    /// the value from `resolve_max_iterations(...)` once per run.
    pub const fn with_iteration_cap(mut self, cap: u32) -> Self {
        self.iteration_cap = Some(cap);
        self
    }

    /// Attach a welded strategy `<strategy>` body for the subagent inline
    /// prompt. The body is the inner text (no tags); `build_system_prompt`
    /// wraps it in `<strategy> … </strategy>` exactly like `StrategyLayer`.
    /// Threaded in by `subagent_spawner::spawn` from `SpawnRequest.strategy`.
    #[must_use]
    pub fn with_strategy(mut self, strategy: String) -> Self {
        self.strategy = Some(strategy);
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
        let input = input
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
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
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
        maybe_trace_prompt_size(&self.pipeline, path, &input);
        let mut prompt = self.pipeline.execute(path, &input);
        // Subagent strategy weld: appended post-pipeline because the Basic-path
        // inline prompt threads no `ResolvedContext` for `StrategyLayer` to read.
        // Wrap mirrors `StrategyLayer` byte-for-byte: `<strategy>\n{body}\n</strategy>\n\n`.
        if let Some(body) = self.strategy.as_deref() {
            prompt.push_str("<strategy>\n");
            prompt.push_str(body);
            prompt.push_str("\n</strategy>\n\n");
        }
        prompt
    }

    /// Build system prompt with hydrated tools from semantic retrieval
    ///
    /// This method builds a complete system prompt using `HydrationResult`
    /// instead of the traditional `ToolInfo` array, enabling semantic tool
    /// selection based on query relevance.
    pub fn build_system_prompt_with_hydration(&self, hydration: &HydrationResult) -> String {
        let input = LayerInput::hydration(&self.config, hydration)
            .with_curated_envelope(self.curated_memory_envelope.clone())
            .with_chain_context_opt(self.chain_context.as_ref())
            .with_resolved_context_opt(self.resolved_context.as_ref())
            .with_provider_protocol_opt(self.provider_protocol.as_deref())
            .with_iteration_cap_opt(self.iteration_cap);
        self.pipeline
            .execute(AssemblyPath::Hydration, &input)
    }

    /// Build system prompt with soul section at the top
    ///
    /// This is the primary entry point when using the Embodiment Engine.
    /// Soul content appears at the very top of the prompt for highest priority.
    /// When a workspace profile is provided, its `system_prompt` is injected
    /// between Soul (priority 50) and Role (priority 100).
    pub fn build_system_prompt_with_soul(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_profile(profile)
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
        self.pipeline.execute(AssemblyPath::Soul, &input)
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
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }

    /// Build system prompt for a sub-agent (basic path, no soul).
    ///
    /// Lighter variant for sub-agents that don't have a `SoulManifest`.
    pub fn build_for_agent_basic(
        &self,
        agent_def: &crate::agents::AgentDef,
        tools: &[ToolInfo],
    ) -> String {
        let input = LayerInput::basic(&self.config, tools)
            .with_agent_def(agent_def)
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
        self.pipeline.execute(AssemblyPath::Basic, &input)
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
        let input = input
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
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
        .with_identity_files_opt(self.identity_files.as_ref())
        .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);

        let dynamic_suffix = self.pipeline.execute_dynamic_only(snapshot.path, &input);
        let mut result = String::with_capacity(snapshot.stable_prefix.len() + dynamic_suffix.len());
        result.push_str(&snapshot.stable_prefix);
        result.push_str(&dynamic_suffix);
        result
    }

    /// Access the underlying config (for reading).
    pub const fn config(&self) -> &PromptConfig {
        &self.config
    }

    /// Access the underlying config mutably (for dynamic modifications).
    pub const fn config_mut(&mut self) -> &mut PromptConfig {
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
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
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
            .with_identity_files_opt(self.identity_files.as_ref())
            .with_extra_files_opt(self.extra_files.as_deref());
        let input = match &self.config.mcp_instructions {
            Some(instructions) => input.with_mcp_instructions(instructions),
            None => input,
        };
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
        self.pipeline
            .assemble(AssemblyPath::Soul, &input, mode, budget)
    }

    /// Build system prompt with full context (soul + profile + identity files + inbound).
    ///
    /// This is the comprehensive entry point that passes all available context
    /// through to the prompt pipeline via `LayerInput`.
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
        let input = input.with_curated_envelope(self.curated_memory_envelope.clone());
        let input = input.with_chain_context_opt(self.chain_context.as_ref());
        let input = input.with_resolved_context_opt(self.resolved_context.as_ref());
        let input = input.with_provider_protocol_opt(self.provider_protocol.as_deref());
        let input = input.with_iteration_cap_opt(self.iteration_cap);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }

    /// Build system prompt using `ResolvedContext`
    ///
    /// This is the new entry point that uses the two-phase filtered context
    /// from the `ContextAggregator`. The pipeline layers handle all sections
    /// (runtime context, environment, security, protocol tokens, etc.)
    /// in priority order.
    pub fn build_system_prompt_with_context(
        &self,
        ctx: &super::context::ResolvedContext,
    ) -> String {
        let input = LayerInput::context(&self.config, ctx)
            .with_curated_envelope(self.curated_memory_envelope.clone())
            .with_chain_context_opt(self.chain_context.as_ref())
            .with_provider_protocol_opt(self.provider_protocol.as_deref())
            .with_iteration_cap_opt(self.iteration_cap);
        self.pipeline.execute(AssemblyPath::Context, &input)
    }
}

/// Optional prompt-size introspection (hermes `prompt_size.py` analogue).
///
/// When the `ALEPH_PROMPT_SIZE_TRACE` env var is set, logs each active layer's
/// char/token contribution for the prompt about to be built (largest first) so
/// prompt bloat is visible while tuning. Off by default = zero overhead on the
/// hot path; reuses [`PromptPipeline::layer_breakdown`] so no live state is
/// duplicated.
fn maybe_trace_prompt_size(pipeline: &PromptPipeline, path: AssemblyPath, input: &LayerInput) {
    if std::env::var_os("ALEPH_PROMPT_SIZE_TRACE").is_none() {
        return;
    }
    let mut bd = pipeline.layer_breakdown(path, input, PromptMode::Full);
    bd.sort_by_key(|l| std::cmp::Reverse(l.chars));
    let total_chars: usize = bd.iter().map(|l| l.chars).sum();
    let total_tokens: usize = bd.iter().map(|l| l.tokens).sum();
    let rows: String = bd
        .iter()
        .map(|l| {
            let zone = match l.stability {
                LayerStability::Stable => "stable",
                LayerStability::Dynamic => "dynamic",
            };
            format!(
                "\n  {:>6}c {:>6}t  {:<7}  p{:<4} {}",
                l.chars, l.tokens, zone, l.priority, l.name
            )
        })
        .collect();
    tracing::info!(
        "prompt-size breakdown [{:?}] — {} layers, {} chars (~{} tokens):{}",
        path,
        bd.len(),
        total_chars,
        total_tokens,
        rows
    );
}
