//! `PromptLayer` trait — the composable unit of prompt assembly

use super::context::ResolvedContext;
use super::identity_files::IdentityFiles;
use super::prompt_builder::PromptConfig;
use super::prompt_mode::PromptMode;
use crate::agents::AgentDef;
use crate::tools::info::ToolInfo;

/// MCP server instruction metadata for prompt injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerInstruction {
    pub server_name: String,
    pub instructions: String,
}

/// Lightweight agent catalog entry for prompt injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCatalogEntry {
    pub id: String,
    pub description: String,
    pub when_to_use: Option<String>,
}

/// Whether a layer's content is stable across requests or changes per request.
///
/// Used by the prompt cache optimisation to partition the system prompt
/// into a stable prefix (cacheable) and a dynamic suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerStability {
    /// Content rarely changes between requests (persona, tools, skills).
    Stable,
    /// Content changes per request (time, session context, memory).
    Dynamic,
}

/// Which assembly path a layer participates in.
///
/// The pipeline filters layers by the active path so that only relevant
/// sections are injected into the final system prompt.
///
/// **There are exactly as many variants as there are entry points.** Phantom
/// paths are how layers go silently missing: a layer that lists only a path no
/// caller ever requests renders nowhere, and the omission is invisible. Three
/// phantoms have been removed for exactly that reason — `Context`
/// (2026-07-20), then `Hydration` and `Soul` (2026-07-26), each surviving only
/// in the `prompt-size` debug flag long after its last real caller went away.
/// If you add a variant, add the entry point that requests it in the same
/// commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssemblyPath {
    /// Inline sub-agent prompt, one flat string, no cache split.
    /// Entry point: `PromptBuilder::build_system_prompt` (`subagent_spawner`).
    Basic,
    /// Main-loop prompt, split into a cacheable stable prefix + dynamic suffix.
    /// Entry point: `PromptBuilder::build_system_prompt_cached_with_mode`
    /// (`harness_bridge::prompt_build`).
    Cached,
}

/// A user-configured extra file loaded for system-prompt injection.
///
/// Sourced from `[prompt.extra_files]` in config (loaded size-capped by the
/// harness bridge) and rendered by `ExtraFilesLayer` @1735. Content crosses
/// the same user-editable-file trust boundary as identity files and is
/// sanitized by the layer before injection.
#[derive(Debug, Clone)]
pub struct ExtraPromptFile {
    /// Display name (the configured path string).
    pub name: String,
    /// File content, already truncated to the loader's per-file cap.
    pub content: String,
}

