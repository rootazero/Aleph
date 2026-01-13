//! Git Branch Tool
//!
//! Provides current branch information via the AgentTool trait.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

use super::GitContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for git_branch tool
#[derive(Debug, Deserialize)]
struct GitBranchParams {
    /// Path to the Git repository
    path: String,
}

/// Git branch tool
///
/// Returns the current branch name of a Git repository.
pub struct GitBranchTool {
    ctx: GitContext,
}

impl GitBranchTool {
    /// Create a new GitBranchTool with the given context
    pub fn new(ctx: GitContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for GitBranchTool {
    fn name(&self) -> &str {
        "git_branch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "git_branch",
            "Get the current branch name of a Git repository.",
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
            ToolCategory::Native,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse parameters
        let params: GitBranchParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid git_branch parameters: {}", e),
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

        // Get current branch
        let branch = self.ctx.git.current_branch(&path).await?;

        // Format output
        let output = format!("Current branch: {}", branch);

        Ok(ToolResult::success_with_data(
            output,
            json!({
                "branch": branch,
                "is_detached": branch == "HEAD"
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
    use crate::services::git::GitOps;
    use std::sync::Arc;

    /// Mock Git implementation for testing
    struct MockGit {
        branch: String,
        is_repo: bool,
    }

    impl MockGit {
        fn new(branch: &str) -> Self {
            Self {
                branch: branch.to_string(),
                is_repo: true,
            }
        }

        fn not_a_repo() -> Self {
            Self {
                branch: String::new(),
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

        async fn diff(
            &self,
            _repo_path: &std::path::Path,
            _staged: bool,
        ) -> Result<Vec<crate::services::git::GitDiff>> {
            Ok(vec![])
        }

        async fn current_branch(&self, _repo_path: &std::path::Path) -> Result<String> {
            Ok(self.branch.clone())
        }

        async fn is_repo(&self, _path: &std::path::Path) -> Result<bool> {
            Ok(self.is_repo)
        }
    }

    use super::super::GitConfig;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_git_branch_main() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new("main"));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitBranchTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Current branch: main"));

        // Check data field
        if let Some(data) = &result.data {
            assert_eq!(data["branch"], "main");
            assert_eq!(data["is_detached"], false);
        }
    }

    #[tokio::test]
    async fn test_git_branch_feature() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new("feature/new-feature"));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitBranchTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Current branch: feature/new-feature"));
    }

    #[tokio::test]
    async fn test_git_branch_detached() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::new("HEAD"));
        let ctx = GitContext::with_git(mock, config);
        let tool = GitBranchTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(result.success);
        assert!(result.content.contains("Current branch: HEAD"));

        // Check data field for detached state
        if let Some(data) = &result.data {
            assert_eq!(data["is_detached"], true);
        }
    }

    #[tokio::test]
    async fn test_git_branch_not_a_repo() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let mock = Arc::new(MockGit::not_a_repo());
        let ctx = GitContext::with_git(mock, config);
        let tool = GitBranchTool::new(ctx);

        let args = json!({ "path": temp_dir.path().to_str().unwrap() }).to_string();
        let result = tool.execute(&args).await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Not a git repository"));
    }

    #[test]
    fn test_git_branch_metadata() {
        let config = GitConfig::default();
        let ctx = GitContext::new(config);
        let tool = GitBranchTool::new(ctx);

        assert_eq!(tool.name(), "git_branch");
        assert!(!tool.requires_confirmation());
        assert_eq!(tool.category(), ToolCategory::Native);
    }
}
