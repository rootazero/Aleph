//! Builtin tool definitions - Single Source of Truth
//!
//! This module defines ALL builtin tools in one place, ensuring consistency
//! across the system.
//!
//! # Architecture
//!
//! This is the authoritative source for builtin tool definitions.
//! Both BuiltinToolRegistry (Agent Loop execution) and AlephToolServer (tool management)
//! source their tool definitions from this module.
//!
//! # Usage
//!
//! - `BUILTIN_TOOL_DEFINITIONS` - List of all tool definitions
//! - `create_tool_boxed()` - Create boxed tool instance for AlephToolServer
//! - `get_builtin_tool_names()` - Get list of all tool names
//! - `is_builtin_tool()` - Check if a name is a builtin tool

use crate::sync_primitives::Arc;

use crate::builtin_tools::{
    BashExecTool, CodeExecTool, DesktopTool, FileOpsTool, ImageGenerateTool, PdfGenerateTool,
    ReadSkillTool, SearchTool, WebFetchTool, YouTubeTool,
};
use crate::builtin_tools::skill_reader::ListSkillsTool as SkillListTool;
use crate::tools::AlephToolDyn;

use super::BuiltinToolConfig;

/// Definition of a builtin tool
///
/// This struct describes how to create and identify a builtin tool.
#[derive(Clone)]
pub struct BuiltinToolDefinition {
    /// Tool name (e.g., "search", "bash", "file_ops")
    pub name: &'static str,
    /// Tool description for AI prompts
    pub description: &'static str,
    /// Whether this tool requires special configuration
    pub requires_config: bool,
}

/// All builtin tools in the system - Single Source of Truth
///
/// This is the authoritative list of all builtin tools.
/// Both BuiltinToolRegistry and AlephToolServer use this list.
pub const BUILTIN_TOOL_DEFINITIONS: &[BuiltinToolDefinition] = &[
    BuiltinToolDefinition {
        name: "search",
        description: "Search the internet using Tavily API",
        requires_config: false, // Optional API key
    },
    BuiltinToolDefinition {
        name: "web_fetch",
        description: "Fetch and read content from a URL",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "youtube",
        description: "Extract YouTube video transcripts",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_ops",
        description: "File system operations - list, read, write, move, copy, delete, etc.",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "bash",
        description: "Execute bash/shell commands (convenience wrapper for code_exec with shell)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "code_exec",
        description: "Execute code in various programming languages (Python, JavaScript, Shell)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "pdf_generate",
        description: "Generate PDF documents from text/Markdown",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "generate_image",
        description: "Generate images from text prompts",
        requires_config: true, // Requires generation registry
    },
    BuiltinToolDefinition {
        name: "read_skill",
        description: "Read skill file content for Progressive Disclosure pattern",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "list_skills",
        description: "List all installed skills",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop",
        description: "Control the macOS desktop: screenshots, OCR, UI automation, keyboard/mouse, app launch, canvas overlays",
        requires_config: false,
    },
    #[cfg(feature = "gateway")]
    BuiltinToolDefinition {
        name: "sessions_list",
        description: "List sessions accessible to this agent for cross-session communication",
        requires_config: true, // Requires gateway_context
    },
    #[cfg(feature = "gateway")]
    BuiltinToolDefinition {
        name: "sessions_send",
        description: "Send messages to other sessions (same or different agent)",
        requires_config: true, // Requires gateway_context
    },
];

