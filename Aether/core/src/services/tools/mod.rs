//! System Tools Module
//!
//! Tier 1 built-in tools that provide native Rust implementations
//! exposed via MCP-like JSON interface for LLM tool invocation.
//!
//! # Tool Categories
//!
//! - **Tier 1 (System Tools)**: Always available, native code, top-level commands
//!   - `/fs` - File system operations
//!   - `/git` - Git repository operations
//!   - `/shell` - Shell command execution
//!   - `/sys` - System information
//!   - `/clipboard` - Clipboard read (NEW)
//!   - `/screen` - Screen capture (NEW)
//!   - `/search` - Web search (NEW)
//!
//! - **Tier 2 (MCP Extensions)**: User-installed, external processes, under `/mcp/`
//!   - See `mcp/external/` module

mod clipboard_tool;
mod fs_tool;
mod git_tool;
mod screen_tool;
mod search_tool;
mod shell_tool;
mod sys_tool;
mod traits;

// Existing tool exports
pub use fs_tool::{FsService, FsServiceConfig};
pub use git_tool::{GitService, GitServiceConfig};
pub use shell_tool::{ShellService, ShellServiceConfig};
pub use sys_tool::SystemInfoService;
pub use traits::SystemTool;

// New tool exports
pub use clipboard_tool::{ClipboardContent, ClipboardService};
pub use screen_tool::ScreenCaptureService;
pub use search_tool::SearchService;

// Backward compatibility alias
pub use traits::BuiltinMcpService;
