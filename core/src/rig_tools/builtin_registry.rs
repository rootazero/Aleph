//! Unified builtin tool registry - Single Source of Truth
//!
//! This module defines ALL builtin tools in one place, ensuring consistency
//! between AetherToolServer (hot-reload/management) and BuiltinToolRegistry (execution).
//!
//! # Architecture
//!
//! Before this module:
//! - AetherToolServer defined tools in agents/rig/tools.rs
//! - BuiltinToolRegistry defined tools in executor/builtin_registry/registry.rs
//! - Tools could get out of sync (e.g., bash tool missing from registry)
//!
//! After this module:
//! - Single definition of all builtin tools
//! - Both systems source from this module
//! - Guaranteed consistency

use std::sync::{Arc, RwLock};

use crate::generation::GenerationProviderRegistry;
use crate::tools::AetherToolDyn; // Use AetherToolDyn instead of AetherTool

use super::{
    BashExecTool, CodeExecTool, FileOpsTool, ImageGenerateTool, PdfGenerateTool,
    ReadSkillTool, SearchTool, WebFetchTool, YouTubeTool,
};
use super::skill_reader::ListSkillsTool as SkillListTool;

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

/// All builtin tools in the system
///
/// This is the Single Source of Truth for builtin tools.
/// Both AetherToolServer and BuiltinToolRegistry use this list.
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
];

/// Configuration for creating builtin tools
#[derive(Clone, Default)]
pub struct BuiltinToolsConfig {
    /// Tavily API key for search tool (optional)
    pub tavily_api_key: Option<String>,
    /// Generation provider registry for image/video/audio generation (optional)
    pub generation_registry: Option<Arc<RwLock<GenerationProviderRegistry>>>,
}

/// Create a boxed tool instance by name
///
/// This function is used by AetherToolServer to create tool instances
/// for hot-reload and management.
///
/// # Arguments
/// * `name` - Tool name (must match BUILTIN_TOOL_DEFINITIONS)
/// * `config` - Optional configuration for tools that need it
///
/// # Returns
/// * `Some(tool)` - Boxed tool instance if the tool exists
/// * `None` - If the tool name is unknown
pub fn create_tool_boxed(
    name: &str,
    config: Option<&BuiltinToolsConfig>,
) -> Option<Box<dyn AetherToolDyn>> {
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
        _ => None,
    }
}

/// Create typed tool instances for BuiltinToolRegistry
///
/// This struct holds actual tool instances (not boxed) for direct invocation.
/// Used by BuiltinToolRegistry for high-performance execution.
pub struct TypedBuiltinTools {
    pub search_tool: SearchTool,
    pub web_fetch_tool: WebFetchTool,
    pub youtube_tool: YouTubeTool,
    pub file_ops_tool: FileOpsTool,
    pub bash_tool: BashExecTool,
    pub code_exec_tool: CodeExecTool,
    pub pdf_generate_tool: PdfGenerateTool,
    pub image_generate_tool: Option<ImageGenerateTool>,
    pub read_skill_tool: ReadSkillTool,
    pub list_skills_tool: SkillListTool,
}

/// Create typed tool instances for BuiltinToolRegistry
///
/// This function creates concrete typed instances for direct invocation,
/// avoiding the overhead of dynamic dispatch.
pub fn create_typed_tools(config: Option<&BuiltinToolsConfig>) -> TypedBuiltinTools {
    let search_tool = if let Some(cfg) = config {
        SearchTool::with_api_key(cfg.tavily_api_key.clone())
    } else {
        SearchTool::new()
    };

    let image_generate_tool = config
        .and_then(|cfg| cfg.generation_registry.as_ref())
        .map(|registry| ImageGenerateTool::new(Arc::clone(registry)));

    TypedBuiltinTools {
        search_tool,
        web_fetch_tool: WebFetchTool::new(),
        youtube_tool: YouTubeTool::new(),
        file_ops_tool: FileOpsTool::new(),
        bash_tool: BashExecTool::new(),
        code_exec_tool: CodeExecTool::new(),
        pdf_generate_tool: PdfGenerateTool::new(),
        image_generate_tool,
        read_skill_tool: ReadSkillTool::default(),
        list_skills_tool: SkillListTool::default(),
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

    #[test]
    fn test_is_builtin_tool() {
        assert!(is_builtin_tool("bash"));
        assert!(is_builtin_tool("code_exec"));
        assert!(is_builtin_tool("file_ops"));
        assert!(!is_builtin_tool("unknown_tool"));
        assert!(!is_builtin_tool("mcp:filesystem"));
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
    fn test_create_typed_tools() {
        let tools = create_typed_tools(None);

        // Verify tools are created
        assert_eq!(BashExecTool::NAME, "bash");
        assert_eq!(CodeExecTool::NAME, "code_exec");
        assert_eq!(FileOpsTool::NAME, "file_ops");

        // Image tool should be None without generation registry
        assert!(tools.image_generate_tool.is_none());
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
