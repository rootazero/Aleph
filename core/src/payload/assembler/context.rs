/// Context formatting for different output formats
///
/// This module handles formatting of AgentContext data (memory, search, MCP)
/// into different formats (Markdown, XML, JSON).
use super::formatters::{
    format_facts_markdown, format_mcp_tool_result_markdown, format_memory_markdown,
    format_search_results_markdown, format_webfetch_content_markdown,
};
use crate::payload::{AgentContext, ContextFormat};

/// Format context data (memory, search, MCP) without base prompt
///
/// Use this when you need only the context part, not the full system prompt.
/// Useful for prepend mode where base prompt should be excluded.
///
/// Selects formatting strategy based on context_format
pub fn format_context(context_format: &ContextFormat, context: &AgentContext) -> Option<String> {
    match context_format {
        ContextFormat::Markdown => format_markdown(context),
        ContextFormat::Xml => format_xml(context),
        ContextFormat::Json => format_json(context),
    }
}

/// Markdown formatting (MVP implementation)
fn format_markdown(context: &AgentContext) -> Option<String> {
    let mut sections = Vec::new();

    // Facts section (Layer 2 - priority, more concise)
    if let Some(facts) = &context.memory_facts {
        if !facts.is_empty() {
            let facts_section = format_facts_markdown(facts);
            sections.push(facts_section);
        }
    }

    // Memory section (Layer 1 - fallback, full conversation history)
    if let Some(memories) = &context.memory_snippets {
        if !memories.is_empty() {
            let memory_section = format_memory_markdown(memories);
            sections.push(memory_section);
        }
    }

    // Search section
    if let Some(results) = &context.search_results {
        if !results.is_empty() {
            let search_section = format_search_results_markdown(results);
            sections.push(search_section);
        }
    }

    // MCP tool result section
    if let Some(result) = &context.mcp_tool_result {
        let mcp_section = format_mcp_tool_result_markdown(result);
        sections.push(mcp_section);
    }

    // MCP resources section (tool listing, less commonly used)
    if let Some(_resources) = &context.mcp_resources {
        // Tool listing is handled in capability instructions, not here
    }

    // WebFetch content section
    if let Some(webfetch) = &context.webfetch_content {
        let webfetch_section = format_webfetch_content_markdown(webfetch);
        sections.push(webfetch_section);
    }

    // NOTE: skill_instructions field is deprecated in favor of Progressive Disclosure pattern.
    // Skills are now loaded on-demand via read_skill tool.
    // The skill metadata is injected separately via format_available_skills_metadata().
    // This block is kept for backward compatibility but should be empty in new code.
    if let Some(instructions) = &context.skill_instructions {
        if !instructions.is_empty() {
            // Legacy: only inject if explicitly set (for backward compatibility during migration)
            let skill_section = format!("## Skill Instructions (Legacy)\n\n{}", instructions);
            sections.push(skill_section);
        }
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "### Context Information\n\n{}",
            sections.join("\n\n")
        ))
    }
}

/// XML formatting (reserved for future)
fn format_xml(_context: &AgentContext) -> Option<String> {
    // TODO: Implement XML formatting
    None
}

/// JSON formatting (reserved for future)
fn format_json(_context: &AgentContext) -> Option<String> {
    // TODO: Implement JSON formatting
    None
}
