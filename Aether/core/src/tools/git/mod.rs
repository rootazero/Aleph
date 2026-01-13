//! Git Tools Module
//!
//! Provides native AgentTool implementations for Git operations.
//! All tools share a common `GitContext` for repository path security
//! and consistent Git access.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `git_status` | Get repository status | No |
//! | `git_diff` | Show diff of changes | No |
//! | `git_log` | Get commit history | No |
//! | `git_branch` | Get current branch | No |
//!
//! # Security
//!
//! All tools validate repository paths against `GitConfig.allowed_repos`.
//! If `allowed_repos` is empty, all repositories are allowed (for backwards compatibility).
//! Operations outside allowed directories are rejected with a `PermissionDenied` error.
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::git::{
//!     GitConfig, GitContext,
//!     GitStatusTool, GitDiffTool, GitLogTool, GitBranchTool,
//! };
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create shared context
//! let config = GitConfig::new(vec!["/home/user/projects".into()]);
//! let ctx = GitContext::new(config);
//!
//! // Register tools
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(GitStatusTool::new(ctx.clone()))).await;
//! registry.register(Arc::new(GitDiffTool::new(ctx.clone()))).await;
//! registry.register(Arc::new(GitLogTool::new(ctx.clone()))).await;
//! registry.register(Arc::new(GitBranchTool::new(ctx.clone()))).await;
//!
//! // Execute
//! let result = registry.execute("git_status", r#"{"path": "/home/user/projects/repo"}"#).await?;
//! ```

mod branch;
mod config;
mod diff;
mod log;
mod status;

pub use branch::GitBranchTool;
pub use config::{GitConfig, GitContext};
pub use diff::GitDiffTool;
pub use log::GitLogTool;
pub use status::GitStatusTool;

use std::sync::Arc;

use super::AgentTool;

/// Create all git tools with shared context
///
/// Convenience function to create all git tools at once.
///
/// # Arguments
///
/// * `config` - Git repository security configuration
///
/// # Returns
///
/// Vector of Arc-wrapped AgentTool implementations
pub fn create_all_tools(config: GitConfig) -> Vec<Arc<dyn AgentTool>> {
    let ctx = GitContext::new(config);
    vec![
        Arc::new(GitStatusTool::new(ctx.clone())),
        Arc::new(GitDiffTool::new(ctx.clone())),
        Arc::new(GitLogTool::new(ctx.clone())),
        Arc::new(GitBranchTool::new(ctx)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_all_tools() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);

        let tools = create_all_tools(config);

        assert_eq!(tools.len(), 4);

        let names: Vec<_> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"git_status"));
        assert!(names.contains(&"git_diff"));
        assert!(names.contains(&"git_log"));
        assert!(names.contains(&"git_branch"));
    }

    #[test]
    fn test_all_tools_are_read_only() {
        let config = GitConfig::default();
        let tools = create_all_tools(config);

        for tool in &tools {
            assert!(
                !tool.requires_confirmation(),
                "{} should not require confirmation (read-only)",
                tool.name()
            );
        }
    }

    #[test]
    fn test_all_tools_have_git_category() {
        use crate::tools::ToolCategory;

        let config = GitConfig::default();
        let tools = create_all_tools(config);

        for tool in &tools {
            assert_eq!(
                tool.category(),
                ToolCategory::Native,
                "{} should have Git category",
                tool.name()
            );
        }
    }
}
