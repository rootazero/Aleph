//! Prompt builder for Agent Loop
//!
//! This module builds prompts for the LLM thinking step,
//! including system prompts and message history.

mod cache;
pub mod cache_monitor;

#[cfg(test)]
mod tests;

use crate::tools::info::ToolInfo;

use super::identity_files::IdentityFiles;
use super::prompt_budget::TokenBudget;
use super::prompt_layer::{AssemblyPath, LayerInput, LayerStability};
use super::prompt_mode::PromptMode;
use super::prompt_pipeline::PromptPipeline;

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
///
/// Every field here MUST have a production writer. Fields whose only writer was
/// a unit test have repeatedly kept whole layers alive that could never speak in
/// production (`persona`, `custom_instructions`, `generation_models`,
/// `tool_index`, `thinking_transparency`, `skill_instructions`,
/// `max_tool_description_tokens` — all removed 2026-07-26). The two production
/// writers are `harness_bridge::prompt_build` and `subagent_spawner`; the
/// `reachable_layers` test in `prompt_pipeline.rs` is the structural guard.
#[derive(Debug, Clone, Default)]
pub struct PromptConfig {
    /// Response language, sourced from `[general] language`.
    ///
    /// Session-stable, so `LanguageLayer` (@1600, Stable) rides the cacheable
    /// prefix without perturbing it.
    pub language: Option<String>,
    /// Runtime capabilities (pre-formatted prompt text)
    /// Describes available runtimes (Python, Node.js, `FFmpeg`, etc.)
    pub runtime_capabilities: Option<String>,
    /// Token budget for system prompt assembly.
    pub token_budget: TokenBudget,
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
    /// Active tool names, used by `SkillInstructionsLayer` to filter
    /// `PromptScope::Tool` skills on the production **cached** path.
    ///
    /// On that path `LayerInput::tools` is empty (native `tool_use` delivers
    /// schemas out-of-band, so the prompt never lists them), which meant every
    /// Tool-scoped skill was silently dropped. The harness bridge populates
    /// this from the tool catalog — but only when a Tool-scoped skill is
    /// actually eligible, so the common path pays nothing. The set is
    /// session-stable, so it does not perturb the cacheable stable prefix.
    pub active_tool_names: Vec<String>,
}

/// Prompt builder for Agent Loop thinking
pub struct PromptBuilder {
    config: PromptConfig,
    pipeline: PromptPipeline,
    /// Optional agent definition for sub-agent prompt assembly.
    /// When set, `build_system_prompt` will inject agent role via `AgentRoleLayer`.
    agent_def: Option<crate::agents::AgentDef>,
    /// Loaded identity files (SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md,
    /// HEARTBEAT.md) from `~/.aleph/agents/{agent_id}/`. When set, these are
    /// threaded into every `LayerInput` so `SoulLayer`, `IdentityFilesLayer`,
    /// `ProfileLayer`, and `CustomInstructionsLayer` can read their respective
    /// files. MEMORY.md is **not** included here — it's owned by the curated
    /// memory module and threaded in via `curated_memory_envelope`.
    identity_files: Option<IdentityFiles>,
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
    /// layers can render content on the Basic / Cached assembly paths.
    resolved_context: Option<super::context::ResolvedContext>,
    behavior_name: Option<String>,
    model_behavior_delta: Option<String>,
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
    /// Usage-mode line for the subagent inline prompt seam. Same rationale as
    /// `strategy` above: the Basic path threads no `ResolvedContext`, so
    /// `SecurityLayer` (which renders the mode line on the gateway path)
    /// never fires here. When set, `build_system_prompt` appends
    /// `SessionMode::subagent_prompt_line` post-pipeline. `None` (and the
    /// caller-side Work skip) leaves the prompt byte-identical.
    session_mode: Option<crate::config::types::policies::SessionMode>,
    /// True when this run's history carries `<session_context>` compaction
    /// summaries. Threaded into every `LayerInput` so `SessionContextGuideLayer`
    /// injects its usage guide only when summaries are present. Sourced by the
    /// harness bridge from `FlowInput::carries_session_summaries()`.
    has_session_summaries: bool,
    /// True when this turn's transient tail carries an auto-recalled
    /// `<memory-context>` message. Threaded into every `LayerInput` so
    /// `MemoryProtocolLayer` claims the block is in the conversation only when
    /// it really is. Sourced by the harness bridge from
    /// `MemoryContextProvider::build_memory_user_message`'s own result.
    has_recalled_memory: bool,
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
            identity_files: None,
            curated_memory_envelope: None,
            chain_context: None,
            resolved_context: None,
            behavior_name: None,
            model_behavior_delta: None,
            iteration_cap: None,
            extra_files: None,
            strategy: None,
            session_mode: None,
            has_session_summaries: false,
            has_recalled_memory: false,
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
    /// the Basic / Cached assembly paths. The harness bridge builds a
    /// default `ResolvedContext` from `Background` paradigm + permissive
    /// security; channel-specific paths can override with a stricter one.
    pub fn with_resolved_context(mut self, ctx: super::context::ResolvedContext) -> Self {
        self.resolved_context = Some(ctx);
        self
    }

    pub fn with_behavior_name(mut self, name: impl Into<String>) -> Self {
        self.behavior_name = Some(name.into());
        self
    }

    pub fn with_model_behavior_delta(mut self, delta: Option<String>) -> Self {
        self.model_behavior_delta = delta;
        self
    }

    /// Attach the resolved Think→Act iteration cap so
    /// `SessionBudgetLayer` can surface it. The harness bridge sources
    /// the value from `resolve_max_iterations(...)` once per run.
    pub const fn with_iteration_cap(mut self, cap: u32) -> Self {
        self.iteration_cap = Some(cap);
        self
    }

