//! Git Operations Module
//!
//! Provides async Git operations through the `GitOps` trait.
//! The `GitRepository` implementation uses git2-rs for native Git operations
//! without requiring the git CLI to be installed.

mod repository;

pub use repository::GitRepository;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::Result;

/// Git file status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileStatus {
    /// Path of the file relative to repository root
    pub path: String,
    /// Status string (e.g., "modified", "added", "deleted", "renamed", "untracked")
    pub status: String,
    /// Whether the change is staged for commit
    pub staged: bool,
}

/// Git commit information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommit {
    /// Full commit SHA
    pub sha: String,
    /// Commit message (first line)
    pub message: String,
    /// Author name
    pub author: String,
    /// Author email
    pub email: String,
    /// Commit timestamp (Unix seconds)
    pub timestamp: i64,
}

/// Git diff information for a single file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    /// Path of the file
    pub file_path: String,
    /// Starting line number in the old file
    pub old_start: u32,
    /// Starting line number in the new file
    pub new_start: u32,
    /// Diff content (unified format)
    pub content: String,
}

/// Trait for Git operations
///
/// This trait provides a unified interface for Git operations that can be
/// implemented by different backends (git2-rs, mock for testing).
#[async_trait]
pub trait GitOps: Send + Sync {
    /// Get repository status (changed files)
    ///
    /// # Arguments
    /// * `repo_path` - Path to the Git repository
    ///
    /// # Returns
    /// List of files with their status
    async fn status(&self, repo_path: &Path) -> Result<Vec<GitFileStatus>>;

    /// Get commit history
    ///
    /// # Arguments
    /// * `repo_path` - Path to the Git repository
    /// * `limit` - Maximum number of commits to return
    ///
    /// # Returns
    /// List of commits, most recent first
    async fn log(&self, repo_path: &Path, limit: usize) -> Result<Vec<GitCommit>>;

    /// Get diff of changes
    ///
    /// # Arguments
    /// * `repo_path` - Path to the Git repository
    /// * `staged` - If true, show staged changes; if false, show unstaged changes
    ///
    /// # Returns
    /// List of diffs for changed files
    async fn diff(&self, repo_path: &Path, staged: bool) -> Result<Vec<GitDiff>>;

    /// Get current branch name
    ///
    /// # Arguments
    /// * `repo_path` - Path to the Git repository
    ///
    /// # Returns
    /// Current branch name, or "HEAD" if detached
    async fn current_branch(&self, repo_path: &Path) -> Result<String>;

    /// Check if path is a Git repository
    ///
    /// # Arguments
    /// * `path` - Path to check
    ///
    /// # Returns
    /// true if path is inside a Git repository
    async fn is_repo(&self, path: &Path) -> Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Mock implementation for testing
    pub struct MockGit {
        pub is_repo: bool,
        pub branch: String,
    }

    impl Default for MockGit {
        fn default() -> Self {
            Self {
                is_repo: true,
                branch: "main".to_string(),
            }
        }
    }

    #[async_trait]
    impl GitOps for MockGit {
        async fn status(&self, _repo_path: &Path) -> Result<Vec<GitFileStatus>> {
            Ok(vec![GitFileStatus {
                path: "test.rs".to_string(),
                status: "modified".to_string(),
                staged: false,
            }])
        }

        async fn log(&self, _repo_path: &Path, limit: usize) -> Result<Vec<GitCommit>> {
            let commits = vec![GitCommit {
                sha: "abc123".to_string(),
                message: "Initial commit".to_string(),
                author: "Test Author".to_string(),
                email: "test@example.com".to_string(),
                timestamp: 1704067200,
            }];
            Ok(commits.into_iter().take(limit).collect())
        }

        async fn diff(&self, _repo_path: &Path, _staged: bool) -> Result<Vec<GitDiff>> {
            Ok(vec![])
        }

        async fn current_branch(&self, _repo_path: &Path) -> Result<String> {
            Ok(self.branch.clone())
        }

        async fn is_repo(&self, _path: &Path) -> Result<bool> {
            Ok(self.is_repo)
        }
    }

    #[tokio::test]
    async fn test_mock_git_status() {
        let git: Arc<dyn GitOps> = Arc::new(MockGit::default());
        let status = git.status(Path::new("/test")).await.unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].path, "test.rs");
    }

    #[tokio::test]
    async fn test_mock_git_branch() {
        let git: Arc<dyn GitOps> = Arc::new(MockGit::default());
        let branch = git.current_branch(Path::new("/test")).await.unwrap();
        assert_eq!(branch, "main");
    }
}
