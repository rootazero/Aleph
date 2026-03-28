//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.
//!
//! # Built-in Tools
//!
//! - [`SearchTool`] - Web search via SearXNG
//! - [`WebFetchTool`] - Web page fetching
//! - [`FileOpsTool`] - File system operations (list, read, write, move, copy, delete, mkdir, search)
//! - [`AtomicOpsTool`] - Atomic operations (search, replace, move) powered by Atomic Engine
//! - [`CodeExecTool`] - Code execution (Python, JavaScript, Shell)
//! - [`PdfGenerateTool`] - PDF generation from text/Markdown
//! - [`ImageGenerateTool`] - Image generation from text prompts
//! - [`SpeechGenerateTool`] - Text-to-speech generation
//!
//! # Meta Tools (Smart Tool Discovery)
//!
//! - [`ListToolsTool`] - List available tools by category
//! - [`GetToolSchemaTool`] - Get full JSON Schema for a specific tool
//!
//! # Tool Wrappers (for hot-reload)
//!
//! - [`McpToolWrapper`] - Wraps MCP server tools as rig-compatible tools
//!
//! # Tool Progress Callbacks
//!
//! This module provides a global callback mechanism for monitoring tool execution.
//! Use `set_tool_progress_handler` to receive notifications when tools start/complete.

use once_cell::sync::Lazy;
use crate::sync_primitives::{Arc, Mutex};
use tracing::debug;

pub mod agent_manage;
pub mod arena;
pub mod atomic_ops;
pub mod bash_exec;
pub mod code_exec;
pub mod error;
pub mod file_ops;
pub mod generation;
pub mod invalid;
pub mod mcp_wrapper;
pub mod memory_search;
pub mod memory_browse;
pub mod meta_tools;
pub mod pdf_generate;
pub mod search;
pub mod sessions;
pub mod skill_install;
pub mod skill_manage;
pub mod skill_reader;
pub mod skill_status;
pub mod web_fetch;
pub mod mcp_resource;
pub mod mcp_prompt;
pub mod desktop;
pub mod system_tool;
pub mod automation_tool;
pub mod permission_tool;
pub mod pim;
pub mod browser;
pub mod browser_tools;
pub mod media_tool;
pub mod media_tools;
pub mod scratchpad;
pub mod vision;
pub mod escalate_task;
pub mod acp_tools;
pub mod cron_manage;
pub mod heartbeat_manage;
pub mod clawhub;
pub mod config_guide;
pub mod self_manage;
pub mod task_manage;
pub mod channel_manage;
pub mod vault_store;
pub mod voice_tools;
pub mod media_send;
pub mod team;

