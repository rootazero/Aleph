//! PromptLayer trait — the composable unit of prompt assembly

use super::context::ResolvedContext;
use super::identity_files::IdentityFiles;
use super::inbound_context::InboundContext;
use super::prompt_builder::PromptConfig;
use super::prompt_mode::PromptMode;
use super::soul::SoulManifest;
use crate::agents::AgentDef;
use crate::tool_metadata::tool_index::HydrationResult;
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
/// The pipeline filters layers by the active path so that only
/// relevant sections are injected into the final system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssemblyPath {
    /// Minimal prompt — tools only, no hydration / soul / context.
    Basic,
    /// Hydration-based prompt — tools come from semantic retrieval.
    Hydration,
    /// Soul-enriched prompt — includes identity / personality.
    Soul,
    /// Context-aware prompt — includes environment / security context.
    Context,
    /// Pre-cached prompt — used when prompt caching is active.
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
    pub hydration: Option<&'a HydrationResult>,
    pub soul: Option<&'a SoulManifest>,
    pub context: Option<&'a ResolvedContext>,
    /// Active workspace profile (system_prompt overlay, tool whitelist, etc.)
    pub profile: Option<&'a crate::config::ProfileConfig>,
    /// Prompt mode for this assembly (default: Full)
    pub mode: PromptMode,
    /// Per-request inbound context (sender, channel, session metadata)
    pub inbound: Option<&'a InboundContext>,
    /// Loaded agent identity files (SOUL.md, IDENTITY.md, AGENTS.md,
    /// TOOLS.md, HEARTBEAT.md) from `~/.aleph/agents/{agent_id}/`.
    /// MEMORY.md is owned by the curated memory module and flows in via
    /// `curated_memory_envelope`, not this list.
    pub identity_files: Option<&'a IdentityFiles>,
    /// Pre-rendered memory XML from `MemoryContextProvider::build_memory_user_message`.
    ///
    /// When set, `MemoryAugmentationLayer` injects this text verbatim into the
    /// system prompt.  Takes precedence over the legacy `memory_context` field.
    pub memory_user_message: Option<String>,
    /// Whether the conversation history contains compressed session summaries.
    ///
    /// Set to `true` when session compaction has produced at least one
    /// `<session_context>` block in the message history.  The
    /// `SessionContextGuideLayer` uses this flag to inject usage guidance.
    pub has_session_summaries: bool,
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
    /// Previous session snapshot for cross-session context restoration.
    ///
    /// When set, `SessionResumeLayer` injects the snapshot summary,
    /// key decisions, active files, and pending tasks into the prompt.
    pub session_snapshot: Option<&'a crate::memory::session_resume::SessionSnapshot>,
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
    /// orchestrator and subagent_spawner already maintain). Layer omits
    /// the section entirely when the chain is at root (`depth == 0`) or
    /// the field is `None`, so the top-level harness path stays unaffected.
    pub chain_context: Option<&'a crate::harness::chain_context::ChainContext>,
    /// Wire-protocol family identifier reported by
    /// `AiProvider::protocol()` (with `model_behavior_override()` taking
    /// precedence). Used by `ProviderGuidanceLayer` to dispatch the
    /// appropriate per-family operational guidance block. `None` keeps
    /// the layer silent — desirable on non-LLM-driven paths (capture,
    /// snapshot, tests).
    pub provider_protocol: Option<&'a str>,
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
    pub fn basic(config: &'a PromptConfig, tools: &'a [ToolInfo]) -> Self {
        Self {
            config,
            tools: Some(tools),
            hydration: None,
            soul: None,
            context: None,
            profile: None,
            mode: PromptMode::Full,
            inbound: None,
            identity_files: None,
            memory_user_message: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
            curated_memory_envelope: None,
            chain_context: None,
            provider_protocol: None,
            iteration_cap: None,
            extra_files: None,
        }
    }

    /// Input for the `Hydration` path — config + hydration result.
    pub fn hydration(config: &'a PromptConfig, hydration: &'a HydrationResult) -> Self {
        Self {
            config,
            tools: None,
            hydration: Some(hydration),
            soul: None,
            context: None,
            profile: None,
            mode: PromptMode::Full,
            inbound: None,
            identity_files: None,
            memory_user_message: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
            curated_memory_envelope: None,
            chain_context: None,
            provider_protocol: None,
            iteration_cap: None,
            extra_files: None,
        }
    }

    /// Input for the `Soul` path — config + tools + soul manifest.
    pub fn soul(config: &'a PromptConfig, tools: &'a [ToolInfo], soul: &'a SoulManifest) -> Self {
        Self {
            config,
            tools: Some(tools),
            hydration: None,
            soul: Some(soul),
            context: None,
            profile: None,
            mode: PromptMode::Full,
            inbound: None,
            identity_files: None,
            memory_user_message: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
            curated_memory_envelope: None,
            chain_context: None,
            provider_protocol: None,
            iteration_cap: None,
            extra_files: None,
        }
    }

    /// Input for the `Context` path — config + resolved context.
    pub fn context(config: &'a PromptConfig, ctx: &'a ResolvedContext) -> Self {
        Self {
            config,
            tools: None,
            hydration: None,
            soul: None,
            context: Some(ctx),
            profile: None,
            mode: PromptMode::Full,
            inbound: None,
            identity_files: None,
            memory_user_message: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
            curated_memory_envelope: None,
            chain_context: None,
            provider_protocol: None,
            iteration_cap: None,
            extra_files: None,
        }
    }

    /// Attach workspace profile to this input.
    pub fn with_profile(mut self, profile: Option<&'a crate::config::ProfileConfig>) -> Self {
        self.profile = profile;
        self
    }

    /// Set the prompt mode for this assembly.
    pub fn with_mode(mut self, mode: PromptMode) -> Self {
        self.mode = mode;
        self
    }

    /// Attach inbound context to this input.
    pub fn with_inbound(mut self, inbound: &'a InboundContext) -> Self {
        self.inbound = Some(inbound);
        self
    }

    /// Attach agent identity files to this input.
    pub fn with_identity_files(mut self, files: &'a IdentityFiles) -> Self {
        self.identity_files = Some(files);
        self
    }

    /// Attach optional inbound context to this input.
    pub fn with_inbound_opt(mut self, inbound: Option<&'a InboundContext>) -> Self {
        self.inbound = inbound;
        self
    }

    /// Attach optional agent identity files to this input.
    pub fn with_identity_files_opt(mut self, files: Option<&'a IdentityFiles>) -> Self {
        self.identity_files = files;
        self
    }

    /// Attach optional `[prompt.extra_files]` content to this input.
    pub fn with_extra_files_opt(mut self, files: Option<&'a [ExtraPromptFile]>) -> Self {
        self.extra_files = files;
        self
    }

    /// Attach pre-rendered memory XML from `build_memory_user_message`.
    ///
    /// When set, `MemoryAugmentationLayer` injects this text verbatim and
    /// ignores the legacy `memory_context` field.
    pub fn with_memory_user_message(mut self, text: String) -> Self {
        self.memory_user_message = Some(text);
        self
    }

    /// Attach the pre-rendered curated memory envelope.
    ///
    /// When set, `CuratedMemoryLayer` injects this text verbatim into the
    /// system prompt as a Stable section so the prefix cache stays warm.
    pub fn with_curated_envelope(mut self, envelope: Option<String>) -> Self {
        self.curated_memory_envelope = envelope;
        self
    }

    /// Signal that the conversation contains compressed session summaries.
    pub fn with_session_summaries(mut self, has: bool) -> Self {
        self.has_session_summaries = has;
        self
    }

    /// Attach an agent definition for sub-agent prompt assembly.
    pub fn with_agent_def(mut self, agent_def: &'a AgentDef) -> Self {
        self.agent_def = Some(agent_def);
        self
    }

    /// Attach MCP server instructions for prompt injection.
    pub fn with_mcp_instructions(mut self, instructions: &'a [McpServerInstruction]) -> Self {
        self.mcp_instructions = Some(instructions);
        self
    }

    /// Attach a previous session snapshot for cross-session resume.
    pub fn with_session_snapshot(
        mut self,
        snapshot: &'a crate::memory::session_resume::SessionSnapshot,
    ) -> Self {
        self.session_snapshot = Some(snapshot);
        self
    }

    /// Attach a subagent call-chain context. Used by `ChainContextLayer`
    /// to surface depth / chain_id to the LLM. Root chains (depth=0) and
    /// `None` both render an empty section.
    pub fn with_chain_context(
        mut self,
        chain: &'a crate::harness::chain_context::ChainContext,
    ) -> Self {
        self.chain_context = Some(chain);
        self
    }

    /// Same as `with_chain_context` but accepts an `Option` for ergonomic
    /// threading from optional config.
    pub fn with_chain_context_opt(
        mut self,
        chain: Option<&'a crate::harness::chain_context::ChainContext>,
    ) -> Self {
        self.chain_context = chain;
        self
    }

    /// Attach an optional `ResolvedContext` to this input.
    ///
    /// Used by the Basic / Hydration / Soul entry points so the Phase 2
    /// widened layers (security, operational_guidelines, protocol_tokens,
    /// runtime_context) can consume context on every assembly path
    /// without forcing a switch to the dedicated `Context` route. The
    /// `Context` path's `LayerInput::context` constructor already sets
    /// this field, so callers on that path leave it alone.
    pub fn with_resolved_context_opt(mut self, ctx: Option<&'a ResolvedContext>) -> Self {
        self.context = ctx;
        self
    }

    /// Attach the wire-protocol family identifier so `ProviderGuidanceLayer`
    /// can dispatch the right per-family operational directives.
    pub fn with_provider_protocol(mut self, protocol: &'a str) -> Self {
        self.provider_protocol = Some(protocol);
        self
    }

    /// Same as `with_provider_protocol` but accepts an `Option` for
    /// ergonomic threading from optional config.
    pub fn with_provider_protocol_opt(mut self, protocol: Option<&'a str>) -> Self {
        self.provider_protocol = protocol;
        self
    }

    /// Attach the per-run iteration cap so `SessionBudgetLayer` can
    /// surface it. Pass the resolved (post-`resolve_max_iterations`)
    /// value, not the raw override.
    pub fn with_iteration_cap(mut self, cap: u32) -> Self {
        self.iteration_cap = Some(cap);
        self
    }

    /// Same as `with_iteration_cap` but accepts an `Option` for
    /// ergonomic threading from optional config.
    pub fn with_iteration_cap_opt(mut self, cap: Option<u32>) -> Self {
        self.iteration_cap = cap;
        self
    }

    /// Get the content of an identity file by name (e.g. `"SOUL.md"`).
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
    /// Stable layers are grouped before dynamic layers in the assembled
    /// prompt so that the stable prefix can be cached by the LLM provider.
    /// The default is [`LayerStability::Stable`]; override to `Dynamic`
    /// for layers whose output changes per request.
    fn stability(&self) -> LayerStability {
        LayerStability::Stable
    }

    /// Append this layer's content to `output`.
    fn inject(&self, output: &mut String, input: &LayerInput);
}

