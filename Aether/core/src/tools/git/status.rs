//! Git Status Tool
//!
//! Provides repository status information via the AgentTool trait.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

use super::GitContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for git_status tool
#[derive(Debug, Deserialize)]
struct GitStatusParams {
    /// Path to the Git repository
    path: String,
}

/// Git status tool
///
/// Returns the status of files in a Git repository,
/// including modified, added, deleted, and untracked files.
pub struct GitStatusTool {
    ctx: GitContext,
}

impl GitStatusTool {
    /// Create a new GitStatusTool with the given context
    pub fn new(ctx: GitContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "git_status",
            "Get the status of files in a Git repository. Returns modified, added, deleted, and untracked files.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the Git repository"
                    }
                },
                "required": ["path"]
            }),
            ToolCategory::Git,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse parameters
        let params: GitStatusParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid git_status parameters: {}", e),
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

        // Get repository status
        let status = self.ctx.git.status(&path).await?;

        // Format output
        let output = if status.is_empty() {
            "Working tree clean - no changes detected".to_string()
        } else {
            let mut lines = Vec::new();

            // Group by staged/unstaged
            let staged: Vec<_> = status.iter().filter(|s| s.staged).collect();
            let unstaged: Vec<_> = status.iter().filter(|s| !s.staged).collect();

            if !staged.is_empty() {
                lines.push("Changes to be committed:".to_string());
                for file in &staged {
                    lines.push(format!("  {} {}", file.status, file.path));
                }
            }

            if !unstaged.is_empty() {
                if !staged.is_empty() {
                    lines.push(String::new());
                }
                lines.push("Changes not staged for commit:".to_string());
                for file in &unstaged {
                    lines.push(format!("  {} {}", file.status, file.path));
                }
            }

            lines.join("\n")
        };

        Ok(ToolResult::success_with_data(
            output,
            json!({
                "file_count": status.len(),
                "staged_count": status.iter().filter(|s| s.staged).count(),
                "unstaged_count": status.iter().filter(|s| !s.staged).count(),
                "files": status
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
    use crate::services::git::{GitFileStatus, GitOps};
    use std::sync::Arc;

    /// Mock Git implementation for testing
    struct MockGit {
        files: Vec<GitFileStatus>,
        is_repo: bool,
    }

    impl MockGit {
        fn new(files: Vec<GitFileStatus>) -> Self {
            Self {
                files,
                is_repo: true,
            }
        }

        fn not_a_repo() -> Self {
            Self {
                files: vec![],
                is_repo: false,
            }
        }
    }

    #[async_trait]
    impl GitOps for MockGit {
        async fn status(&self, _repo_path: &std::path::Path) -> Result<Vec<GitFileStatus>> {
            Ok(self.files.clone())
        }

        async fn log(
            &self,
            _repo_path: &std::path::Path,
            _limit: usize,
        ) -> Result<Vec<crate::services::git::GitCommit>> {
            Ok(vec![])
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
    async fn test_git_status_clean() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitStatusTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Working tree clean"));
    }

    #[tokio::test]
    async fn test_git_status_with_changes() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![
            GitFileStatus {
                path: "src/main.rs".to_string(),
                status: "modified".to_string(),
                staged: false,
            },
            GitFileStatus {
                path: "README.md".to_string(),
                status: "added".to_string(),
                staged: true,
            },
        ]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitStatusTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Changes to be committed"));
        assert!(result.content.contains("README.md"));
        assert!(result.content.contains("Changes not staged for commit"));
        assert!(result.content.contains("src/main.rs"));
    }

    #[tokio::test]
    async fn test_git_status_not_a_repo() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::not_a_repo());
        let ctx = GitContext::with_git(mock, config);
        let tool = GitStatusTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Not a git repository"));
    }

    #[test]
    fn test_git_status_metadata() {
        let config = GitConfig::default();
        let ctx = GitContext::new(config);
        let tool = GitStatusTool::new(ctx);

        assert_eq!(tool.name(), "git_status");
        assert!(!tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Git);
    }
}
