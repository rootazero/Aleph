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

use crate::builtin_tools::browser_tools::{
    BrowserClickTool, BrowserConsoleTool, BrowserEvaluateTool, BrowserFillFormTool,
    BrowserNavigateTool, BrowserOpenTool, BrowserPressKeyTool, BrowserProfileTool,
    BrowserScreenshotTool, BrowserSelectTool, BrowserSnapshotTool, BrowserTabsTool,
    BrowserTypeTool, BrowserWaitForTool,
};
use crate::builtin_tools::skill_reader::ListSkillsTool as SkillListTool;
use crate::builtin_tools::{
    BashExecTool, CodeExecTool, DesktopTool, FileEditTool, FileOpsTool, FileReadTool,
    FileWriteTool, ImageGenerateTool, PdfGenerateTool, ReadConfigGuideTool, SearchTool,
    SelfManageTool, VaultStoreTool, WebFetchTool,
};
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
        description: "File system operations - list, move, copy, delete, mkdir, search, batch_move, organize",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_read",
        description: "Read the contents of a file with optional offset/limit for partial reads",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_write",
        description: "Write content to a file (content is a required parameter)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "file_edit",
        description: "Perform exact string replacement in a file",
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
        name: "image_generate",
        description: "Generate images from text prompts",
        requires_config: true, // Requires generation registry
    },
    BuiltinToolDefinition {
        name: "skill_list",
        description: "List all installed skills",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_read",
        description: "Read the full instructions of an installed skill. Call this before executing any skill-based task.",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "gateway_route",
        description: "Query Aleph's routing engine to determine how a message would be routed. Returns the target agent, session key, and task classification.",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "desktop",
        description: "Control the desktop via platform-native capabilities: screenshots, OCR, keyboard/mouse, app launch, windows, and screen recording",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "read_config_guide",
        description: "Get Aleph configuration manual for self-management operations",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "self_manage",
        description: "Enter self-management mode when user wants to configure, modify, or fix Aleph",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "vault_store",
        description: "Manage encrypted secret vault (store/delete/list API keys)",
        requires_config: true, // Requires SharedTokenManager
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
        name: "memory_explore",
        description: "Explore related knowledge by following semantic connections from a starting query across multiple hops",
        requires_config: true, // Requires memory_db + embedder
    },
    BuiltinToolDefinition {
        name: "memory_timeline",
        description: "View the complete lifecycle of a memory fact — creation, modification, decay, invalidation timeline",
        requires_config: true, // Requires StateDatabase
    },
    BuiltinToolDefinition {
        name: "session_list",
        description: "List sessions accessible to this agent for cross-session communication",
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "session_send",
        description: "Send messages to other sessions (same or different agent)",
        requires_config: true, // Requires gateway_context
    },
    BuiltinToolDefinition {
        name: "session_new",
        description: "Start a new conversation session, closing the current one",
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_rename",
        description: "Rename the current session's topic/title",
        requires_config: true, // Requires SessionManager (via gateway_context)
    },
    BuiltinToolDefinition {
        name: "session_search",
        description: "Search past conversation transcripts across all sessions using full-text search",
        requires_config: true, // Requires SessionManager
    },
    BuiltinToolDefinition {
        name: "cron_manage",
        description: "Manage scheduled tasks — create, list, delete, enable/disable cron jobs",
        requires_config: true, // Requires SharedCronService
    },
    // Heartbeat management tools — require SharedHeartbeatService
    BuiltinToolDefinition {
        name: "heartbeat_list",
        description: "List all heartbeat monitoring tasks",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_create",
        description: "Create a new heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_update",
        description: "Update an existing heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_delete",
        description: "Delete a heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    BuiltinToolDefinition {
        name: "heartbeat_toggle",
        description: "Enable or disable a heartbeat monitoring task",
        requires_config: true, // Requires SharedHeartbeatService
    },
    // Heartbeat report tool — always available, used during L2 heartbeat execution
    BuiltinToolDefinition {
        name: "heartbeat_report",
        description: "Report results of a heartbeat monitoring analysis (silent or notify user)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "agent_create",
        description: "Create a new agent with an isolated workspace and register it for use",
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
        name: "browser_press_key",
        description: "Press a keyboard key in the browser",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_wait_for",
        description: "Wait for specific text to appear on the page",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_console",
        description: "Read browser console messages for debugging",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "browser_profile",
        description: "List and manage browser profiles",
        requires_config: false,
    },
    // Media tools — require MediaPipeline
    BuiltinToolDefinition {
        name: "media_understand",
        description: "Understand media content (images, audio, video, documents) with auto-detection and multi-provider fallback",
        requires_config: true, // Requires media_pipeline
    },
    BuiltinToolDefinition {
        name: "audio_transcribe",
        description: "Transcribe audio files to text with language detection",
        requires_config: true, // Requires media_pipeline
    },
    BuiltinToolDefinition {
        name: "document_extract",
        description: "Extract text and structured data from documents",
        requires_config: true, // Requires media_pipeline
    },
    BuiltinToolDefinition {
        name: "clawhub",
        description: "Search, browse, install, and update skills from ClawHub registry",
        requires_config: false,
    },
    // Team management tools — require TeamStore
    BuiltinToolDefinition {
        name: "team_create",
        description: "Create a new team with the calling agent as leader, enrolling existing or inline-created members",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_delegate",
        description: "Delegate a task to a team member, execute it, and return the result",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_status",
        description: "Query the current state of a team, including members and task history",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_disband",
        description: "Mark a team as disbanded (preserved for history, cannot be undone)",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_member_remove",
        description: "Remove a member from a team (leader only, cannot remove self)",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "team_digest",
        description: "Generate a summary of recent team activity for the specified time period",
        requires_config: true,
    },
    // Team messaging tools — require MessageRouter / Inbox
    BuiltinToolDefinition {
        name: "message_send",
        description: "Send a message to team members with to/cc routing",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "inbox_read",
        description: "Read inbox messages or a full thread. Use mode='inbox' (default) to read your messages, mode='thread' with thread_id to read a conversation thread.",
        requires_config: true,
    },
    // Task coordination tools — require CoordTaskStore
    BuiltinToolDefinition {
        name: "task_create",
        description: "Create a coordination task with optional dependencies and team assignment",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_update",
        description: "Update a coordination task's status, owner, result, or metadata",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_list",
        description: "List coordination tasks with optional filtering by team, status, or owner",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_wait",
        description: "Wait for specific tasks or all team tasks to complete",
        requires_config: true,
    },
    // Task artifact tools — require ArtifactStore
    BuiltinToolDefinition {
        name: "task_submit",
        description: "Submit a structured artifact as task output",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "task_read_artifact",
        description: "Read artifacts submitted for a task",
        requires_config: true,
    },
    // Collaborative session tools — require SessionCoordinator / SessionStore
    BuiltinToolDefinition {
        name: "session_collaborate",
        description: "Start a collaborative session between team members for real-time multi-turn discussion",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_turn",
        description: "Respond in a collaborative session or propose its conclusion",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "session_read",
        description: "Read a collaborative session's transcript, status, and outcome",
        requires_config: true,
    },
    // Channel management tools — require ChannelRegistry
    BuiltinToolDefinition {
        name: "channel_pairing",
        description: "Manage channel pairing codes — generate new codes or list active ones for Telegram/other channels",
        requires_config: true, // Requires ChannelRegistry (deferred injection)
    },
    // Media send tool — no dependencies, just passes URLs through to ReplyEmitter
    BuiltinToolDefinition {
        name: "media_send",
        description: "Send media files (images, videos, audio) directly to the user in the chat",
        requires_config: false,
    },
    // voice_mode_set is a LLM tool only — NOT a slash command.
    // Use /voice on|off instead. Excluded from BUILTIN_TOOL_DEFINITIONS
    // to avoid appearing in command lists.

    // Skill management tools — LLM-callable tools for querying and configuring skills
    BuiltinToolDefinition {
        name: "skill_status",
        description: "Query skill system status — list all skills with readiness, missing deps, and install options",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_install",
        description: "Install missing dependencies for a skill",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "skill_manage",
        description: "Toggle or configure a skill (enable/disable, change prompt scope)",
        requires_config: false,
    },
    BuiltinToolDefinition {
        name: "note_manage",
        description: "Create, update, append, query, list, or delete personal knowledge notes across all categories",
        requires_config: true,
    },
    // ACP delegate tool — unified delegation to any external CLI agent.
    // Requires AcpHarnessManager; execution returns clear error if harness unavailable.
    BuiltinToolDefinition {
        name: "acp_delegate",
        description: "Delegate a task to an external CLI agent via ACP. Use 'claude-code', 'codex', or 'gemini' as the harness parameter, or any custom harness registered via acp.create.",
        requires_config: true,
    },
    BuiltinToolDefinition {
        name: "acp_switch",
        description: "Switch to direct conversation with an external CLI agent (Claude Code, Codex, or Gemini), or switch back to Aleph.",
        requires_config: true,
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
        "file_read" => Some(Box::new(FileReadTool::new())),
        "file_write" => Some(Box::new(FileWriteTool::new())),
        "file_edit" => Some(Box::new(FileEditTool::new())),
        "bash" => Some(Box::new(BashExecTool::new())),
        "code_exec" => Some(Box::new(CodeExecTool::new())),
        "pdf_generate" => Some(Box::new(PdfGenerateTool::new())),
        "image_generate" => {
            if let Some(cfg) = config {
                if let Some(ref registry) = cfg.generation_registry {
                    return Some(Box::new(ImageGenerateTool::new(Arc::clone(registry))));
                }
            }
            None // Requires generation registry
        }
        "skill_list" => Some(Box::new(SkillListTool::default())),
        "read_config_guide" => Some(Box::new(ReadConfigGuideTool::default())),
        "self_manage" => Some(Box::new(SelfManageTool::default())),
        "desktop" => Some(Box::new(DesktopTool::new())),
        "vault_store" => config
            .and_then(|c| c.shared_token_manager.as_ref())
            .map(|mgr| Box::new(VaultStoreTool::new(Arc::clone(mgr))) as Box<dyn AlephToolDyn>),
        // Sessions tools require gateway_context and caller_agent_id at runtime,
        // so they cannot be created via create_tool_boxed. They are created
        // dynamically in BuiltinToolRegistry::execute_tool().
        "session_list" | "session_send" => None,
        // Session new tool requires SessionManager (from gateway_context) at runtime
        "session_new" => None,
        // Session set-topic tool requires SessionManager (from gateway_context) at runtime
        "session_rename" => None,
        // Session search tool requires SessionManager at runtime
        "session_search" => None,
        // Cron management tool requires SharedCronService at runtime
        "cron_manage" => None,
        // Heartbeat management tools require SharedHeartbeatService at runtime
        "heartbeat_list" | "heartbeat_create" | "heartbeat_update" | "heartbeat_delete"
        | "heartbeat_toggle" => None,
        // Heartbeat report tool — always available (no dependencies)
        "heartbeat_report" => Some(Box::new(
            crate::builtin_tools::heartbeat_manage::HeartbeatReportTool,
        )),
        // Agent management tools require agent_registry + workspace_manager + session_context,
        // created dynamically in BuiltinToolRegistry::with_config().
        "agent_create" | "agent_list" | "agent_delete" => None,
        // Media tools — require MediaPipeline
        "media_understand" => config
            .and_then(|c| c.media_pipeline.as_ref())
            .map(|pipeline| {
                Box::new(crate::builtin_tools::media_tools::MediaUnderstandTool::new(
                    Arc::clone(pipeline),
                )) as Box<dyn AlephToolDyn>
            }),
        "audio_transcribe" => config
            .and_then(|c| c.media_pipeline.as_ref())
            .map(|pipeline| {
                Box::new(crate::builtin_tools::media_tools::AudioTranscribeTool::new(
                    Arc::clone(pipeline),
                )) as Box<dyn AlephToolDyn>
            }),
        "document_extract" => config
            .and_then(|c| c.media_pipeline.as_ref())
            .map(|pipeline| {
                Box::new(crate::builtin_tools::media_tools::DocumentExtractTool::new(
                    Arc::clone(pipeline),
                )) as Box<dyn AlephToolDyn>
            }),
        "clawhub" => Some(Box::new(crate::builtin_tools::clawhub::ClawHubTool::new())),
        "media_send" => Some(Box::new(
            crate::builtin_tools::media_send::MediaSendTool::new(),
        )),
        // Team management tools require TeamStore at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "team_create" | "team_delegate" | "team_status" | "team_disband" | "team_member_remove"
        | "team_digest" | "message_send" | "inbox_read" => None,
        // Task coordination tools require CoordTaskStore + AgentMessageBus at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "task_create" | "task_update" | "task_list" | "task_wait" => None,
        // Task artifact tools require ArtifactStore + current_agent_id at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "task_submit" | "task_read_artifact" => None,
        // Session collaboration tools require SessionCoordinator / SessionStore at runtime,
        // created dynamically in BuiltinToolRegistry::with_config().
        "session_collaborate" | "session_turn" | "session_read" => None,
        // Browser tools — create ProfileManager from config or use default
        "browser_open" | "browser_click" | "browser_type" | "browser_screenshot"
        | "browser_snapshot" | "browser_navigate" | "browser_tabs" | "browser_select"
        | "browser_evaluate" | "browser_fill_form" | "browser_press_key" | "browser_wait_for"
        | "browser_console" | "browser_profile" => {
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
                "browser_press_key" => Some(Box::new(BrowserPressKeyTool::new(manager))),
                "browser_wait_for" => Some(Box::new(BrowserWaitForTool::new(manager))),
                "browser_console" => Some(Box::new(BrowserConsoleTool::new(manager))),
                "browser_profile" => Some(Box::new(BrowserProfileTool::new(manager))),
                _ => None,
            }
        }
        // Skill management tools — always available
        "skill_status" => Some(Box::new(
            crate::builtin_tools::skill_status::SkillStatusTool::new(
                crate::skill::SkillSystem::new(),
            ),
        )),
        "skill_install" => Some(Box::new(
            crate::builtin_tools::skill_install::SkillInstallTool::new(
                crate::skill::SkillSystem::new(),
            ),
        )),
        "skill_manage" => Some(Box::new(
            crate::builtin_tools::skill_manage::SkillManageTool::new(
                crate::skill::SkillSystem::new(),
            ),
        )),
        // note_manage requires memory backend — cannot create standalone fallback
        "note_manage" => None,
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
#[allow(dead_code)]
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
        assert!(names.contains(&"image_generate".to_string()));
        assert!(names.contains(&"skill_list".to_string()));
        assert!(names.contains(&"read_config_guide".to_string()));
        assert!(names.contains(&"vault_store".to_string()));

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
        assert!(names.contains(&"browser_press_key".to_string()));
        assert!(names.contains(&"browser_wait_for".to_string()));
        assert!(names.contains(&"browser_console".to_string()));
        assert!(names.contains(&"browser_profile".to_string()));
    }

    #[test]
    fn test_sessions_tools_defined() {
        let names = get_builtin_tool_names();

        // Verify sessions tools are defined when gateway feature is enabled
        assert!(names.contains(&"session_list".to_string()));
        assert!(names.contains(&"session_send".to_string()));
    }

    #[test]
    fn test_sessions_tools_require_config() {
        // Sessions tools require gateway_context (dynamic creation)
        assert!(create_tool_boxed("session_list", None).is_none());
        assert!(create_tool_boxed("session_send", None).is_none());
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
        assert!(is_builtin_tool("session_list"));
        assert!(is_builtin_tool("session_send"));
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
        assert!(create_tool_boxed("image_generate", None).is_none());

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
        assert!(create_tool_boxed("browser_press_key", None).is_some());
        assert!(create_tool_boxed("browser_wait_for", None).is_some());
        assert!(create_tool_boxed("browser_console", None).is_some());
        assert!(create_tool_boxed("browser_profile", None).is_some());
    }

    #[test]
    fn test_tool_definitions_consistency() {
        // Verify all definitions have non-empty names and descriptions
        for def in BUILTIN_TOOL_DEFINITIONS {
            assert!(!def.name.is_empty(), "Tool name cannot be empty");
            assert!(
                !def.description.is_empty(),
                "Tool description cannot be empty"
            );
        }
    }
}
