//! Prompt builder for Agent Loop
//!
//! This module builds prompts for the LLM thinking step,
//! including system prompts and message history.

use crate::agent_loop::{LoopState, Observation, StepSummary, ToolInfo};
use crate::core::MediaAttachment;
use crate::dispatcher::tool_index::HydrationResult;

use super::context::ResolvedContext;
use super::prompt_layer::{AssemblyPath, LayerInput};
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
    pub fn build_system_prompt_with_soul(&self, tools: &[ToolInfo], soul: &SoulManifest) -> String {
        let input = LayerInput::soul(&self.config, tools, soul);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }

    /// Build system prompt using ResolvedContext
    ///
    /// This is the new entry point that uses the two-phase filtered context
    /// from the ContextAggregator. The pipeline layers handle all sections
    /// (runtime context, environment, security, protocol tokens, etc.)
    /// in priority order.
    pub fn build_system_prompt_with_context(&self, ctx: &ResolvedContext) -> String {
        let input = LayerInput::context(&self.config, ctx);
        self.pipeline.execute(AssemblyPath::Context, &input)
    }

    /// Build two-part system prompt for Anthropic cache optimization
    ///
    /// Returns a vector of SystemPromptParts where:
    /// - Part 1: Static header (cacheable) - role definition, core instructions
    /// - Part 2: Dynamic content (not cacheable) - tools, runtimes, custom instructions
    ///
    /// This maximizes Anthropic's prompt cache hit rate by keeping
    /// the frequently-repeated header separate from dynamic content.
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        let header = Self::build_static_header();
        let input = LayerInput::basic(&self.config, tools);
        let dynamic = self.pipeline.execute(AssemblyPath::Cached, &input);

        vec![
            SystemPromptPart {
                content: header,
                cache: true,
            },
            SystemPromptPart {
                content: dynamic,
                cache: false,
            },
        ]
    }

    /// Build the static header portion of the system prompt
    ///
    /// This content is stable across invocations and can be cached.
    fn build_static_header() -> String {
        let mut prompt = String::new();

        // Role definition
        prompt.push_str("You are an AI assistant executing tasks step by step.\n\n");

        // Core instructions
        prompt.push_str("## Your Role\n");
        prompt.push_str("- Observe the current state and history\n");
        prompt.push_str("- Decide the SINGLE next action to take\n");
        prompt.push_str("- Execute until the task is complete or you need user input\n\n");

        // Decision framework
        prompt.push_str("## Decision Framework\n");
        prompt.push_str("For each step, consider:\n");
        prompt.push_str("1. What is the current state?\n");
        prompt.push_str("2. What is the next logical step?\n");
        prompt.push_str("3. Which tool is most appropriate?\n\n");

        prompt
    }

    /// Build messages for the thinking step
    pub fn build_messages(
        &self,
        original_request: &str,
        observation: &Observation,
    ) -> Vec<Message> {
        let mut messages = Vec::new();

        // 1. User's original request with context
        let mut user_msg = format!("Task: {}\n", original_request);

        // Add attachments info
        if !observation.attachments.is_empty() {
            user_msg.push_str("\nAttachments:\n");
            for (i, attachment) in observation.attachments.iter().enumerate() {
                user_msg.push_str(&format!("{}. {}\n", i + 1, format_attachment(attachment)));
            }
        }

        messages.push(Message::user(user_msg));

        // 2. Compressed history summary (if any)
        if !observation.history_summary.is_empty() {
            messages.push(Message::assistant(format!(
                "[Previous steps summary]\n{}",
                observation.history_summary
            )));
        }

        // 3. Recent steps with full details
        for step in &observation.recent_steps {
            // Assistant's thinking and action
            messages.push(Message::assistant(format!(
                "Reasoning: {}\nAction: {} {}",
                step.reasoning, step.action_type, step.action_args
            )));

            // CRITICAL FIX: User responses must use User role, not Tool role
            // This ensures the LLM understands the user has answered the question
            // and doesn't ask the same question again
            if step.action_type == "ask_user" {
                // User's response to a question - use User role
                messages.push(Message::user(step.result_output.clone()));
            } else {
                // Tool result - use full output to ensure LLM sees complete data
                // (e.g., full file paths, complete JSON output)
                messages.push(Message::tool_result(&step.action_type, &step.result_output));
            }
        }

        // 4. Current context and request for next action
        // IMPORTANT: Use clear system-level language to avoid confusing agent
        // with user instructions (e.g., "Current step: X" was misinterpreted
        // as user requesting to restart at step X, causing infinite loops)
        let context_msg = format!(
            "[System] Loop iteration: {} | Tokens: {} | Continue with your next action.",
            observation.current_step, observation.total_tokens
        );
        messages.push(Message::user(context_msg));

        messages
    }

    /// Build observation from state
    pub fn build_observation(
        &self,
        state: &LoopState,
        tools: &[ToolInfo],
        window_size: usize,
    ) -> Observation {
        let recent_steps: Vec<StepSummary> = state
            .recent_steps(window_size)
            .iter()
            .map(StepSummary::from)
            .collect();

        Observation {
            history_summary: state.history_summary.clone(),
            recent_steps,
            available_tools: tools.to_vec(),
            attachments: state.context.attachments.clone(),
            current_step: state.step_count,
            total_tokens: state.total_tokens,
        }
    }
}