/// Everything a layer might need to produce its output.
///
/// Each constructor pre-fills only the fields relevant to a given
/// assembly path; the rest stay `None`.
pub struct LayerInput<'a> {
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub context: Option<&'a ResolvedContext>,
    /// Prompt mode for this assembly (default: Full)
    pub mode: PromptMode,
    /// Loaded agent identity files (SOUL.md, IDENTITY.md, AGENTS.md,
    /// TOOLS.md, HEARTBEAT.md) from `~/.aleph/agents/{agent_id}/`.
    /// MEMORY.md is owned by the curated memory module and flows in via
    /// `curated_memory_envelope`, not this list.
    pub identity_files: Option<&'a IdentityFiles>,
    /// Whether the conversation history contains compressed session summaries.
    ///
    /// Set to `true` when session compaction has produced at least one
    /// `<session_context>` block in the message history.  The
    /// `SessionContextGuideLayer` uses this flag to inject usage guidance.
    pub has_session_summaries: bool,
    /// Whether this turn actually carries an auto-recalled `<memory-context>`
    /// message.
    ///
    /// The recall message is delivered as a transient tail message, never
    /// through the builder, so no layer can observe it directly. The harness
    /// bridge sets this from the single producer that decides it —
    /// `MemoryContextProvider::build_memory_user_message`, which returns
    /// `Ok(None)` under `MemoryInjectionMode::Tools` and for an empty query.
    /// `MemoryProtocolLayer` gates its "already in this conversation" claim on
    /// it: asserting a block the turn does not carry costs the model a wasted
    /// turn.
    pub has_recalled_memory: bool,
    /// Agent definition for sub-agent prompt assembly.
    ///
    /// When set, `AgentRoleLayer` injects the role header and protocol
    /// sections declared in `agent_def.prompt_sections`.
    pub agent_def: Option<&'a AgentDef>,
    /// MCP server instructions for prompt injection.
    ///
    /// When set, `McpInstructionsLayer` injects per-server instruction
    /// blocks into the system prompt.
    pub mcp_instructions: Option<&'a [McpServerInstruction]>,
    /// Pre-rendered curated memory envelope (`<CuratedMemory>` + `<UserProfile>`).
    ///
    /// Populated once per session by `MemoryContextProvider::build_curated_message`
    /// and threaded through every `LayerInput` construction.  `CuratedMemoryLayer`
    /// (priority 60, Stable) injects this text verbatim, preserving the prompt
    /// prefix cache.
    pub curated_memory_envelope: Option<String>,
    /// Subagent call-chain position for `ChainContextLayer`.
    ///
    /// Threaded through from `HarnessDeps.chain_context` (the value the
    /// orchestrator and `subagent_spawner` already maintain). Layer omits
    /// the section entirely when the chain is at root (`depth == 0`) or
    /// the field is `None`, so the top-level harness path stays unaffected.
    pub chain_context: Option<&'a crate::harness::chain_context::ChainContext>,
    /// Resolved governance behavior name (`resolve_behavior`: anthropic /
    /// openai / gemini / ollama / strict / unknown). Drives
    /// `ProviderGuidanceLayer`'s baseline-block selection. `None` keeps the
    /// layer silent (capture / snapshot / tests).
    pub behavior_name: Option<&'a str>,
    /// Pre-loaded per-family coaching delta from `model_behaviors/{name}.md`
    /// (overridable at `~/.aleph/model_behaviors/`). Appended verbatim by
    /// `ProviderGuidanceLayer` after the shared baseline blocks. `None` = no
    /// delta for this family.
    pub model_behavior_delta: Option<&'a str>,
    /// Static per-run Think→Act iteration cap, resolved by the harness
    /// bridge from `FlowOverrides.max_iterations` falling back to the
    /// boot-time `[execution] max_iterations` default. Used by
    /// `SessionBudgetLayer` to surface the cap to the LLM as a planning
    /// hint. `None` or `Some(0)` keeps the layer silent.
    pub iteration_cap: Option<u32>,
    /// User-configured extra files from `[prompt.extra_files]`, loaded
    /// (size-capped) by the harness bridge. `ExtraFilesLayer` renders each
    /// as a named section; `None` / empty keeps the layer silent.
    pub extra_files: Option<&'a [ExtraPromptFile]>,
}

impl<'a> LayerInput<'a> {
    /// Input for the `Basic` path — config + tool list.
    #[must_use]
    pub const fn basic(config: &'a PromptConfig, tools: &'a [ToolInfo]) -> Self {
        Self {
            config,
            tools: Some(tools),
            context: None,
            mode: PromptMode::Full,
            identity_files: None,
            has_session_summaries: false,
            has_recalled_memory: false,
            agent_def: None,
            mcp_instructions: None,
            curated_memory_envelope: None,
            chain_context: None,
            behavior_name: None,
            model_behavior_delta: None,
            iteration_cap: None,
            extra_files: None,
        }
    }

    /// Set the prompt mode for this assembly.
    #[must_use]
    pub const fn with_mode(mut self, mode: PromptMode) -> Self {
        self.mode = mode;
        self
    }

    /// Attach agent identity files to this input.
    #[must_use]
    pub const fn with_identity_files(mut self, files: &'a IdentityFiles) -> Self {
        self.identity_files = Some(files);
        self
    }

    /// Attach optional agent identity files to this input.
    #[must_use]
    pub const fn with_identity_files_opt(mut self, files: Option<&'a IdentityFiles>) -> Self {
        self.identity_files = files;
        self
    }

