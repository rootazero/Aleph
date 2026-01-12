//! Native Function Calling Tools Module
//!
//! This module provides the `AgentTool` trait and infrastructure for
//! native LLM function calling tools. Unlike the previous `SystemTool`
//! approach that wrapped tools in MCP-style JSON interfaces, this module
//! provides direct tool invocation with typed parameters.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    NativeToolRegistry                            │
//! │  (Stores and manages Arc<dyn AgentTool> instances)              │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  Filesystem Tools        │  Git Tools        │  Other Tools     │
//! │  ├── FileReadTool        │  ├── GitStatusTool│  ├── ShellTool   │
//! │  ├── FileWriteTool       │  ├── GitDiffTool  │  ├── SystemInfo  │
//! │  ├── FileListTool        │  ├── GitLogTool   │  └── ...         │
//! │  └── ...                 │  └── ...          │                  │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key Components
//!
//! - [`AgentTool`] - The core trait that all tools implement
//! - [`ToolDefinition`] - Tool metadata including JSON Schema for LLM
//! - [`ToolResult`] - Standardized result format for tool execution
//! - [`ToolCategory`] - Tool categorization for UI grouping
//! - [`NativeToolRegistry`] - Registry for tool management and execution
//!
//! # Example
//!
//! ```rust,ignore
//! use aether_core::tools::{AgentTool, ToolDefinition, ToolResult, ToolCategory};
//!
//! pub struct MyTool;
//!
//! #[async_trait]
//! impl AgentTool for MyTool {
//!     fn name(&self) -> &str {
//!         "my_tool"
//!     }
//!
//!     fn definition(&self) -> ToolDefinition {
//!         ToolDefinition::new(
//!             "my_tool",
//!             "Does something useful",
//!             json!({
//!                 "type": "object",
//!                 "properties": {
//!                     "input": { "type": "string" }
//!                 },
//!                 "required": ["input"]
//!             }),
//!             ToolCategory::Other,
//!         )
//!     }
//!
//!     async fn execute(&self, args: &str) -> Result<ToolResult> {
//!         // Parse args, do work, return result
//!         Ok(ToolResult::success("Done!"))
//!     }
//! }
//! ```
//!
//! # Module Organization
//!
//! - `traits` - Core trait definitions (`AgentTool`, `ToolDefinition`, etc.)
//! - `registry` - `NativeToolRegistry` for tool management
//! - `filesystem/` - Filesystem operation tools
//! - `git/` - Git operation tools
//! - `shell/` - Shell execution tools
//! - `system/` - System information tools
//! - `clipboard/` - Clipboard read tools
//! - `screen/` - Screen capture tools
//! - `search/` - Web search tools

pub mod clipboard;
pub mod filesystem;
pub mod git;
pub mod handler;
pub mod params;
pub mod screen;
pub mod search;
pub mod shell;
pub mod system;
mod registry;
mod traits;

// Re-export core types
pub use registry::NativeToolRegistry;
pub use traits::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

// Re-export params types for schemars-based tool definitions
pub use params::{
    FileReadParams, FileWriteParams, SearchParams, ShellExecuteParams, SummarizeParams,
    ToolOutput, ToolParams, TranslateParams,
};

// Re-export handler types for type-safe tool execution
pub use handler::{wrap_handler, DynToolHandler, ToolHandler, ToolHandlerDef, TypedHandlerWrapper};

// Re-export filesystem tools for convenience
pub use filesystem::{
    create_all_tools as create_filesystem_tools, FileDeleteTool, FileListTool, FileReadTool,
    FileSearchTool, FileWriteTool, FilesystemConfig, FilesystemContext,
};

// Re-export git tools for convenience
pub use git::{
    create_all_tools as create_git_tools, GitBranchTool, GitConfig, GitContext, GitDiffTool,
    GitLogTool, GitStatusTool,
};

// Re-export shell tools for convenience
pub use shell::{
    create_all_tools as create_shell_tools, ShellConfig, ShellContext, ShellExecuteTool,
};

// Re-export system tools for convenience
pub use system::{create_all_tools as create_system_tools, SystemContext, SystemInfoTool};

// Re-export clipboard tools for convenience
pub use clipboard::{
    create_all_tools as create_clipboard_tools, ClipboardContent, ClipboardContext,
    ClipboardReadTool,
};

// Re-export screen tools for convenience
pub use screen::{
    create_all_tools as create_screen_tools, ScreenCaptureTool, ScreenConfig, ScreenContext,
};

// Re-export search tools for convenience
pub use search::{
    create_all_tools as create_search_tools, SearchConfig, SearchContext, WebSearchTool,
};