/// Message type for LLM conversation
#[derive(Debug, Clone)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

/// Message role
#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

impl Message {
    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }

    /// Create a tool result message
    pub fn tool_result(tool_name: &str, result: &str) -> Self {
        Self {
            role: MessageRole::Tool,
            content: format!("[{}]\n{}", tool_name, result),
        }
    }
}

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

/// Format attachment for display
fn format_attachment(attachment: &MediaAttachment) -> String {
    let preview = truncate_str(&attachment.data, 50);

    match attachment.media_type.as_str() {
        "image" => {
            format!(
                "Image ({}, {} bytes)",
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "document" => {
            format!(
                "Document: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        "file" => {
            format!(
                "File: {} ({}, {} bytes)",
                attachment.filename.as_deref().unwrap_or("unnamed"),
                attachment.mime_type,
                attachment.size_bytes
            )
        }
        _ => {
            format!(
                "{}: {} ({} bytes)",
                attachment.media_type,
                attachment.filename.as_deref().unwrap_or(&preview),
                attachment.size_bytes
            )
        }
    }
}

// Tests migrated to BDD format in core/tests/features/thinker/prompt_builder.feature

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Integration tests: public API via Pipeline ==========

    #[test]
    fn test_build_system_prompt_with_soul() {
        let builder = PromptBuilder::new(PromptConfig::default());

        let soul = SoulManifest {
            identity: "I am Aleph.".to_string(),
            directives: vec!["Help users".to_string()],
            ..Default::default()
        };

        let prompt = builder.build_system_prompt_with_soul(&[], &soul);

        // Soul should appear first
        let identity_pos = prompt.find("# Identity").unwrap();
        let role_pos = prompt.find("Your Role").unwrap();
        assert!(
            identity_pos < role_pos,
            "Identity should appear before Role"
        );

        // Standard sections should still be present
        assert!(prompt.contains("Response Format"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_thinking_guidance_disabled_by_default() {
        let builder = PromptBuilder::new(PromptConfig::default());
        let prompt = builder.build_system_prompt(&[]);

        // Default is off, so no thinking transparency section
        assert!(!prompt.contains("Thinking Transparency"));
        assert!(!prompt.contains("Reasoning Flow"));
    }

    #[test]
    fn test_thinking_guidance_enabled() {
        let config = PromptConfig {
            thinking_transparency: true,
            ..Default::default()
        };
        let builder = PromptBuilder::new(config);
        let prompt = builder.build_system_prompt(&[]);

        // Should contain thinking transparency section
        assert!(prompt.contains("## Thinking Transparency"));
        assert!(prompt.contains("### Reasoning Flow"));

        // Should contain the four phases
        assert!(prompt.contains("**Observation**"));
        assert!(prompt.contains("**Analysis**"));
        assert!(prompt.contains("**Planning**"));
        assert!(prompt.contains("**Decision**"));

        // Should contain uncertainty guidance
        assert!(prompt.contains("Expressing Uncertainty"));
        assert!(prompt.contains("High confidence"));
        assert!(prompt.contains("Low confidence"));

        // Should contain alternatives guidance
        assert!(prompt.contains("Acknowledging Alternatives"));
    }

    #[test]
    fn test_thinking_guidance_with_soul() {
        let config = PromptConfig {
            thinking_transparency: true,
            ..Default::default()
        };
        let builder = PromptBuilder::new(config);

        let soul = SoulManifest {
            identity: "Test assistant.".to_string(),
            ..Default::default()
        };

        let prompt = builder.build_system_prompt_with_soul(&[], &soul);

        // Both soul and thinking guidance should be present
        assert!(prompt.contains("# Identity"));
        assert!(prompt.contains("## Thinking Transparency"));
    }

    #[test]
    fn test_build_system_prompt_with_context_includes_runtime_context() {
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::security_context::SecurityContext;

        let builder = PromptBuilder::new(PromptConfig::default());

        // Build a ResolvedContext with runtime_context set
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let mut ctx = ContextAggregator::resolve(&interaction, &security, &[]);

        ctx.runtime_context = Some(crate::thinker::runtime_context::RuntimeContext {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: std::path::PathBuf::from("/home/user"),
            repo_root: None,
            current_model: "gpt-4".to_string(),
            hostname: "server-01".to_string(),
        });

        let prompt = builder.build_system_prompt_with_context(&ctx);

        // Runtime context should be present
        assert!(prompt.contains("## Runtime Environment"));
        assert!(prompt.contains("os=linux"));
        assert!(prompt.contains("model=gpt-4"));

        // Runtime context should appear before environment contract
        let runtime_pos = prompt.find("## Runtime Environment").unwrap();
        let env_pos = prompt.find("## Environment").unwrap();
        assert!(
            runtime_pos < env_pos,
            "Runtime context should appear before environment contract"
        );
    }

    #[test]
    fn test_build_system_prompt_with_context_no_runtime_context() {
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::security_context::SecurityContext;

        let builder = PromptBuilder::new(PromptConfig::default());

        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);

        // runtime_context should be None by default
        assert!(ctx.runtime_context.is_none());

        let prompt = builder.build_system_prompt_with_context(&ctx);

        // Runtime context section should NOT be present
        assert!(!prompt.contains("## Runtime Environment"));
    }

    // ========== Integration tests: full prompt assembly ==========

    #[test]
    fn test_full_prompt_with_all_enhancements_background_mode() {
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::runtime_context::RuntimeContext;
        use crate::thinker::security_context::SecurityContext;

        let builder = PromptBuilder::new(PromptConfig::default());

        // Build a Background-mode context (should trigger all 4 enhancements)
        let interaction = InteractionManifest::new(InteractionParadigm::Background);
        let security = SecurityContext::permissive();
        let mut resolved = ContextAggregator::resolve(&interaction, &security, &[]);

        // Add RuntimeContext
        resolved.runtime_context = Some(RuntimeContext {
            os: "macOS 15.3".to_string(),
            arch: "aarch64".to_string(),
            shell: "zsh".to_string(),
            working_dir: std::path::PathBuf::from("/workspace"),
            repo_root: Some(std::path::PathBuf::from("/workspace")),
            current_model: "claude-opus-4-6".to_string(),
            hostname: "test-host".to_string(),
        });

        let prompt = builder.build_system_prompt_with_context(&resolved);

        // 1. RuntimeContext should be present
        assert!(
            prompt.contains("## Runtime Environment"),
            "Missing RuntimeContext section"
        );
        assert!(prompt.contains("os=macOS 15.3"), "Missing OS info");
        assert!(
            prompt.contains("model=claude-opus-4-6"),
            "Missing model info"
        );

        // 2. Protocol tokens should be present (Background has SilentReply)
        assert!(
            prompt.contains("ALEPH_HEARTBEAT_OK"),
            "Missing protocol tokens: ALEPH_HEARTBEAT_OK"
        );
        assert!(
            prompt.contains("ALEPH_SILENT_COMPLETE"),
            "Missing protocol tokens: ALEPH_SILENT_COMPLETE"
        );

        // 3. Operational guidelines should be present (Background mode)
        assert!(
            prompt.contains("System Operational Awareness"),
            "Missing operational guidelines"
        );
        assert!(
            prompt.contains("Diagnostic Capabilities"),
            "Missing diagnostic capabilities in operational guidelines"
        );

        // 4. Citation standards should be present (always injected)
        assert!(
            prompt.contains("Citation Standards"),
            "Missing citation standards"
        );
        assert!(
            prompt.contains("citation is mandatory"),
            "Missing citation requirement"
        );

        // Standard sections should still be present
        assert!(prompt.contains("Your Role"), "Missing role section");
        assert!(
            prompt.contains("Response Format"),
            "Missing response format section"
        );

        // Verify ordering: RuntimeContext -> Environment -> Protocol -> Guidelines -> Citations
        let runtime_pos = prompt.find("## Runtime Environment").unwrap();
        let env_pos = prompt.find("## Environment").unwrap();
        let protocol_pos = prompt.find("Response Protocol Tokens").unwrap();
        let guidelines_pos = prompt.find("System Operational Awareness").unwrap();
        let citation_pos = prompt.find("Citation Standards").unwrap();

        assert!(
            runtime_pos < env_pos,
            "RuntimeContext should appear before Environment contract"
        );
        assert!(
            env_pos < protocol_pos,
            "Environment should appear before Protocol tokens"
        );
        assert!(
            protocol_pos < guidelines_pos,
            "Protocol tokens should appear before Operational guidelines"
        );
        assert!(
            guidelines_pos < citation_pos,
            "Operational guidelines should appear before Citation standards"
        );
    }

    #[test]
    fn test_interactive_prompt_minimal_token_overhead() {
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::runtime_context::RuntimeContext;
        use crate::thinker::security_context::SecurityContext;

        let builder = PromptBuilder::new(PromptConfig::default());

        // Build a WebRich-mode context (interactive, not background)
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let mut resolved = ContextAggregator::resolve(&interaction, &security, &[]);

        // Add RuntimeContext (should still be included for interactive)
        resolved.runtime_context = Some(RuntimeContext {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: std::path::PathBuf::from("/home/user"),
            repo_root: None,
            current_model: "gpt-4".to_string(),
            hostname: "web-server".to_string(),
        });

        let prompt = builder.build_system_prompt_with_context(&resolved);

        // 1. RuntimeContext SHOULD be present (always injected when provided)
        assert!(
            prompt.contains("## Runtime Environment"),
            "RuntimeContext should be present in WebRich mode"
        );
        assert!(prompt.contains("os=linux"), "Missing OS info in WebRich mode");
        assert!(
            prompt.contains("model=gpt-4"),
            "Missing model info in WebRich mode"
        );

        // 2. Protocol tokens should NOT be present (WebRich has no SilentReply)
        assert!(
            !prompt.contains("ALEPH_HEARTBEAT_OK"),
            "Protocol tokens should NOT be present in WebRich mode"
        );
        assert!(
            !prompt.contains("Response Protocol Tokens"),
            "Protocol tokens section should NOT be present in WebRich mode"
        );

        // 3. Operational guidelines should NOT be present (WebRich is not Background/CLI)
        assert!(
            !prompt.contains("System Operational Awareness"),
            "Operational guidelines should NOT be present in WebRich mode"
        );

        // 4. Citation standards SHOULD be present (always injected)
        assert!(
            prompt.contains("Citation Standards"),
            "Citation standards should be present in WebRich mode"
        );
        assert!(
            prompt.contains("citation is mandatory"),
            "Citation requirement should be present in WebRich mode"
        );

        // Standard sections should be present
        assert!(prompt.contains("Your Role"), "Missing role section");
        assert!(
            prompt.contains("Response Format"),
            "Missing response format section"
        );
    }
}
