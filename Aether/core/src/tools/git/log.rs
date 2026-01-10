//! Git Log Tool
//!
//! Provides commit history via the AgentTool trait.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

use super::GitContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for git_log tool
#[derive(Debug, Deserialize)]
struct GitLogParams {
    /// Path to the Git repository
    path: String,
    /// Maximum number of commits to return (default: 10)
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

/// Git log tool
///
/// Returns the commit history of a Git repository.
pub struct GitLogTool {
    ctx: GitContext,
}

impl GitLogTool {
    /// Create a new GitLogTool with the given context
    pub fn new(ctx: GitContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "git_log",
            "Get the commit history of a Git repository.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the Git repository"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of commits to return (default: 10)",
                        "default": 10
                    }
                },
                "required": ["path"]
            }),
            ToolCategory::Git,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse parameters
        let params: GitLogParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid git_log parameters: {}", e),
                suggestion: Some("Provide a valid JSON object with 'path' field".to_string()),
            }
        })?;

        let path = PathBuf::from(&params.path);

        // Validate repository path
        self.ctx.validate_repo(&path)?;

        // Check if it's a valid git repository
        if !self.ctx.git.is_repo(&path).await? {
            return Ok(ToolResult::error(format!(
                "Not a git repository: {}",
                path.display()
            )));
        }

        // Get commit log
        let commits = self.ctx.git.log(&path, params.limit).await?;

        // Format output
        let output = if commits.is_empty() {
            "No commits found".to_string()
        } else {
            let mut lines = Vec::new();
            for commit in &commits {
                // Format timestamp
                let datetime = chrono::DateTime::from_timestamp(commit.timestamp, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| commit.timestamp.to_string());

                lines.push(format!("commit {}", &commit.sha[..7.min(commit.sha.len())]));
                lines.push(format!("Author: {} <{}>", commit.author, commit.email));
                lines.push(format!("Date:   {}", datetime));
                lines.push(String::new());
                lines.push(format!("    {}", commit.message));
                lines.push(String::new());
            }
            lines.join("\n")
        };

        Ok(ToolResult::success_with_data(
            output,
            json!({
                "commit_count": commits.len(),
                "commits": commits
            }),
        ))
    }

    fn requires_confirmation(&self) -> bool {
        false // Read-only operation
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Git
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::git::{GitCommit, GitOps};
    use std::sync::Arc;

    /// Mock Git implementation for testing
    struct MockGit {
        commits: Vec<GitCommit>,
        is_repo: bool,
    }

    impl MockGit {
        fn new(commits: Vec<GitCommit>) -> Self {
            Self {
                commits,
                is_repo: true,
            }
        }

        fn not_a_repo() -> Self {
            Self {
                commits: vec![],
                is_repo: false,
            }
        }
    }

    #[async_trait]
    impl GitOps for MockGit {
        async fn status(
            &self,
            _repo_path: &std::path::Path,
        ) -> Result<Vec<crate::services::git::GitFileStatus>> {
            Ok(vec![])
        }

        async fn log(&self, _repo_path: &std::path::Path, limit: usize) -> Result<Vec<GitCommit>> {
            Ok(self.commits.iter().take(limit).cloned().collect())
        }

        async fn diff(
            &self,
            _repo_path: &std::path::Path,
            _staged: bool,
        ) -> Result<Vec<crate::services::git::GitDiff>> {
            Ok(vec![])
        }

        async fn current_branch(&self, _repo_path: &std::path::Path) -> Result<String> {
            Ok("main".to_string())
        }

        async fn is_repo(&self, _path: &std::path::Path) -> Result<bool> {
            Ok(self.is_repo)
        }
    }

    use super::super::GitConfig;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_git_log_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitLogTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("No commits found"));
    }

    #[tokio::test]
    async fn test_git_log_with_commits() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![
            GitCommit {
                sha: "abc1234567890".to_string(),
                message: "Add new feature".to_string(),
                author: "John Doe".to_string(),
                email: "john@example.com".to_string(),
                timestamp: 1704067200, // 2024-01-01 00:00:00 UTC
            },
            GitCommit {
                sha: "def0987654321".to_string(),
                message: "Initial commit".to_string(),
                author: "Jane Doe".to_string(),
                email: "jane@example.com".to_string(),
                timestamp: 1704063600,
            },
        ]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitLogTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("commit abc1234"));
        assert!(result.content.contains("John Doe"));
        assert!(result.content.contains("Add new feature"));
        assert!(result.content.contains("commit def0987"));
        assert!(result.content.contains("Initial commit"));
    }

    #[tokio::test]
    async fn test_git_log_with_limit() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![
            GitCommit {
                sha: "abc123".to_string(),
                message: "First".to_string(),
                author: "Test".to_string(),
                email: "test@example.com".to_string(),
                timestamp: 1704067200,
            },
            GitCommit {
                sha: "def456".to_string(),
                message: "Second".to_string(),
                author: "Test".to_string(),
                email: "test@example.com".to_string(),
                timestamp: 1704063600,
            },
            GitCommit {
                sha: "ghi789".to_string(),
                message: "Third".to_string(),
                author: "Test".to_string(),
                email: "test@example.com".to_string(),
                timestamp: 1704060000,
            },
        ]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitLogTool::new(ctx);

        let args = json!({
            "path": temp_dir.path().to_str().unwrap(),
            "limit": 2
        })
        .to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("First"));
        assert!(result.content.contains("Second"));
        assert!(!result.content.contains("Third"));
    }

    #[tokio::test]
    async fn test_git_log_not_a_repo() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::not_a_repo());
        let ctx = GitContext::with_git(mock, config);
        let tool = GitLogTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Not a git repository"));
    }

    #[test]
    fn test_git_log_metadata() {
        let config = GitConfig::default();
        let ctx = GitContext::new(config);
        let tool = GitLogTool::new(ctx);

        assert_eq!(tool.name(), "git_log");
        assert!(!tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Git);
    }
}
