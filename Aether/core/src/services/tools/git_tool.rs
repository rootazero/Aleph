//! Git Tool
//!
//! Wraps `services::git::GitRepository` with JSON interface for LLM tool invocation.
//! This is a Tier 1 System Tool, exposed at `/git`.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

use super::SystemTool;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};
use crate::services::git::{GitOps, GitRepository};

/// Git service configuration
#[derive(Debug, Clone)]
pub struct GitServiceConfig {
    /// Allowed repository paths
    pub allowed_repos: Vec<PathBuf>,
}

impl Default for GitServiceConfig {
    fn default() -> Self {
        Self {
            allowed_repos: vec![],
        }
    }
}

/// Git MCP service
///
/// Provides Git operations (status, log, diff, branch) using git2-rs.
pub struct GitService {
    git: Arc<dyn GitOps>,
    config: GitServiceConfig,
}

impl GitService {
    /// Create a new GitService with default GitRepository implementation
    pub fn new(config: GitServiceConfig) -> Self {
        Self {
            git: Arc::new(GitRepository::new()),
            config,
        }
    }

    /// Create a new GitService with custom GitOps implementation (for testing)
    pub fn with_git_ops(git: Arc<dyn GitOps>, config: GitServiceConfig) -> Self {
        Self { git, config }
    }

    /// Check if a repository path is allowed
    fn is_repo_allowed(&self, path: &PathBuf) -> bool {
        if self.config.allowed_repos.is_empty() {
            return false;
        }

        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => return false,
        };

        self.config.allowed_repos.iter().any(|root| {
            if let Ok(canonical_root) = root.canonicalize() {
                canonical.starts_with(&canonical_root)
            } else {
                canonical.starts_with(root)
            }
        })
    }

    /// Validate repository path
    fn validate_repo(&self, path: &PathBuf) -> Result<()> {
        if !self.is_repo_allowed(path) {
            return Err(AetherError::PermissionDenied {
                message: format!("Repository not allowed: {}", path.display()),
                suggestion: Some("Check that the path is within allowed_repos configuration".to_string()),
            });
        }
        Ok(())
    }

    /// Extract repo path from arguments
    fn get_repo_arg(&self, args: &Value) -> Result<PathBuf> {
        args.get("repo")
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .ok_or_else(|| AetherError::InvalidConfig {
                message: "Missing 'repo' argument".to_string(),
                suggestion: None,
            })
    }
}