#[cfg(test)]
mod identity_inbound_tests {
    use super::*;
    use crate::thinker::identity_files::{IdentityFile, IdentityFiles};
    use crate::thinker::inbound_context::{InboundContext, SenderInfo};
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
                truncated: false,
                original_size: 14,
            }],
        };

        let input = LayerInput::basic(&config, &[]).with_identity_files(&files);
        assert_eq!(input.identity_file("SOUL.md"), Some("You are Aleph."));
        assert_eq!(input.identity_file("MISSING.md"), None);
    }

    #[test]
    fn layer_input_inbound_access() {
        let config = make_config();
        let inbound = InboundContext {
            sender: SenderInfo {
                id: "u42".to_string(),
                display_name: Some("Alice".to_string()),
                is_owner: true,
            },
            ..Default::default()
        };

        let input = LayerInput::basic(&config, &[]).with_inbound(&inbound);
        assert!(input.inbound.is_some());
        let ctx = input.inbound.unwrap();
        assert_eq!(ctx.sender.id, "u42");
        assert!(ctx.sender.is_owner);
    }

    #[test]
    fn with_opt_methods_work() {
        let config = make_config();
        let files = IdentityFiles {
            identity_dir: std::path::PathBuf::from("/tmp"),
            files: vec![],
        };
        let inbound = InboundContext::default();

        // None variant
        let input = LayerInput::basic(&config, &[])
            .with_identity_files_opt(None)
            .with_inbound_opt(None);
        assert!(input.identity_files.is_none());
        assert!(input.inbound.is_none());

        // Some variant
        let input = LayerInput::basic(&config, &[])
            .with_identity_files_opt(Some(&files))
            .with_inbound_opt(Some(&inbound));
        assert!(input.identity_files.is_some());
        assert!(input.inbound.is_some());
    }
}
