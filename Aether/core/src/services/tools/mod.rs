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
//!
//! - **Tier 2 (MCP Extensions)**: User-installed, external processes, under `/mcp/`
//!   - See `mcp/external/` module

mod fs_tool;
mod git_tool;
mod shell_tool;
mod sys_tool;
mod traits;

pub use fs_tool::{FsService, FsServiceConfig};
pub use git_tool::{GitService, GitServiceConfig};
pub use shell_tool::{ShellService, ShellServiceConfig};
pub use sys_tool::SystemInfoService;
pub use traits::SystemTool;

// Backward compatibility alias
pub use traits::BuiltinMcpService;
