/// PromptAssembler - Core assembler structure and main methods
///
/// This module contains the core PromptAssembler struct and its primary
/// public methods for building prompts.
use super::context::format_context;
use super::capability::format_capability_instructions;
use super::tools::format_available_tools;
use crate::payload::{AgentContext, AgentPayload, ContextFormat, SkillMetadata};

/// Prompt assembler that converts AgentPayload to LLM message format
///
/// Supports different context injection formats (Markdown, XML, JSON)
pub struct PromptAssembler {
    pub(crate) context_format: ContextFormat,
}

impl PromptAssembler {
    /// Create a new prompt assembler
    ///
    /// # Arguments
    ///
    /// * `format` - Context injection format to use
    pub fn new(format: ContextFormat) -> Self {
        Self {
            context_format: format,
        }
    }

    /// Build a capability-aware system prompt for AI-first intent detection.
    ///
    /// This method creates a system prompt that:
    /// 1. Includes the base prompt
    /// 2. Describes available capabilities to the AI
    /// 3. Instructs AI how to request capability invocation via JSON
    /// 4. Optionally includes existing context (memory)
    ///
    /// # Arguments
    ///
    /// * `base_prompt` - Base system prompt from routing rule or provider
    /// * `capabilities` - List of available capabilities
    /// * `context` - Optional existing context (memory snippets, etc.)
    ///
    /// # Returns
    ///
    /// Complete system prompt with capability instructions
    pub fn build_capability_aware_prompt(
        &self,
        base_prompt: &str,
        capabilities: &[crate::capability::CapabilityDeclaration],
        context: Option<&AgentContext>,
    ) -> String {
        self.build_capability_aware_prompt_with_tools(base_prompt, capabilities, None, context)
    }

    /// Complete system prompt with capability instructions and unified tool list.
    ///
    /// This method builds a prompt that includes:
    /// 1. Base prompt
    /// 2. Capability instructions (Search, Video, Tool execution, etc.)
    /// 3. Available Tools section (all 5 types with proper categorization)
    /// 4. Optional context (memory, search results, etc.)
    ///
    /// # Arguments
    ///
    /// * `base_prompt` - Base system prompt from routing rule or provider
    /// * `capabilities` - List of available capabilities
    /// * `tools_prompt_block` - Optional unified tools list from ToolRegistry.to_prompt_block()
    /// * `context` - Optional existing context (memory snippets, etc.)
    pub fn build_capability_aware_prompt_with_tools(
        &self,
        base_prompt: &str,
        capabilities: &[crate::capability::CapabilityDeclaration],
        tools_prompt_block: Option<&str>,
        context: Option<&AgentContext>,
    ) -> String {
        let mut prompt = base_prompt.to_string();

        // Add capability instructions if any capabilities are available
        let available_caps: Vec<_> = capabilities.iter().filter(|c| c.available).collect();
        if !available_caps.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&format_capability_instructions(&available_caps));
        }

        // Add unified tools section if provided
        // This shows all 5 tool types (Builtin, Native, MCP, Skills, Custom) with proper categorization
        if let Some(tools_block) = tools_prompt_block {
            if !tools_block.is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(&format_available_tools(tools_block));
            }
        }

        // Add existing context if provided
        if let Some(ctx) = context {
            if let Some(formatted_ctx) = format_context(&self.context_format, ctx) {
                prompt.push_str("\n\n");
                prompt.push_str(&formatted_ctx);
            }
        }

        prompt
    }

    /// Assemble complete system prompt
    ///
    /// Format: {base_prompt}\n\n{formatted_context}
    ///
    /// # Arguments
    ///
    /// * `base_prompt` - Base system prompt from routing rule or provider
    /// * `payload` - Agent payload containing context data
    ///
    /// # Returns
    ///
    /// Complete system prompt with context data appended
    pub fn assemble_system_prompt(&self, base_prompt: &str, payload: &AgentPayload) -> String {
        let mut prompt = base_prompt.to_string();

        // Append formatted context if available
        if let Some(formatted_ctx) = format_context(&self.context_format, &payload.context) {
            prompt.push_str("\n\n");
            prompt.push_str(&formatted_ctx);
        }

        prompt
    }

    /// Format context data (memory, search, MCP) without base prompt
    ///
    /// Use this when you need only the context part, not the full system prompt.
    /// Useful for prepend mode where base prompt should be excluded.
    ///
    /// Selects formatting strategy based on context_format
    pub fn format_context(&self, context: &AgentContext) -> Option<String> {
        format_context(&self.context_format, context)
    }

    /// Format available skills metadata for system prompt (Progressive Disclosure Level 1)
    ///
    /// This method generates the skill metadata section that goes into the system prompt.
    /// It only includes skill names and descriptions (Level 1), not full instructions.
    ///
    /// The agent will use `read_skill` tool to load full instructions when needed (Level 2).
    ///
    /// # Arguments
    ///
    /// * `skills` - List of available skill metadata (id, name, description)
    ///
    /// # Returns
    ///
    /// Formatted skill metadata section, or None if no skills available
    pub fn format_available_skills_metadata(&self, skills: &[SkillMetadata]) -> Option<String> {
        if skills.is_empty() {
            return None;
        }

        let mut lines = vec!["## Available Skills".to_string()];
        lines.push(String::new());
        lines.push("The following skills are installed and can be used to handle specific tasks:".to_string());
        lines.push(String::new());

        for skill in skills {
            lines.push(format!("- **{}**: {}", skill.id, skill.description));
        }

        lines.push(String::new());
        lines.push("### How to Use Skills".to_string());
        lines.push(String::new());
        lines.push("When a user request matches a skill's purpose:".to_string());
        lines.push("1. Use `read_skill(skill_id)` to load the skill's complete instructions".to_string());
        lines.push("2. Follow the loaded instructions exactly - they are task directives, not suggestions".to_string());
        lines.push("3. You can also read additional resources within a skill using `read_skill(skill_id, file_name)`".to_string());
        lines.push(String::new());
        lines.push("**Important**: Skill instructions are mandatory guidance that must be followed precisely.".to_string());

        Some(lines.join("\n"))
    }
}