/// Create a boxed tool instance by name
///
/// This function is used by AlephToolServer to create tool instances
/// for tool management and hot-reload capabilities.
///
/// # Arguments
/// * `name` - Tool name (must match BUILTIN_TOOL_DEFINITIONS)
/// * `config` - Optional configuration for tools that need it
///
/// # Returns
/// * `Some(tool)` - Boxed tool instance if the tool exists
/// * `None` - If the tool name is unknown or requires missing config
pub fn create_tool_boxed(
    name: &str,
    config: Option<&BuiltinToolConfig>,
) -> Option<Box<dyn AlephToolDyn>> {
    match name {
        "search" => {
            let tool = if let Some(cfg) = config {
                SearchTool::with_api_key(cfg.tavily_api_key.clone())
            } else {
                SearchTool::new()
            };
            Some(Box::new(tool))
        }
        "web_fetch" => Some(Box::new(WebFetchTool::new())),
        "youtube" => Some(Box::new(YouTubeTool::new())),
        "file_ops" => Some(Box::new(FileOpsTool::new())),
        "bash" => Some(Box::new(BashExecTool::new())),
        "code_exec" => Some(Box::new(CodeExecTool::new())),
        "pdf_generate" => Some(Box::new(PdfGenerateTool::new())),
        "generate_image" => {
            if let Some(cfg) = config {
                if let Some(ref registry) = cfg.generation_registry {
                    return Some(Box::new(ImageGenerateTool::new(Arc::clone(registry))));
                }
            }
            None // Requires generation registry
        }
        "read_skill" => Some(Box::new(ReadSkillTool::default())),
        "list_skills" => Some(Box::new(SkillListTool::default())),
        "desktop" => Some(Box::new(DesktopTool::new())),
        // Sessions tools require gateway_context and caller_agent_id at runtime,
        // so they cannot be created via create_tool_boxed. They are created
        // dynamically in BuiltinToolRegistry::execute_tool().
        #[cfg(feature = "gateway")]
        "sessions_list" | "sessions_send" => None,
        _ => None,
    }
}

/// Get list of all builtin tool names
///
/// This is used for initialization and display purposes.
pub fn get_builtin_tool_names() -> Vec<String> {
    BUILTIN_TOOL_DEFINITIONS
        .iter()
        .map(|def| def.name.to_string())
        .collect()
}

/// Check if a tool name is a builtin tool
pub fn is_builtin_tool(name: &str) -> bool {
    BUILTIN_TOOL_DEFINITIONS.iter().any(|def| def.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_tools_defined() {
        let names = get_builtin_tool_names();

        // Verify core tools
        assert!(names.contains(&"search".to_string()));
        assert!(names.contains(&"web_fetch".to_string()));
        assert!(names.contains(&"youtube".to_string()));
        assert!(names.contains(&"file_ops".to_string()));
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"code_exec".to_string()));
        assert!(names.contains(&"pdf_generate".to_string()));
        assert!(names.contains(&"generate_image".to_string()));
        assert!(names.contains(&"read_skill".to_string()));
        assert!(names.contains(&"list_skills".to_string()));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn test_sessions_tools_defined() {
        let names = get_builtin_tool_names();

        // Verify sessions tools are defined when gateway feature is enabled
        assert!(names.contains(&"sessions_list".to_string()));
        assert!(names.contains(&"sessions_send".to_string()));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn test_sessions_tools_require_config() {
        // Sessions tools require gateway_context (dynamic creation)
        assert!(create_tool_boxed("sessions_list", None).is_none());
        assert!(create_tool_boxed("sessions_send", None).is_none());
    }

    #[test]
    fn test_is_builtin_tool() {
        assert!(is_builtin_tool("bash"));
        assert!(is_builtin_tool("code_exec"));
        assert!(is_builtin_tool("file_ops"));
        assert!(!is_builtin_tool("unknown_tool"));
        assert!(!is_builtin_tool("mcp:filesystem"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn test_is_builtin_tool_sessions() {
        assert!(is_builtin_tool("sessions_list"));
        assert!(is_builtin_tool("sessions_send"));
    }

    #[test]
    fn test_create_tool_boxed() {
        // Test creating basic tools without config
        assert!(create_tool_boxed("bash", None).is_some());
        assert!(create_tool_boxed("code_exec", None).is_some());
        assert!(create_tool_boxed("file_ops", None).is_some());

        // Test unknown tool
        assert!(create_tool_boxed("unknown", None).is_none());

        // Test tool requiring config (should return None without config)
        assert!(create_tool_boxed("generate_image", None).is_none());
    }

    #[test]
    fn test_tool_definitions_consistency() {
        // Verify all definitions have non-empty names and descriptions
        for def in BUILTIN_TOOL_DEFINITIONS {
            assert!(!def.name.is_empty(), "Tool name cannot be empty");
            assert!(!def.description.is_empty(), "Tool description cannot be empty");
        }
    }
}
