//! Filesystem Tools Module
//!
//! Provides native AgentTool implementations for filesystem operations.
//! All tools share a common `FilesystemContext` for path security and
//! consistent file system access.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `file_read` | Read file contents | No |
//! | `file_write` | Write content to file | Yes |
//! | `file_list` | List directory contents | No |
//! | `file_delete` | Delete file or directory | Yes |
//! | `file_search` | Search files by glob pattern | No |
//!
//! # Security
//!
//! All tools validate paths against `FilesystemConfig.allowed_roots`.
//! Operations outside allowed directories are rejected with a
//! `PermissionDenied` error.
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::filesystem::{
//!     FilesystemConfig, FilesystemContext,
//!     FileReadTool, FileWriteTool, FileListTool,
//! };
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create shared context
//! let config = FilesystemConfig::new(vec!["/home/user/projects".into()]);
//! let ctx = FilesystemContext::new(config);
//!
//! // Register tools
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(FileReadTool::new(ctx.clone()))).await;
//! registry.register(Arc::new(FileWriteTool::new(ctx.clone()))).await;
//! registry.register(Arc::new(FileListTool::new(ctx.clone()))).await;
//!
//! // Execute
//! let result = registry.execute("file_read", r#"{"path": "/home/user/projects/file.txt"}"#).await?;
//! ```

mod config;
mod file_delete;
mod file_list;
mod file_read;
mod file_search;
mod file_write;

pub use config::{FilesystemConfig, FilesystemContext};
pub use file_delete::FileDeleteTool;
pub use file_list::FileListTool;
pub use file_read::FileReadTool;
pub use file_search::FileSearchTool;
pub use file_write::FileWriteTool;

use std::sync::Arc;

use super::AgentTool;

/// Create all filesystem tools with shared context
///
/// Convenience function to create all filesystem tools at once.
///
/// # Arguments
///
/// * `config` - Filesystem security configuration
///
/// # Returns
///
/// Vector of Arc-wrapped AgentTool implementations
pub fn create_all_tools(config: FilesystemConfig) -> Vec<Arc<dyn AgentTool>> {
    let ctx = FilesystemContext::new(config);
    vec![
        Arc::new(FileReadTool::new(ctx.clone())),
        Arc::new(FileWriteTool::new(ctx.clone())),
        Arc::new(FileListTool::new(ctx.clone())),
        Arc::new(FileDeleteTool::new(ctx.clone())),
        Arc::new(FileSearchTool::new(ctx)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_all_tools() {
        let temp_dir = TempDir::new().unwrap();
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);

        let tools = create_all_tools(config);

        assert_eq!(tools.len(), 5);

        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"file_write"));
        assert!(names.contains(&"file_list"));
        assert!(names.contains(&"file_delete"));
        assert!(names.contains(&"file_search"));
    }

    #[test]
    fn test_confirmation_requirements() {
        let temp_dir = TempDir::new().unwrap();
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);
        let tools = create_all_tools(config);

        for tool in &tools {
            let requires = tool.requires_confirmation();
            match tool.name() {
                "file_read" | "file_list" | "file_search" => {
                    assert!(!requires, "{} should not require confirmation", tool.name());
                }
                "file_write" | "file_delete" => {
                    assert!(requires, "{} should require confirmation", tool.name());
                }
                _ => panic!("Unknown tool: {}", tool.name()),
            }
        }
    }
}