pub use agent_manage::{
    AgentCreateArgs, AgentCreateOutput, AgentCreateTool, AgentDeleteArgs, AgentDeleteOutput,
    AgentDeleteTool, AgentListArgs, AgentListInfo, AgentListOutput, AgentListTool,
    SessionContext, SessionContextHandle,
};
pub use arena::{
    ArenaCreateArgs, ArenaCreateOutput, ArenaCreateTool, ArenaQueryArgs, ArenaQueryOutput,
    ArenaQueryTool, ArenaSettleArgs, ArenaSettleOutput, ArenaSettleTool, SlotSummary,
};
pub use atomic_ops::{AtomicOpsArgs, AtomicOpsOutput, AtomicOpsTool};
pub use bash_exec::{BashExecArgs, BashExecTool};
pub use code_exec::{CodeExecArgs, CodeExecTool};
pub use error::ToolError;
pub use file_ops::{FileOpsArgs, FileOpsTool};
pub use invalid::{InvalidTool, InvalidToolArgs, InvalidToolOutput};
pub use generation::{ImageGenerateArgs, ImageGenerateTool, SpeechGenerateArgs, SpeechGenerateTool};
pub use mcp_wrapper::McpToolWrapper;
pub use memory_search::{MemorySearchArgs, MemorySearchOutput, MemorySearchTool, PathCluster};
pub use memory_browse::{MemoryBrowseArgs, MemoryBrowseOutput, MemoryBrowseTool};
pub use meta_tools::{
    GetToolSchemaArgs, GetToolSchemaOutput, GetToolSchemaTool, ListToolsArgs, ListToolsOutput,
    ListToolsTool,
};
pub use pdf_generate::{PdfGenerateArgs, PdfGenerateTool};
pub use search::{SearchArgs, SearchTool};
pub use skill_reader::{
    ListSkillsArgs, ListSkillsOutput, ListSkillsTool, SkillSummary,
};
pub use web_fetch::{WebFetchArgs, WebFetchTool};
pub use mcp_resource::{McpReadResourceArgs, McpReadResourceOutput, McpReadResourceTool};
pub use mcp_prompt::{McpGetPromptArgs, McpGetPromptOutput, McpGetPromptTool, PromptOutputMessage};
pub use desktop::{DesktopArgs, DesktopOutput, DesktopTool};
pub use system_tool::{SystemArgs, SystemOutput, SystemTool};
pub use automation_tool::{AutomationArgs, AutomationOutput, AutomationTool};
pub use media_tool::{MediaArgs, MediaOutput, MediaTool};
pub use permission_tool::{PermissionArgs, PermissionOutput, PermissionTool};
pub use pim::{PimArgs, PimOutput, PimTool};
pub use browser::{BrowserAction, BrowserArgs, BrowserOutput, BrowserTool};
pub use scratchpad::{ScratchpadAction, ScratchpadArgs, ScratchpadOutput, ScratchpadTool};
pub use vision::{VisionAction, VisionArgs, VisionOutput, VisionTool};
pub use media_tools::{
    AudioTranscribeArgs, AudioTranscribeOutput, AudioTranscribeTool, DocumentExtractArgs,
    DocumentExtractOutput, DocumentExtractTool, MediaUnderstandArgs, MediaUnderstandOutput,
    MediaUnderstandTool,
};
pub use escalate_task::{EscalateTaskArgs, EscalateTaskOutput, EscalateTaskTool};
pub use acp_tools::{
    AcpDelegateArgs, AcpDelegateOutput, AcpDelegateTool,
    AcpSwitchArgs, AcpSwitchOutput, AcpSwitchTool,
};
pub use cron_manage::{CronAction, CronManageArgs, CronManageOutput, CronManageTool};
pub use heartbeat_manage::{
    HeartbeatListArgs, HeartbeatListOutput, HeartbeatListTool,
    HeartbeatCreateArgs, HeartbeatCreateOutput, HeartbeatCreateTool,
    HeartbeatUpdateArgs, HeartbeatUpdateOutput, HeartbeatUpdateTool,
    HeartbeatDeleteArgs, HeartbeatDeleteOutput, HeartbeatDeleteTool,
    HeartbeatToggleArgs, HeartbeatToggleOutput, HeartbeatToggleTool,
    HeartbeatReportAction, HeartbeatReportArgs, HeartbeatReportOutput, HeartbeatReportTool,
};
pub use clawhub::{ClawHubAction, ClawHubArgs, ClawHubOutput, ClawHubTool};
pub use config_guide::{GuideTopic, ReadConfigGuideArgs, ReadConfigGuideOutput, ReadConfigGuideTool};
pub use self_manage::{SelfManageArgs, SelfManageOutput, SelfManageTool};
pub use task_manage::{TaskCreateTool, TaskListTool, TaskUpdateTool, TaskWaitTool};
pub use channel_manage::{ChannelPairingArgs, ChannelPairingOutput, ChannelPairingTool, PairingAction};
pub use vault_store::{VaultAction, VaultStoreArgs, VaultStoreOutput, VaultStoreTool};
pub use voice_tools::{VoiceModeSetArgs, VoiceModeSetOutput, VoiceModeSetTool};
pub use team::{
    CreateAgentSpec, EnrolledMember, MemberSpec, TeamCreateArgs, TeamCreateOutput, TeamCreateTool,
    TeamDelegateArgs, TeamDelegateOutput, TeamDelegateTool,
    TeamStatusArgs, TeamStatusOutput, TeamStatusTool,
    TeamDisbandArgs, TeamDisbandOutput, TeamDisbandTool,
};


