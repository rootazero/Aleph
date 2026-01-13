//! Git Diff Tool
//!
//! Provides diff output for repository changes via the AgentTool trait.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

use super::GitContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for git_diff tool
#[derive(Debug, Deserialize)]
struct GitDiffParams {
    /// Path to the Git repository
    path: String,
    /// Whether to show staged changes (default: false for unstaged)
    #[serde(default)]
    staged: bool,
}

/// Git diff tool
///
/// Shows the diff of changes in a Git repository.
/// Can show either staged or unstaged changes.
pub struct GitDiffTool {
    ctx: GitContext,
}

impl GitDiffTool {
    /// Create a new GitDiffTool with the given context
    pub fn new(ctx: GitContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "git_diff",
            "Show the diff of changes in a Git repository. Can show staged or unstaged changes.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the Git repository"
                    },
                    "staged": {
                        "type": "boolean",
                        "description": "If true, show staged changes; if false (default), show unstaged changes"
                    }
                },
                "required": ["path"]
            }),
            ToolCategory::Native,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse parameters
        let params: GitDiffParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid git_diff parameters: {}", e),
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

        // Get diff
        let diffs = self.ctx.git.diff(&path, params.staged).await?;

        // Format output
        let output = if diffs.is_empty() {
            let change_type = if params.staged { "staged" } else { "unstaged" };
            format!("No {} changes", change_type)
        } else {
            let mut lines = Vec::new();
            for diff in &diffs {
                lines.push(format!("--- a/{}", diff.file_path));
                lines.push(format!("+++ b/{}", diff.file_path));
                lines.push(format!(
                    "@@ -{},{} +{},{} @@",
                    diff.old_start, 0, diff.new_start, 0
                ));
                lines.push(diff.content.clone());
                lines.push(String::new());
            }
            lines.join("\n")
        };

        Ok(ToolResult::success_with_data(
            output,
            json!({
                "file_count": diffs.len(),
                "staged": params.staged,
                "diffs": diffs
            }),
        ))
    }

    fn requires_confirmation(&self) -> bool {
        false // Read-only operation
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Native
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::git::{GitDiff, GitOps};
    use std::sync::Arc;

    /// Mock Git implementation for testing
    struct MockGit {
        diffs: Vec<GitDiff>,
        is_repo: bool,
    }

    impl MockGit {
        fn new(diffs: Vec<GitDiff>) -> Self {
            Self { diffs, is_repo: true }
        }

        fn not_a_repo() -> Self {
            Self {
                diffs: vec![],
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

        async fn log(
            &self,
            _repo_path: &std::path::Path,
            _limit: usize,
        ) -> Result<Vec<crate::services::git::GitCommit>> {
            Ok(vec![])
        }

        async fn diff(&self, _repo_path: &std::path::Path, _staged: bool) -> Result<Vec<GitDiff>> {
            Ok(self.diffs.clone())
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
    async fn test_git_diff_no_changes() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitDiffTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("No unstaged changes"));
    }

    #[tokio::test]
    async fn test_git_diff_with_changes() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![GitDiff {
            file_path: "src/main.rs".to_string(),
            old_start: 10,
            new_start: 10,
            content: "+fn new_function() {}".to_string(),
        }]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitDiffTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("--- a/src/main.rs"));
        assert!(result.content.contains("+++ b/src/main.rs"));
        assert!(result.content.contains("+fn new_function()"));
    }

    #[tokio::test]
    async fn test_git_diff_staged() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new(vec![]));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitDiffTool::new(ctx);

        let args = json!({
            "path": temp_dir.path().to_str().unwrap(),
            "staged": true
        })
        .to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("No staged changes"));
    }

    #[tokio::test]
    async fn test_git_diff_not_a_repo() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::not_a_repo());
        let ctx = GitContext::with_git(mock, config);
        let tool = GitDiffTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Not a git repository"));
    }

    #[test]
    fn test_git_diff_metadata() {
        let config = GitConfig::default();
        let ctx = GitContext::new(config);
        let tool = GitDiffTool::new(ctx);

        assert_eq!(tool.name(), "git_diff");
        assert!(!tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Native);
    }
}
