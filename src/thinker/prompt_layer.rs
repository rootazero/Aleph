//! PromptLayer trait — the composable unit of prompt assembly

use super::context::ResolvedContext;
use super::identity_files::IdentityFiles;
use super::inbound_context::InboundContext;
use super::prompt_builder::PromptConfig;
use super::prompt_mode::PromptMode;
use super::soul::SoulManifest;
use crate::agent_loop::ToolInfo;
use crate::agents::AgentDef;
use crate::dispatcher::tool_index::HydrationResult;

/// MCP server instruction metadata for prompt injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerInstruction {
    pub server_name: String,
    pub instructions: String,
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
    /// Loaded workspace files (SOUL.md, IDENTITY.md, etc.)
    pub workspace: Option<&'a IdentityFiles>,
    /// Pre-fetched memory context from LanceDB (facts + memory summaries).
    pub memory_context: Option<&'a super::memory_context::MemoryContext>,
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
            workspace: None,
            memory_context: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
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
            workspace: None,
            memory_context: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
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
            workspace: None,
            memory_context: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
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
            workspace: None,
            memory_context: None,
            has_session_summaries: false,
            agent_def: None,
            mcp_instructions: None,
            session_snapshot: None,
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

    /// Attach workspace files to this input.
    pub fn with_workspace(mut self, workspace: &'a IdentityFiles) -> Self {
        self.workspace = Some(workspace);
        self
    }

    /// Attach optional inbound context to this input.
    pub fn with_inbound_opt(mut self, inbound: Option<&'a InboundContext>) -> Self {
        self.inbound = inbound;
        self
    }

    /// Attach optional workspace files to this input.
    pub fn with_workspace_opt(mut self, workspace: Option<&'a IdentityFiles>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Attach pre-fetched memory context.
    pub fn with_memory_context(mut self, ctx: &'a super::memory_context::MemoryContext) -> Self {
        self.memory_context = Some(ctx);
        self
    }

    /// Attach optional pre-fetched memory context.
    pub fn with_memory_context_opt(
        mut self,
        ctx: Option<&'a super::memory_context::MemoryContext>,
    ) -> Self {
        self.memory_context = ctx;
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

    /// Get the content of a workspace file by name.
    pub fn workspace_file(&self, name: &str) -> Option<&str> {
        self.workspace.and_then(|ws| ws.get(name))
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
mod workspace_inbound_tests {
    use super::*;
    use crate::thinker::identity_files::{IdentityFile, IdentityFiles};
    use crate::thinker::inbound_context::{InboundContext, SenderInfo};
    use crate::thinker::prompt_builder::PromptConfig;

    fn make_config() -> PromptConfig {
        PromptConfig::default()
    }

    #[test]
    fn layer_input_workspace_file_access() {
        let config = make_config();
        let ws = IdentityFiles {
            workspace_dir: std::path::PathBuf::from("/tmp"),
            files: vec![IdentityFile {
                name: "SOUL.md",
                content: Some("You are Aleph.".to_string()),
                truncated: false,
                original_size: 14,
            }],
        };

        let input = LayerInput::basic(&config, &[]).with_workspace(&ws);
        assert_eq!(input.workspace_file("SOUL.md"), Some("You are Aleph."));
        assert_eq!(input.workspace_file("MISSING.md"), None);
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
        let ws = IdentityFiles {
            workspace_dir: std::path::PathBuf::from("/tmp"),
            files: vec![],
        };
        let inbound = InboundContext::default();

        // None variant
        let input = LayerInput::basic(&config, &[])
            .with_workspace_opt(None)
            .with_inbound_opt(None);
        assert!(input.workspace.is_none());
        assert!(input.inbound.is_none());

        // Some variant
        let input = LayerInput::basic(&config, &[])
            .with_workspace_opt(Some(&ws))
            .with_inbound_opt(Some(&inbound));
        assert!(input.workspace.is_some());
        assert!(input.inbound.is_some());
    }
}