// ============================================================================
// Tool Progress Callback System
// ============================================================================

/// Callback trait for monitoring tool execution progress
///
/// Implement this trait to receive notifications when tools start and complete.
/// This enables streaming progress updates to the UI during agent execution.
pub trait ToolProgressCallback: Send + Sync {
    /// Called when a tool starts execution
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool being executed
    /// * `args_summary` - Brief summary of the arguments (may be truncated for display)
    fn on_tool_start(&self, tool_name: &str, args_summary: &str);

    /// Called when a tool completes execution
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool that completed
    /// * `result_summary` - Brief summary of the result (may be truncated for display)
    /// * `success` - Whether the tool completed successfully
    fn on_tool_result(&self, tool_name: &str, result_summary: &str, success: bool);

    /// Called when a tool emits a streaming chunk (e.g., ACP delegation output).
    /// Default no-op — existing implementations don't break.
    fn on_tool_streaming_chunk(&self, tool_name: &str, chunk: &str) {
        let _ = (tool_name, chunk);
    }
}

/// Global storage for the tool progress callback
static TOOL_PROGRESS_CALLBACK: Lazy<Mutex<Option<Arc<dyn ToolProgressCallback>>>> =
    Lazy::new(|| Mutex::new(None));

/// Set the global tool progress handler
///
/// Call this before executing agent operations to receive progress updates.
/// Pass `None` to clear the handler after execution completes.
///
/// # Thread Safety
/// This function is thread-safe. Only one handler can be active at a time.
///
/// # Example
/// ```ignore
/// let handler = Arc::new(MyHandler::new());
/// set_tool_progress_handler(Some(handler));
/// // ... execute agent operations ...
/// set_tool_progress_handler(None); // Clear after done
/// ```
pub fn set_tool_progress_handler(handler: Option<Arc<dyn ToolProgressCallback>>) {
    let mut callback = TOOL_PROGRESS_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    *callback = handler;
    debug!(
        has_handler = callback.is_some(),
        "Tool progress handler updated"
    );
}

/// Notify that a tool has started execution
///
/// Called by tool implementations at the start of their `call` method.
/// If no handler is set, this is a no-op.
pub fn notify_tool_start(tool_name: &str, args_summary: &str) {
    let callback = TOOL_PROGRESS_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref handler) = *callback {
        handler.on_tool_start(tool_name, args_summary);
    }
}

/// Notify that a tool has completed execution
///
/// Called by tool implementations at the end of their `call` method.
/// If no handler is set, this is a no-op.
pub fn notify_tool_result(tool_name: &str, result_summary: &str, success: bool) {
    let callback = TOOL_PROGRESS_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref handler) = *callback {
        handler.on_tool_result(tool_name, result_summary, success);
    }
}

/// Notify that a tool has emitted a streaming chunk
///
/// Called by streaming tools (e.g., ACP delegate) to forward real-time output.
/// If no handler is set, this is a no-op.
pub fn notify_tool_streaming_chunk(tool_name: &str, chunk: &str) {
    // Clone Arc under brief lock, then call handler outside lock.
    // This is a hot streaming path — don't hold the global Mutex during the callback.
    let handler = {
        let callback = TOOL_PROGRESS_CALLBACK.lock().unwrap_or_else(|e| e.into_inner());
        callback.as_ref().map(Arc::clone)
    };
    if let Some(ref h) = handler {
        h.on_tool_streaming_chunk(tool_name, chunk);
    }
}
