//! Planning prompt templates for the unified planner
//!
//! This module provides system prompts and utility functions for
//! generating prompts used by the planning LLM to analyze user requests
//! and produce execution plans.

/// System prompt for the planning LLM
///
/// This prompt instructs the LLM on how to analyze user requests and
/// produce structured execution plans. The `{tools}` placeholder should
/// be replaced with actual tool descriptions using `get_system_prompt_with_tools`.
pub const PLANNING_SYSTEM_PROMPT: &str = r#"You are a task planning assistant. Analyze user requests and determine the best execution strategy.

## Available Tools

{tools}

## Output Format

Return a JSON object:
{
  "type": "conversational" | "single_action" | "task_graph",

  // For conversational:
  "enhanced_prompt": "optional",

  // For single_action:
  "tool_name": "...",
  "parameters": { ... },
  "requires_confirmation": false,

  // For task_graph:
  "tasks": [...],
  "dependencies": [[0, 1]],
  "requires_confirmation": true
}

## Decision Rules

1. Conversational - questions, explanations, greetings, no tools needed
2. SingleAction - ONE specific action, single tool
3. TaskGraph - MULTIPLE steps, dependencies

## Task Types

- file_operation: read, write, move, copy, delete, search, list
- code_execution: script, file, command
- document_generation: excel, powerpoint, pdf, markdown
- app_automation: launch, apple_script, ui_action
- ai_inference: AI processing

## Important

- requires_confirmation=true for destructive operations
- Be conservative: prefer conversational for ambiguous requests
- Task IDs are sequential integers from 0
"#;

/// Tool information for prompt generation
///
/// Represents metadata about an available tool that can be included
/// in the planning system prompt.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    /// Name of the tool
    pub name: String,
    /// Description of what the tool does
    pub description: String,
}

