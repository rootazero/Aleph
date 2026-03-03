//! Rig tool implementations
//!
//! All tools implement rig's Tool trait for AI-callable functions.
//!
//! # Built-in Tools
//!
//! - [`SearchTool`] - Web search via SearXNG
//! - [`WebFetchTool`] - Web page fetching
//! - [`YouTubeTool`] - YouTube video transcript extraction
//! - [`FileOpsTool`] - File system operations (list, read, write, move, copy, delete, mkdir, search)
//! - [`AtomicOpsTool`] - Atomic operations (search, replace, move) powered by Atomic Engine
//! - [`CodeExecTool`] - Code execution (Python, JavaScript, Shell)
//! - [`PdfGenerateTool`] - PDF generation from text/Markdown
//! - [`ImageGenerateTool`] - Image generation from text prompts
//! - [`SpeechGenerateTool`] - Text-to-speech generation
//! - [`MessageTool`] - Cross-channel message operations
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
pub mod message;
pub mod meta_tools;
pub mod pdf_generate;
pub mod search;
pub mod sessions;
pub mod skill_reader;
pub mod web_fetch;
pub mod youtube;
pub mod mcp_resource;
pub mod mcp_prompt;
pub mod desktop;
pub mod pim;
pub mod browser;
pub mod config_read;
pub mod config_update;
pub mod profile_update;
pub mod scratchpad;
pub mod soul_update;
pub mod vision;

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
    ListSkillsArgs, ListSkillsOutput, ListSkillsTool, ReadSkillArgs, ReadSkillOutput,
    ReadSkillTool, SkillSummary,
};
pub use web_fetch::{WebFetchArgs, WebFetchTool};
pub use youtube::{YouTubeArgs, YouTubeTool};
pub use mcp_resource::{McpReadResourceArgs, McpReadResourceOutput, McpReadResourceTool};
pub use mcp_prompt::{McpGetPromptArgs, McpGetPromptOutput, McpGetPromptTool, PromptOutputMessage};
pub use desktop::{DesktopArgs, DesktopOutput, DesktopTool};
pub use pim::{PimArgs, PimOutput, PimTool};
pub use browser::{BrowserAction, BrowserArgs, BrowserOutput, BrowserTool};
pub use config_read::{ConfigReadArgs, ConfigReadOutput, ConfigReadTool};
pub use config_update::{ConfigUpdateArgs, ConfigUpdateOutput, ConfigUpdateTool};
pub use profile_update::{ProfileField, ProfileOperation, ProfileUpdateArgs, ProfileUpdateOutput, ProfileUpdateTool};
pub use scratchpad::{ScratchpadAction, ScratchpadArgs, ScratchpadOutput, ScratchpadTool};
pub use soul_update::{SoulField, SoulOperation, SoulUpdateArgs, SoulUpdateOutput, SoulUpdateTool};
pub use vision::{VisionAction, VisionArgs, VisionOutput, VisionTool};

// Message tool re-exports
pub use message::{
    ChannelCapabilities, DeleteParams, EditParams, MessageAction, MessageOperations,
    MessageResult, MessageTool, MessageToolArgs, MessageToolOutput, ReactParams, ReplyParams,
    SendParams,
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
