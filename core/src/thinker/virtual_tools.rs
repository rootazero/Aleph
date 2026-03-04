//! Virtual tools for native tool_use mode
//!
//! These are not real tools -- they are decision signals registered as tool definitions
//! so the LLM can express Complete/AskUser/Fail decisions through the native tool_use API.
//!
//! In native tool_use mode, ALL LLM outputs must go through tool calls. These virtual
//! tools allow the LLM to signal non-tool decisions (task completion, asking for
//! clarification, reporting failure) using the same mechanism.

use crate::dispatcher::{ToolCategory, ToolDefinition};
use serde_json::json;

/// Prefix for virtual tools -- double underscore to avoid collision with real tools
pub const VIRTUAL_COMPLETE: &str = "__complete";
pub const VIRTUAL_ASK_USER: &str = "__ask_user";
pub const VIRTUAL_FAIL: &str = "__fail";

/// Generate virtual tool definitions for native tool_use mode
///
/// Returns three virtual tools:
/// - `__complete`: Signal task completion with a summary
/// - `__ask_user`: Ask the user a question for clarification
/// - `__fail`: Report an unrecoverable failure
pub fn virtual_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            VIRTUAL_COMPLETE,
            "Report that the task is complete. Call this when you have finished the user's request and want to provide a final summary.",
            json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "A concise summary of what was accomplished and the final result"
                    }
                },
                "required": ["summary"]
            }),
            ToolCategory::Builtin,
        ),
        ToolDefinition::new(
            VIRTUAL_ASK_USER,
            "Ask the user a question when you need clarification or input before proceeding.",
            json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question to ask the user"
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of suggested answer choices"
                    }
                },
                "required": ["question"]
            }),
            ToolCategory::Builtin,
        ),
        ToolDefinition::new(
            VIRTUAL_FAIL,
            "Report that the task cannot be completed. Call this when you encounter an unrecoverable error.",
            json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "description": "Explanation of why the task failed"
                    }
                },
                "required": ["reason"]
            }),
            ToolCategory::Builtin,
        ),
    ]
}

/// Check if a tool name is a virtual tool
pub fn is_virtual_tool(name: &str) -> bool {
    matches!(name, VIRTUAL_COMPLETE | VIRTUAL_ASK_USER | VIRTUAL_FAIL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_tool_definitions() {
        let defs = virtual_tool_definitions();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].name, VIRTUAL_COMPLETE);
        assert_eq!(defs[1].name, VIRTUAL_ASK_USER);
        assert_eq!(defs[2].name, VIRTUAL_FAIL);
    }

    #[test]
    fn test_is_virtual_tool() {
        assert!(is_virtual_tool(VIRTUAL_COMPLETE));
        assert!(is_virtual_tool(VIRTUAL_ASK_USER));
        assert!(is_virtual_tool(VIRTUAL_FAIL));
        assert!(!is_virtual_tool("search"));
        assert!(!is_virtual_tool("pdf_generate"));
        assert!(!is_virtual_tool("complete")); // without prefix
    }

    #[test]
    fn test_virtual_tools_have_valid_schemas() {
        for def in virtual_tool_definitions() {
            assert!(
                def.parameters.is_object(),
                "Parameters should be an object for {}",
                def.name
            );
            let props = &def.parameters["properties"];
            assert!(
                props.is_object(),
                "Properties should be an object for {}",
                def.name
            );
            let required = &def.parameters["required"];
            assert!(
                required.is_array(),
                "Required should be an array for {}",
                def.name
            );
        }
    }

    #[test]
    fn test_virtual_tools_start_with_double_underscore() {
        for def in virtual_tool_definitions() {
            assert!(
                def.name.starts_with("__"),
                "Virtual tool '{}' should start with __",
                def.name
            );
        }
    }
}
