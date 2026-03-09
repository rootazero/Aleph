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
    BashExecTool, CodeExecTool, ConfigReadTool, ConfigUpdateTool, DesktopTool, EscalateTaskTool,
    FileOpsTool, ImageGenerateTool, PdfGenerateTool, ReadSkillTool, SearchTool, WebFetchTool,
};
use crate::builtin_tools::browser_tools::{
    BrowserOpenTool, BrowserClickTool, BrowserTypeTool, BrowserScreenshotTool,
    BrowserSnapshotTool, BrowserNavigateTool, BrowserTabsTool, BrowserSelectTool,
    BrowserEvaluateTool, BrowserFillFormTool, BrowserProfileTool,
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
    BuiltinToolDefinition {
        name: "config_read",
        description: "Read current Aleph configuration with sensitive fields masked",
        requires_config: true, // Requires config Arc<RwLock<Config>>
    },
    BuiltinToolDefinition {
        name: "config_update",
        description: "Update Aleph configuration with schema validation and secret vault integration",
        requires_config: true, // Requires ConfigPatcher
    },
    BuiltinToolDefinition {
        name: "memory_search",
        description: "Search personal memory for relevant facts and conversation history with workspace-scoped retrieval",
        requires_config: true, // Requires memory_db + embedder
    },
    BuiltinToolDefinition {
        name: "memory_browse",
        description: "Browse personal memory via hierarchical VFS navigation (ls, read, glob on aleph:// paths)",
        requires_config: true, // Requires memory_db
    },
    BuiltinToolDefinition {
        name: "sessions_list",
        description: "List sessions accessible to this agent for cross-session communication",
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "sessions_send",
        description: "Send messages to other sessions (same or different agent)",
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "escalate_task",
        description: "Request escalation to a more capable execution strategy",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "agent_create",
        description: "Create a new agent with an isolated workspace and register it for use",
        requires_config: true, // Requires agent_registry + workspace_manager
    },
    BuiltinToolDefinition {
        name: "agent_switch",
        description: "Switch the active agent for the current conversation",
        requires_config: true, // Requires agent_registry + workspace_manager
    },
    BuiltinToolDefinition {
        name: "agent_list",
        description: "List all registered agents and show which is active for the current session",
        requires_config: true, // Requires agent_registry
    },
    BuiltinToolDefinition {
        name: "agent_delete",
        description: "Delete an agent and archive its workspace (cannot delete 'main')",
        requires_config: true, // Requires agent_registry + workspace_manager
    },
    BuiltinToolDefinition {
        name: "subagent_spawn",
        description: "Spawn a sub-agent to handle a task autonomously and return the result",
        requires_config: true, // Requires sub_agent_dispatcher
    },
    BuiltinToolDefinition {
        name: "subagent_steer",
        description: "Send additional instructions to a running sub-agent",
        requires_config: true, // Requires sub_agent_dispatcher + sub_agent_registry
    },
    BuiltinToolDefinition {
        name: "subagent_kill",
        description: "Terminate a running sub-agent",
        requires_config: true, // Requires sub_agent_dispatcher + sub_agent_registry
    },
    // Browser tools — always available, share a ProfileManager
    BuiltinToolDefinition {
        name: "browser_open",
        description: "Open URL in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_click",
        description: "Click element in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_type",
        description: "Type text in browser element",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_screenshot",
        description: "Capture browser screenshot",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_snapshot",
        description: "Get browser ARIA accessibility tree",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_navigate",
        description: "Navigate browser back/forward/refresh",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_tabs",
        description: "List, switch, or close browser tabs",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_select",
        description: "Select dropdown option in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_evaluate",
        description: "Execute JavaScript in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_fill_form",
        description: "Fill multiple form fields in browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_profile",
        description: "List and manage browser profiles",
        requires_config: false,
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
        // Config tools require runtime context (config handle / patcher)
        "config_read" => {
            if let Some(cfg) = config {
                if let Some(ref config_handle) = cfg.config {
                    return Some(Box::new(ConfigReadTool::new(Arc::clone(config_handle))));
                }
            }
            None
        }
        "config_update" => {
            if let Some(cfg) = config {
                if let Some(ref patcher) = cfg.config_patcher {
                    return Some(Box::new(ConfigUpdateTool::new(Arc::clone(patcher))));
                }
            }
            None
        }
        // Sessions tools require gateway_context and caller_agent_id at runtime,
        // so they cannot be created via create_tool_boxed. They are created
        // dynamically in BuiltinToolRegistry::execute_tool().
        "sessions_list" | "sessions_send" => None,
        // Agent management tools require agent_registry + workspace_manager + session_context,
        // created dynamically in BuiltinToolRegistry::with_config().
        "agent_create" | "agent_switch" | "agent_list" | "agent_delete" => None,
        // Subagent management tools require sub_agent_dispatcher (+ registry for steer/kill),
        // created dynamically in BuiltinToolRegistry::with_config().
        "subagent_spawn" | "subagent_steer" | "subagent_kill" => None,
        "escalate_task" => Some(Box::new(EscalateTaskTool)),
        // Browser tools — create ProfileManager from config or use default
        "browser_open" | "browser_click" | "browser_type" | "browser_screenshot"
        | "browser_snapshot" | "browser_navigate" | "browser_tabs" | "browser_select"
        | "browser_evaluate" | "browser_fill_form" | "browser_profile" => {
            let manager = config
                .and_then(|cfg| cfg.browser_profile_manager.clone())
                .unwrap_or_else(|| {
                    Arc::new(crate::browser::manager::ProfileManager::new(
                        crate::browser::profile::BrowserSystemConfig::default(),
                    ))
                });
            match name {
                "browser_open" => Some(Box::new(BrowserOpenTool::new(manager))),
                "browser_click" => Some(Box::new(BrowserClickTool::new(manager))),
                "browser_type" => Some(Box::new(BrowserTypeTool::new(manager))),
                "browser_screenshot" => Some(Box::new(BrowserScreenshotTool::new(manager))),
                "browser_snapshot" => Some(Box::new(BrowserSnapshotTool::new(manager))),
                "browser_navigate" => Some(Box::new(BrowserNavigateTool::new(manager))),
                "browser_tabs" => Some(Box::new(BrowserTabsTool::new(manager))),
                "browser_select" => Some(Box::new(BrowserSelectTool::new(manager))),
                "browser_evaluate" => Some(Box::new(BrowserEvaluateTool::new(manager))),
                "browser_fill_form" => Some(Box::new(BrowserFillFormTool::new(manager))),
                "browser_profile" => Some(Box::new(BrowserProfileTool::new(manager))),
                _ => None,
            }
        }
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
        assert!(names.contains(&"file_ops".to_string()));
        assert!(names.contains(&"bash".to_string()));
        assert!(names.contains(&"code_exec".to_string()));
        assert!(names.contains(&"pdf_generate".to_string()));
        assert!(names.contains(&"generate_image".to_string()));
        assert!(names.contains(&"read_skill".to_string()));
        assert!(names.contains(&"list_skills".to_string()));

        // Verify browser tools
        assert!(names.contains(&"browser_open".to_string()));
        assert!(names.contains(&"browser_click".to_string()));
        assert!(names.contains(&"browser_type".to_string()));
        assert!(names.contains(&"browser_screenshot".to_string()));
        assert!(names.contains(&"browser_snapshot".to_string()));
        assert!(names.contains(&"browser_navigate".to_string()));
        assert!(names.contains(&"browser_tabs".to_string()));
        assert!(names.contains(&"browser_select".to_string()));
        assert!(names.contains(&"browser_evaluate".to_string()));
        assert!(names.contains(&"browser_fill_form".to_string()));
        assert!(names.contains(&"browser_profile".to_string()));
    }

    #[test]
    fn test_sessions_tools_defined() {
        let names = get_builtin_tool_names();

        // Verify sessions tools are defined when gateway feature is enabled
        assert!(names.contains(&"sessions_list".to_string()));
        assert!(names.contains(&"sessions_send".to_string()));
    }

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

        // Test browser tools (always available, no config required)
        assert!(create_tool_boxed("browser_open", None).is_some());
        assert!(create_tool_boxed("browser_click", None).is_some());
        assert!(create_tool_boxed("browser_type", None).is_some());
        assert!(create_tool_boxed("browser_screenshot", None).is_some());
        assert!(create_tool_boxed("browser_snapshot", None).is_some());
        assert!(create_tool_boxed("browser_navigate", None).is_some());
        assert!(create_tool_boxed("browser_tabs", None).is_some());
        assert!(create_tool_boxed("browser_select", None).is_some());
        assert!(create_tool_boxed("browser_evaluate", None).is_some());
        assert!(create_tool_boxed("browser_fill_form", None).is_some());
        assert!(create_tool_boxed("browser_profile", None).is_some());
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