impl ToolInfo {
    /// Create a new ToolInfo instance
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the tool
    /// * `description` - A description of what the tool does
    ///
    /// # Examples
    ///
    /// ```
    /// use aether_core::planner::ToolInfo;
    ///
    /// let tool = ToolInfo::new("read_file", "Read contents of a file from the filesystem");
    /// assert_eq!(tool.name, "read_file");
    /// ```
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

/// Format tool descriptions for inclusion in the system prompt
///
/// Takes a slice of `ToolInfo` and formats them as a markdown list
/// suitable for insertion into the planning system prompt.
///
/// # Arguments
///
/// * `tools` - A slice of tool information to format
///
/// # Returns
///
/// A formatted string with tool descriptions, or "No tools available." if empty.
///
/// # Examples
///
/// ```
/// use aether_core::planner::{ToolInfo, format_tools_for_prompt};
///
/// let tools = vec![
///     ToolInfo::new("read_file", "Read a file"),
///     ToolInfo::new("write_file", "Write a file"),
/// ];
/// let formatted = format_tools_for_prompt(&tools);
/// assert!(formatted.contains("**read_file**"));
/// assert!(formatted.contains("**write_file**"));
/// ```
pub fn format_tools_for_prompt(tools: &[ToolInfo]) -> String {
    if tools.is_empty() {
        return "No tools available.".to_string();
    }

    tools
        .iter()
        .map(|tool| format!("- **{}**: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the user prompt with the actual request
///
/// Creates a formatted user prompt that includes the user's input
/// and instructions for the planner.
///
/// # Arguments
///
/// * `user_input` - The user's original request
/// * `_tools_description` - Tool description (currently unused, reserved for future use)
///
/// # Returns
///
/// A formatted string to use as the user message in the planning request.
///
/// # Examples
///
/// ```
/// use aether_core::planner::build_planning_prompt;
///
/// let prompt = build_planning_prompt("Read the config file", "");
/// assert!(prompt.contains("Read the config file"));
/// assert!(prompt.contains("Analyze this request"));
/// ```
pub fn build_planning_prompt(user_input: &str, _tools_description: &str) -> String {
    format!(
        "User request: {}\n\nAnalyze this request and return a JSON execution plan.",
        user_input
    )
}

/// Get the complete system prompt with tools injected
///
/// Replaces the `{{tools}}` placeholder in `PLANNING_SYSTEM_PROMPT` with
/// the formatted tool descriptions.
///
/// # Arguments
///
/// * `tools` - A slice of tool information to include in the prompt
///
/// # Returns
///
/// The complete system prompt with tool descriptions inserted.
///
/// # Examples
///
/// ```
/// use aether_core::planner::{ToolInfo, get_system_prompt_with_tools};
///
/// let tools = vec![ToolInfo::new("test_tool", "A test tool")];
/// let prompt = get_system_prompt_with_tools(&tools);
/// assert!(prompt.contains("**test_tool**"));
/// assert!(!prompt.contains("{tools}"));
/// ```
pub fn get_system_prompt_with_tools(tools: &[ToolInfo]) -> String {
    let tools_section = format_tools_for_prompt(tools);
    PLANNING_SYSTEM_PROMPT.replace("{tools}", &tools_section)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_info_new() {
        let tool = ToolInfo::new("read_file", "Read contents of a file");
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description, "Read contents of a file");
    }

    #[test]
    fn test_tool_info_new_with_string() {
        let name = String::from("write_file");
        let desc = String::from("Write contents to a file");
        let tool = ToolInfo::new(name, desc);
        assert_eq!(tool.name, "write_file");
        assert_eq!(tool.description, "Write contents to a file");
    }

    #[test]
    fn test_format_tools_for_prompt_empty() {
        let tools: Vec<ToolInfo> = vec![];
        let result = format_tools_for_prompt(&tools);
        assert_eq!(result, "No tools available.");
    }

    #[test]
    fn test_format_tools_for_prompt_single() {
        let tools = vec![ToolInfo::new("test_tool", "A test tool for testing")];
        let result = format_tools_for_prompt(&tools);
        assert_eq!(result, "- **test_tool**: A test tool for testing");
    }

    #[test]
    fn test_format_tools_for_prompt_multiple() {
        let tools = vec![
            ToolInfo::new("read_file", "Read a file from the filesystem"),
            ToolInfo::new("write_file", "Write content to a file"),
            ToolInfo::new("delete_file", "Delete a file"),
        ];
        let result = format_tools_for_prompt(&tools);

        assert!(result.contains("- **read_file**: Read a file from the filesystem"));
        assert!(result.contains("- **write_file**: Write content to a file"));
        assert!(result.contains("- **delete_file**: Delete a file"));

        // Check proper line separation
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_build_planning_prompt() {
        let user_input = "Read the config.json file and summarize it";
        let result = build_planning_prompt(user_input, "");

        assert!(result.contains("User request: Read the config.json file and summarize it"));
        assert!(result.contains("Analyze this request"));
        assert!(result.contains("JSON execution plan"));
    }

    #[test]
    fn test_build_planning_prompt_empty_input() {
        let result = build_planning_prompt("", "");
        assert!(result.contains("User request: "));
        assert!(result.contains("Analyze this request"));
    }

    #[test]
    fn test_get_system_prompt_with_tools_empty() {
        let tools: Vec<ToolInfo> = vec![];
        let result = get_system_prompt_with_tools(&tools);

        // Should contain "No tools available."
        assert!(result.contains("No tools available."));
        // Should NOT contain the placeholder
        assert!(!result.contains("{tools}"));
        // Should still have the rest of the prompt
        assert!(result.contains("You are a task planning assistant"));
        assert!(result.contains("## Decision Rules"));
    }

    #[test]
    fn test_get_system_prompt_with_tools_multiple() {
        let tools = vec![
            ToolInfo::new("read_file", "Read a file"),
            ToolInfo::new("execute_command", "Execute a shell command"),
        ];
        let result = get_system_prompt_with_tools(&tools);

        // Should contain formatted tools
        assert!(result.contains("- **read_file**: Read a file"));
        assert!(result.contains("- **execute_command**: Execute a shell command"));
        // Should NOT contain the placeholder
        assert!(!result.contains("{tools}"));
        // Should have the full prompt structure
        assert!(result.contains("## Available Tools"));
        assert!(result.contains("## Output Format"));
        assert!(result.contains("## Task Types"));
    }

    #[test]
    fn test_planning_system_prompt_contains_all_sections() {
        // Verify the system prompt contains all expected sections
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Available Tools"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Output Format"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Decision Rules"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Task Types"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("## Important"));
    }

    #[test]
    fn test_planning_system_prompt_contains_task_types() {
        // Verify all task types are documented
        assert!(PLANNING_SYSTEM_PROMPT.contains("file_operation"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("code_execution"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("document_generation"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("app_automation"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("ai_inference"));
    }

    #[test]
    fn test_planning_system_prompt_contains_plan_types() {
        // Verify all plan types are documented
        assert!(PLANNING_SYSTEM_PROMPT.contains("conversational"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("single_action"));
        assert!(PLANNING_SYSTEM_PROMPT.contains("task_graph"));
    }

    #[test]
    fn test_planning_system_prompt_has_tools_placeholder() {
        // Verify the placeholder exists for tool injection
        assert!(PLANNING_SYSTEM_PROMPT.contains("{tools}"));
    }

    #[test]
    fn test_tool_info_clone() {
        let tool = ToolInfo::new("test", "description");
        let cloned = tool.clone();
        assert_eq!(tool.name, cloned.name);
        assert_eq!(tool.description, cloned.description);
    }

    #[test]
    fn test_tool_info_debug() {
        let tool = ToolInfo::new("test", "description");
        let debug_str = format!("{:?}", tool);
        assert!(debug_str.contains("ToolInfo"));
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("description"));
    }
}
