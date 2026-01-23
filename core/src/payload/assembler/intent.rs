/// Intent-based prompt building
///
/// This module handles building prompts based on execution intent
/// and execution mode.
use crate::intent::{AgentModePrompt, ExecutionIntent, ExecutionMode, TaskCategory};
use crate::payload::{AgentContext, ContextFormat};
use crate::prompt::{PromptBuilder, PromptConfig, ToolInfo};
use crate::capability::CapabilityDeclaration;
use super::context::format_context;
use super::capability::format_capability_instructions;

/// Build prompt with agent mode injection based on intent.
///
/// When the intent is `ExecutionIntent::Executable`, this method injects
/// the Agent Mode Prompt that guides AI to:
/// 1. Skip asking for options - present best plan directly
/// 2. Show plan summary with operations
/// 3. Wait for user confirmation before destructive operations
///
/// # Arguments
///
/// * `context_format` - The context format to use
/// * `base_prompt` - Base system prompt
/// * `capabilities` - List of available capabilities
/// * `context` - Optional existing context
/// * `intent` - Optional execution intent from IntentClassifier
///
/// # Returns
///
/// Complete system prompt with agent mode injection if applicable
pub fn build_prompt_with_intent(
    context_format: &ContextFormat,
    base_prompt: &str,
    capabilities: &[CapabilityDeclaration],
    context: Option<&AgentContext>,
    intent: Option<&ExecutionIntent>,
) -> String {
    let mut prompt = build_capability_aware_prompt(context_format, base_prompt, capabilities, context);

    // Inject agent mode prompt if intent is executable
    if let Some(ExecutionIntent::Executable(_)) = intent {
        let agent_prompt = AgentModePrompt::new().generate();
        prompt.push_str("\n\n");
        prompt.push_str(&agent_prompt);
    }

    prompt
}

/// Build capability-aware prompt (helper function)
fn build_capability_aware_prompt(
    context_format: &ContextFormat,
    base_prompt: &str,
    capabilities: &[CapabilityDeclaration],
    context: Option<&AgentContext>,
) -> String {
    let mut prompt = base_prompt.to_string();

    // Add capability instructions if any capabilities are available
    let available_caps: Vec<_> = capabilities.iter().filter(|c| c.available).collect();
    if !available_caps.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(&format_capability_instructions(&available_caps));
    }

    // Add existing context if provided
    if let Some(ctx) = context {
        if let Some(formatted_ctx) = format_context(context_format, ctx) {
            prompt.push_str("\n\n");
            prompt.push_str(&formatted_ctx);
        }
    }

    prompt
}

/// Build prompt using the new unified ExecutionMode system.
///
/// This is the new recommended method that uses `ExecutionIntentDecider`
/// results directly. It provides cleaner separation between execution
/// and conversation modes.
///
/// # Arguments
///
/// * `context_format` - The context format to use
/// * `execution_mode` - Mode determined by ExecutionIntentDecider
/// * `tools` - Available tools (only used in Execute mode)
/// * `context` - Optional existing context
/// * `config` - Optional prompt configuration
///
/// # Returns
///
/// Complete system prompt appropriate for the execution mode
pub fn build_prompt_with_execution_mode(
    context_format: &ContextFormat,
    execution_mode: &ExecutionMode,
    tools: &[ToolInfo],
    context: Option<&AgentContext>,
    config: Option<&PromptConfig>,
) -> String {
    let mut prompt = match execution_mode {
        ExecutionMode::DirectTool(invocation) => {
            // For direct tool calls, use minimal prompt
            PromptBuilder::direct_tool_prompt(&invocation.tool_id, "Execute the requested tool")
        }
        ExecutionMode::Skill(skill) => {
            // For skills, inject skill instructions as system context
            format!(
                "# Skill: {}\n\n{}\n\n---\n\n{}",
                skill.display_name,
                skill.instructions,
                PromptBuilder::executor_prompt(TaskCategory::General, tools, config)
            )
        }
        ExecutionMode::Mcp(mcp) => {
            // For MCP commands, use agent prompt with MCP tool hint
            let tool_hint = if let Some(ref tool_name) = mcp.tool_name {
                format!("Use the {} tool from {} server", tool_name, mcp.server_name)
            } else {
                format!("Use tools from the {} MCP server", mcp.server_name)
            };
            format!(
                "{}\n\n---\n\nTool hint: {}",
                PromptBuilder::executor_prompt(TaskCategory::General, tools, config),
                tool_hint
            )
        }
        ExecutionMode::Custom(custom) => {
            // For custom commands, use the custom system prompt if provided
            if let Some(ref system_prompt) = custom.system_prompt {
                system_prompt.clone()
            } else {
                PromptBuilder::executor_prompt(TaskCategory::General, tools, config)
            }
        }
        ExecutionMode::Execute(category) => {
            // For execution mode, use executor prompt with category-specific tools
            let category_tools = filter_tools_for_category(tools, *category);
            PromptBuilder::executor_prompt(*category, &category_tools, config)
        }
        ExecutionMode::Converse => {
            // For conversation mode, use conversational prompt (no tools)
            PromptBuilder::conversational_prompt(config)
        }
    };

    // Add context if provided (memory, search results, etc.)
    if let Some(ctx) = context {
        if let Some(formatted_ctx) = format_context(context_format, ctx) {
            prompt.push_str("\n\n");
            prompt.push_str(&formatted_ctx);
        }
    }

    prompt
}

/// Filter tools relevant to a task category.
///
/// This reduces tool list noise by only showing tools relevant to the task.
fn filter_tools_for_category(tools: &[ToolInfo], _category: TaskCategory) -> Vec<ToolInfo> {
    // For now, return all tools. In the future, this can be enhanced
    // to filter based on category-tool mappings.
    //
    // TODO: Implement category-specific tool filtering:
    // - FileOrganize/FileOperation → file_ops, search
    // - ImageGeneration → generate_image, vision_*
    // - WebSearch → search, web_fetch
    // - CodeExecution → code_runner, shell
    tools.to_vec()
}