    /// Attach optional `[prompt.extra_files]` content to this input.
    #[must_use]
    pub const fn with_extra_files_opt(mut self, files: Option<&'a [ExtraPromptFile]>) -> Self {
        self.extra_files = files;
        self
    }

    /// Attach the pre-rendered curated memory envelope.
    ///
    /// When set, `CuratedMemoryLayer` injects this text verbatim into the
    /// system prompt as a Stable section so the prefix cache stays warm.
    #[must_use]
    pub fn with_curated_envelope(mut self, envelope: Option<String>) -> Self {
        self.curated_memory_envelope = envelope;
        self
    }

    /// Signal that the conversation contains compressed session summaries.
    #[must_use]
    pub const fn with_session_summaries(mut self, has: bool) -> Self {
        self.has_session_summaries = has;
        self
    }

    /// Signal that this turn carries an auto-recalled `<memory-context>`
    /// message. Pass the producer's own answer (`memory_text.is_some()`), not
    /// a proxy for it — `MemoryProtocolLayer` states it to the model as fact.
    #[must_use]
    pub const fn with_recalled_memory(mut self, has: bool) -> Self {
        self.has_recalled_memory = has;
        self
    }

    /// Attach an agent definition for sub-agent prompt assembly.
    #[must_use]
    pub const fn with_agent_def(mut self, agent_def: &'a AgentDef) -> Self {
        self.agent_def = Some(agent_def);
        self
    }

    /// Attach MCP server instructions for prompt injection.
    #[must_use]
    pub const fn with_mcp_instructions(mut self, instructions: &'a [McpServerInstruction]) -> Self {
        self.mcp_instructions = Some(instructions);
        self
    }

    /// Attach a subagent call-chain context. Used by `ChainContextLayer`
    /// to surface depth / `chain_id` to the LLM. Root chains (depth=0) and
    /// `None` both render an empty section.
    #[must_use]
    pub const fn with_chain_context(
        mut self,
        chain: &'a crate::harness::chain_context::ChainContext,
    ) -> Self {
        self.chain_context = Some(chain);
        self
    }

    /// Same as `with_chain_context` but accepts an `Option` for ergonomic
    /// threading from optional config.
    #[must_use]
    pub const fn with_chain_context_opt(
        mut self,
        chain: Option<&'a crate::harness::chain_context::ChainContext>,
    ) -> Self {
        self.chain_context = chain;
        self
    }

    /// Attach an optional `ResolvedContext` to this input.
    ///
    /// Threaded by every production entry point (Basic / Cached) so the
    /// Phase 2 widened layers (security, `operational_guidelines`,
    /// `protocol_tokens`, `runtime_context`) can consume context on any
    /// assembly path. `None` leaves those layers silent.
    #[must_use]
    pub const fn with_resolved_context_opt(mut self, ctx: Option<&'a ResolvedContext>) -> Self {
        self.context = ctx;
        self
    }

    #[must_use]
    pub const fn with_behavior_name(mut self, name: &'a str) -> Self {
        self.behavior_name = Some(name);
        self
    }

    #[must_use]
    pub const fn with_behavior_name_opt(mut self, name: Option<&'a str>) -> Self {
        self.behavior_name = name;
        self
    }

    #[must_use]
    pub const fn with_model_behavior_delta_opt(mut self, delta: Option<&'a str>) -> Self {
        self.model_behavior_delta = delta;
        self
    }

    /// Attach the per-run iteration cap so `SessionBudgetLayer` can
    /// surface it. Pass the resolved (post-`resolve_max_iterations`)
    /// value, not the raw override.
    #[must_use]
    pub const fn with_iteration_cap(mut self, cap: u32) -> Self {
        self.iteration_cap = Some(cap);
        self
    }

    /// Same as `with_iteration_cap` but accepts an `Option` for
    /// ergonomic threading from optional config.
    #[must_use]
    pub const fn with_iteration_cap_opt(mut self, cap: Option<u32>) -> Self {
        self.iteration_cap = cap;
        self
    }

