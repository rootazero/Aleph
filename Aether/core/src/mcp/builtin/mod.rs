//! Builtin MCP Services
//!
//! These services run directly in the Rust core without external dependencies.
//! They wrap the shared foundation modules with MCP protocol adaptation.

mod fs_service;
mod git_service;
mod shell_service;
mod system_info_service;
mod traits;

pub use fs_service::{FsService, FsServiceConfig};
pub use git_service::{GitService, GitServiceConfig};
pub use shell_service::{ShellService, ShellServiceConfig};
pub use system_info_service::SystemInfoService;
pub use traits::BuiltinMcpService;
