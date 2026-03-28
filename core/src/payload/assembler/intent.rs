/// Intent-based prompt building
///
/// This module handles building prompts based on `IntentResult`.
use crate::intent::{IntentResult, TaskCategory};
use crate::payload::{AgentContext, ContextFormat};
use crate::prompt::{PromptBuilder, PromptConfig, ToolInfo};
use crate::capability::CapabilityDeclaration;
use super::context::format_context;
use super::core::PromptAssembler;

/// Build prompt with agent mode injection based on `IntentResult`.
///
/// When the result is `Execute` or `DirectTool`, this method injects
/// the Agent Mode Prompt that guides AI to:
/// 1. Skip asking for options - present best plan directly
/// 2. Show plan summary with operations
/// 3. Wait for user confirmation before destructive operations
pub fn build_prompt_with_intent_result(
    context_format: &ContextFormat,
    base_prompt: &str,
    capabilities: &[CapabilityDeclaration],
    context: Option<&AgentContext>,
    result: Option<&IntentResult>,
) -> String {
    // Reuse PromptAssembler to avoid duplicating capability+context logic
    let assembler = PromptAssembler::new(*context_format);
    let mut prompt = assembler.build_capability_aware_prompt(base_prompt, capabilities, context);

    // Inject agent mode hint if intent is actionable
    if let Some(IntentResult::Execute { .. }) | Some(IntentResult::DirectTool { .. }) = result {
        prompt.push_str("\n\nYou are in execution mode. Present your best plan directly, show a summary of operations, and wait for user confirmation before destructive operations.");
    }

    prompt
}

/// Build prompt using the new `IntentResult` enum.
///
/// Maps each variant to the appropriate `PromptBuilder` method:
/// - `DirectTool` → `direct_tool_prompt`
/// - `Execute` → `executor_prompt` (General category)
/// - `Converse` → `conversational_prompt`
/// - `Abort` → `conversational_prompt` (graceful fallback)
pub fn build_prompt_for_intent(
    context_format: &ContextFormat,
    result: &IntentResult,
    tools: &[ToolInfo],
    context: Option<&AgentContext>,
    config: Option<&PromptConfig>,
) -> String {
    let mut prompt = match result {
        IntentResult::DirectTool { tool_id, args, .. } => {
            PromptBuilder::direct_tool_prompt(tool_id, args.as_deref().unwrap_or(""))
        }
        IntentResult::Execute { .. } => {
            PromptBuilder::executor_prompt(TaskCategory::General, tools, config)
        }
        IntentResult::Converse { .. } => PromptBuilder::conversational_prompt(config),
        IntentResult::Abort => PromptBuilder::conversational_prompt(config),
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