    /// Get the content of an identity file by name (e.g. `"SOUL.md"`).
    #[must_use]
    pub fn identity_file(&self, name: &str) -> Option<&str> {
        self.identity_files.and_then(|files| files.get(name))
    }
}

/// A composable unit of prompt assembly.
///
/// Each layer appends its contribution to the system prompt string.
/// Layers declare which assembly paths they participate in and a
/// numeric priority that controls ordering (lower = earlier).
pub trait PromptLayer: Send + Sync {
    /// Human-readable name for debugging / logging.
    fn name(&self) -> &'static str;

    /// Sort key — layers are executed in ascending priority order.
    fn priority(&self) -> u32;

    /// Which assembly paths this layer participates in.
    fn paths(&self) -> &'static [AssemblyPath];

    /// Whether this layer participates in the given [`PromptMode`].
    ///
    /// The default returns `true` for all modes.  Override in layers
    /// that should be excluded from Compact or Minimal prompts.
    fn supports_mode(&self, _mode: PromptMode) -> bool {
        true
    }

    /// Whether this layer produces stable or dynamic content.
    ///
    /// Stable layers are grouped before dynamic layers in the assembled prompt
    /// so the stable prefix can be cached by the LLM provider.
    ///
    /// **Deliberately has no default.** It used to default to
    /// [`LayerStability::Stable`], which meant a layer that simply never
    /// mentioned stability rode into the provider-cached prefix by omission —
    /// and that is not a hypothetical: `ToolRuntimeStateLayer` sat at priority
    /// 502 without declaring, so a 30-second tool-health probe silently
    /// invalidated the cacheable prefix for entire sessions. A human reading the
    /// code found that, not a mechanism. Requiring the method moves the catch to
    /// the compiler: a new layer cannot be written without answering the
    /// question, and the answer is visible at the layer instead of inferred from
    /// its absence. (Ported idiom: codex forces the same triage on new request
    /// fields via an exhaustive destructure — `codex-rs/core/src/client.rs`.)
    ///
    /// Answer it by asking **whether these bytes change within a session** —
    /// not by priority, not by whether the content is re-read from disk each
    /// build, and not by what the content is *about*. Using any of those to
    /// infer this is how layers end up in the wrong region (FEATURE_LOCATOR
    /// §2.18 Round 3). If one layer's two halves disagree, split the layer
    /// rather than taking the worse rating for both (Round 4).
    fn stability(&self) -> LayerStability;

    /// Append this layer's content to `output`.
    fn inject(&self, output: &mut String, input: &LayerInput);
}

#[cfg(test)]
mod layer_input_tests {
    use super::*;
    use crate::thinker::identity_files::{IdentityFile, IdentityFiles};
    use crate::thinker::prompt_builder::PromptConfig;

    fn make_config() -> PromptConfig {
        PromptConfig::default()
    }

    #[test]
    fn layer_input_identity_file_access() {
        let config = make_config();
        let files = IdentityFiles {
            identity_dir: std::path::PathBuf::from("/tmp"),
            files: vec![IdentityFile {
                name: "SOUL.md",
                content: Some("You are Aleph.".to_string()),
            }],
        };

        let input = LayerInput::basic(&config, &[]).with_identity_files(&files);
        assert_eq!(input.identity_file("SOUL.md"), Some("You are Aleph."));
        assert_eq!(input.identity_file("MISSING.md"), None);
    }

    #[test]
    fn with_identity_files_opt_works() {
        let config = make_config();
        let files = IdentityFiles {
            identity_dir: std::path::PathBuf::from("/tmp"),
            files: vec![],
        };

        // None variant
        let input = LayerInput::basic(&config, &[]).with_identity_files_opt(None);
        assert!(input.identity_files.is_none());

        // Some variant
        let input = LayerInput::basic(&config, &[]).with_identity_files_opt(Some(&files));
        assert!(input.identity_files.is_some());
    }
}