    /// Mark whether this run's history carries `<session_context>` compaction
    /// summaries, so `SessionContextGuideLayer` surfaces its guide. The harness
    /// bridge sources the value from `FlowInput::carries_session_summaries()`.
    #[must_use]
    pub const fn with_session_summaries(mut self, has: bool) -> Self {
        self.has_session_summaries = has;
        self
    }

    /// Mark whether this turn's transient tail carries an auto-recalled
    /// `<memory-context>` message, so `MemoryProtocolLayer` may state it as
    /// fact. The harness bridge sources the value from the producer itself —
    /// `build_memory_user_message` returns `Ok(None)` under
    /// `MemoryInjectionMode::Tools`, which is exactly the configuration the
    /// unconditional claim used to lie about.
    #[must_use]
    pub const fn with_recalled_memory(mut self, has: bool) -> Self {
        self.has_recalled_memory = has;
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

    /// Attach the parent run's usage mode (chat / work / code) so the child
    /// prompt names the partition its inherited tool surface was built with.
    /// Threaded in by `subagent_spawner::spawn` from `SpawnRequest.session_mode`.
    #[must_use]
    pub const fn with_session_mode(
        mut self,
        mode: crate::config::types::policies::SessionMode,
    ) -> Self {
        self.session_mode = Some(mode);
        self
    }

    /// The [`LayerInput`] for the `Basic` assembly path, shared by both Basic
    /// entry points ([`Self::build_system_prompt`] and
    /// [`Self::build_system_prompt_parts`]) so they cannot disagree about which
    /// builder fields reach the layers. The `Cached` twin is
    /// [`Self::build_cached_input`].
    pub(super) fn build_basic_input<'a>(&'a self, tools: &'a [ToolInfo]) -> LayerInput<'a> {
        let input = LayerInput::basic(&self.config, tools)
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
        input
            .with_curated_envelope(self.curated_memory_envelope.clone())
            .with_chain_context_opt(self.chain_context.as_ref())
            .with_resolved_context_opt(self.resolved_context.as_ref())
            .with_behavior_name_opt(self.behavior_name.as_deref())
            .with_model_behavior_delta_opt(self.model_behavior_delta.as_deref())
            .with_iteration_cap_opt(self.iteration_cap)
            .with_session_summaries(self.has_session_summaries)
            .with_recalled_memory(self.has_recalled_memory)
    }

    /// The two post-pipeline welds the Basic path carries, appended to `out`.
    ///
    /// Both exist because a sub-agent prompt historically threaded no
    /// `ResolvedContext`, so `StrategyLayer` and the session-mode line had no
    /// input to read. A caller that DOES supply a resolved context must leave
    /// `ResolvedContext::strategy` unset (the spawner does) or the strategy
    /// body would be stated twice — the layer once and this weld again.
    pub(super) fn append_basic_welds(&self, out: &mut String) {
        // Wrap mirrors `StrategyLayer` byte-for-byte:
        // `<strategy>\n{body}\n</strategy>\n\n`.
        if let Some(body) = self.strategy.as_deref() {
            out.push_str("<strategy>\n");
            out.push_str(body);
            out.push_str("\n</strategy>\n\n");
        }
        if let Some(mode) = self.session_mode {
            out.push_str(mode.subagent_prompt_line());
            out.push_str("\n\n");
        }
    }

    /// Build the system prompt as one undivided string.
    ///
    /// Carries **no** prompt-cache breakpoint, so the whole prompt is subject
    /// to the token budget — there is no protected floor to keep intact. Use
    /// [`Self::build_system_prompt_parts`] for anything that reaches a
    /// provider: an unsplit prompt is cached (or not) as a single block, which
    /// means one per-run byte anywhere in it re-keys all of it.
    pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
        let path = AssemblyPath::Basic;
        let input = self.build_basic_input(tools);
        maybe_trace_prompt_size(&self.pipeline, path, &input, PromptMode::Full);
        let mut prompt = self.pipeline.execute(path, &input);
        self.append_basic_welds(&mut prompt);
        crate::thinker::prompt_budget::fit_dynamic_suffix_with_content(
            "",
            prompt,
            &self.config.token_budget,
        )
    }

    /// Access the underlying config (for reading).
    pub const fn config(&self) -> &PromptConfig {
        &self.config
    }
}

/// Optional prompt-size introspection (hermes `prompt_size.py` analogue).
///
/// When the `ALEPH_PROMPT_SIZE_TRACE` env var is set, logs each active layer's
/// char/token contribution for the prompt about to be built (largest first) so
/// prompt bloat is visible while tuning. Off by default = zero overhead on the
/// hot path; reuses [`PromptPipeline::layer_breakdown`] so no live state is
/// duplicated.
///
/// Called from **both** entry points — `build_system_prompt` (Basic, sub-agent)
/// and `build_system_prompt_cached_with_mode` (Cached, main loop). It used to
/// hang off the Basic path only, which meant the trace never fired for the
/// prompt that actually matters. `mode` must be the mode the caller is about to
/// assemble with, or the breakdown reports layers the prompt won't contain.
fn maybe_trace_prompt_size(
    pipeline: &PromptPipeline,
    path: AssemblyPath,
    input: &LayerInput,
    mode: PromptMode,
) {
    if std::env::var_os("ALEPH_PROMPT_SIZE_TRACE").is_none() {
        return;
    }
    let mut bd = pipeline.layer_breakdown(path, input, mode);
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