#[async_trait]
impl SystemTool for GitService {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Git repository operations (status, log, diff, branch)"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        let mut resources = Vec::new();
        for repo in &self.config.allowed_repos {
            // Check if it's actually a git repo
            if self.git.is_repo(repo).await.unwrap_or(false) {
                resources.push(McpResource {
                    uri: format!("git://{}", repo.display()),
                    name: repo.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| repo.to_string_lossy().to_string()),
                    description: Some(format!("Git repository: {}", repo.display())),
                    mime_type: None,
                });
            }
        }
        Ok(resources)
    }

    async fn read_resource(&self, uri: &str) -> Result<String> {
        let path = uri.strip_prefix("git://")
            .ok_or_else(|| AetherError::InvalidConfig {
                message: format!("Invalid git URI: {}", uri),
                suggestion: Some("Use git:// prefix".to_string()),
            })?;

        let path = PathBuf::from(path);
        self.validate_repo(&path)?;

        // Return repository info as JSON
        let branch = self.git.current_branch(&path).await?;
        let status = self.git.status(&path).await?;

        Ok(serde_json::to_string_pretty(&json!({
            "path": path.to_string_lossy(),
            "branch": branch,
            "changed_files": status.len(),
        }))?)
    }

    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "git_status".to_string(),
                description: "Get repository status (changed files)".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository path"
                        }
                    },
                    "required": ["repo"]
                }),
                requires_confirmation: false,
            },
            McpTool {
                name: "git_log".to_string(),
                description: "Get commit history".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository path"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of commits (default: 10)"
                        }
                    },
                    "required": ["repo"]
                }),
                requires_confirmation: false,
            },
            McpTool {
                name: "git_diff".to_string(),
                description: "Get diff of changes".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository path"
                        },
                        "staged": {
                            "type": "boolean",
                            "description": "Show staged changes (default: false)"
                        }
                    },
                    "required": ["repo"]
                }),
                requires_confirmation: false,
            },
            McpTool {
                name: "git_branch".to_string(),
                description: "Get current branch name".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository path"
                        }
                    },
                    "required": ["repo"]
                }),
                requires_confirmation: false,
            },
        ]
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        match name {
            "git_status" => {
                let repo = self.get_repo_arg(&args)?;
                self.validate_repo(&repo)?;

                let status = self.git.status(&repo).await?;
                let result: Vec<Value> = status
                    .into_iter()
                    .map(|s| json!({
                        "path": s.path,
                        "status": s.status,
                        "staged": s.staged,
                    }))
                    .collect();

                Ok(McpToolResult::success(json!(result)))
            }

            "git_log" => {
                let repo = self.get_repo_arg(&args)?;
                self.validate_repo(&repo)?;

                let limit = args.get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(10);

                let commits = self.git.log(&repo, limit).await?;
                let result: Vec<Value> = commits
                    .into_iter()
                    .map(|c| json!({
                        "sha": c.sha,
                        "message": c.message,
                        "author": c.author,
                        "email": c.email,
                        "timestamp": c.timestamp,
                    }))
                    .collect();

                Ok(McpToolResult::success(json!(result)))
            }

            "git_diff" => {
                let repo = self.get_repo_arg(&args)?;
                self.validate_repo(&repo)?;

                let staged = args.get("staged")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let diffs = self.git.diff(&repo, staged).await?;
                let result: Vec<Value> = diffs
                    .into_iter()
                    .map(|d| json!({
                        "file_path": d.file_path,
                        "old_start": d.old_start,
                        "new_start": d.new_start,
                        "content": d.content,
                    }))
                    .collect();

                Ok(McpToolResult::success(json!(result)))
            }

            "git_branch" => {
                let repo = self.get_repo_arg(&args)?;
                self.validate_repo(&repo)?;

                let branch = self.git.current_branch(&repo).await?;
                Ok(McpToolResult::success(json!({
                    "branch": branch,
                })))
            }

            _ => Ok(McpToolResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn requires_confirmation(&self, tool_name: &str) -> bool {
        // Read-only git operations don't need confirmation
        // Future write operations (commit, push, reset) would need confirmation
        matches!(tool_name, "git_commit" | "git_push" | "git_reset")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use git2::Repository;

    fn init_test_repo(dir: &std::path::Path) -> Repository {
        let repo = Repository::init(dir).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        repo
    }

    fn create_test_service(temp_dir: &TempDir) -> GitService {
        GitService::new(GitServiceConfig {
            allowed_repos: vec![temp_dir.path().to_path_buf()],
        })
    }

    #[tokio::test]
    async fn test_git_status() {
        let temp_dir = TempDir::new().unwrap();
        init_test_repo(temp_dir.path());
        let service = create_test_service(&temp_dir);

        // Create untracked file
        std::fs::write(temp_dir.path().join("test.txt"), "hello").unwrap();

        let result = service.call_tool("git_status", json!({
            "repo": temp_dir.path().to_string_lossy()
        })).await.unwrap();

        assert!(result.success);
        let entries: Vec<Value> = serde_json::from_value(result.content).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["path"], "test.txt");
    }

    #[tokio::test]
    async fn test_git_branch() {
        let temp_dir = TempDir::new().unwrap();
        let repo = init_test_repo(temp_dir.path());
        let service = create_test_service(&temp_dir);

        // Create initial commit
        let sig = repo.signature().unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Initial", &tree, &[]).unwrap();

        let result = service.call_tool("git_branch", json!({
            "repo": temp_dir.path().to_string_lossy()
        })).await.unwrap();

        assert!(result.success);
        let branch = result.content["branch"].as_str().unwrap();
        assert!(branch == "master" || branch == "main");
    }

    #[tokio::test]
    async fn test_repo_security() {
        let temp_dir = TempDir::new().unwrap();
        let service = create_test_service(&temp_dir);

        // Try to access repo outside allowed paths
        let result = service.call_tool("git_status", json!({
            "repo": "/tmp/some-other-repo"
        })).await;

        assert!(result.is_err());
    }
}
