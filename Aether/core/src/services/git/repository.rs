//! Git Repository Implementation using git2-rs
//!
//! Provides `GitRepository` which implements `GitOps` using the git2 crate
//! for native Git operations without requiring the git CLI.

use async_trait::async_trait;
use git2::{DiffOptions, Repository, StatusOptions};
use std::path::Path;

use super::{GitCommit, GitDiff, GitFileStatus, GitOps};
use crate::error::{AetherError, Result};

/// Git repository implementation using git2-rs
#[derive(Debug, Default, Clone)]
pub struct GitRepository;

impl GitRepository {
    /// Create a new GitRepository instance
    pub fn new() -> Self {
        Self
    }

    /// Open a repository at the given path
    fn open_repo(path: &Path) -> Result<Repository> {
        Repository::discover(path).map_err(|e| {
            AetherError::GitError(format!("Failed to open repository at {:?}: {}", path, e))
        })
    }

    /// Convert git2 status to string representation
    fn status_to_string(status: git2::Status) -> &'static str {
        if status.is_index_new() || status.is_wt_new() {
            "added"
        } else if status.is_index_modified() || status.is_wt_modified() {
            "modified"
        } else if status.is_index_deleted() || status.is_wt_deleted() {
            "deleted"
        } else if status.is_index_renamed() || status.is_wt_renamed() {
            "renamed"
        } else if status.is_index_typechange() || status.is_wt_typechange() {
            "typechange"
        } else if status.is_ignored() {
            "ignored"
        } else if status.is_conflicted() {
            "conflicted"
        } else {
            "untracked"
        }
    }
}

#[async_trait]
impl GitOps for GitRepository {
    async fn status(&self, repo_path: &Path) -> Result<Vec<GitFileStatus>> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Self::open_repo(&path)?;
            let mut opts = StatusOptions::new();
            opts.include_untracked(true)
                .include_ignored(false)
                .include_unmodified(false);

            let statuses = repo
                .statuses(Some(&mut opts))
                .map_err(|e| AetherError::GitError(format!("Failed to get status: {}", e)))?;

            let results: Vec<GitFileStatus> = statuses
                .iter()
                .filter_map(|entry| {
                    let path = entry.path()?.to_string();
                    let status = entry.status();

                    // Check if staged (index) or unstaged (worktree)
                    let staged = status.is_index_new()
                        || status.is_index_modified()
                        || status.is_index_deleted()
                        || status.is_index_renamed()
                        || status.is_index_typechange();

                    Some(GitFileStatus {
                        path,
                        status: Self::status_to_string(status).to_string(),
                        staged,
                    })
                })
                .collect();

            Ok(results)
        })
        .await
        .map_err(|e| AetherError::GitError(format!("Task join error: {}", e)))?
    }

    async fn log(&self, repo_path: &Path, limit: usize) -> Result<Vec<GitCommit>> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Self::open_repo(&path)?;

            let head = repo
                .head()
                .map_err(|e| AetherError::GitError(format!("Failed to get HEAD: {}", e)))?;

            let head_commit = head
                .peel_to_commit()
                .map_err(|e| AetherError::GitError(format!("Failed to get HEAD commit: {}", e)))?;

            let mut revwalk = repo
                .revwalk()
                .map_err(|e| AetherError::GitError(format!("Failed to create revwalk: {}", e)))?;

            revwalk.push(head_commit.id()).map_err(|e| {
                AetherError::GitError(format!("Failed to push HEAD to revwalk: {}", e))
            })?;

            let commits: Vec<GitCommit> = revwalk
                .take(limit)
                .filter_map(|oid_result| {
                    let oid = oid_result.ok()?;
                    let commit = repo.find_commit(oid).ok()?;
                    let author = commit.author();

                    Some(GitCommit {
                        sha: oid.to_string(),
                        message: commit.summary().unwrap_or("").to_string(),
                        author: author.name().unwrap_or("Unknown").to_string(),
                        email: author.email().unwrap_or("").to_string(),
                        timestamp: commit.time().seconds(),
                    })
                })
                .collect();

            Ok(commits)
        })
        .await
        .map_err(|e| AetherError::GitError(format!("Task join error: {}", e)))?
    }

    async fn diff(&self, repo_path: &Path, staged: bool) -> Result<Vec<GitDiff>> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Self::open_repo(&path)?;

            let mut diff_opts = DiffOptions::new();
            diff_opts.include_untracked(false);

            let diff = if staged {
                // Staged changes: diff between HEAD and index
                let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
                repo.diff_tree_to_index(head.as_ref(), None, Some(&mut diff_opts))
            } else {
                // Unstaged changes: diff between index and workdir
                repo.diff_index_to_workdir(None, Some(&mut diff_opts))
            }
            .map_err(|e| AetherError::GitError(format!("Failed to get diff: {}", e)))?;

            // Use print callback to collect diff output
            use std::cell::RefCell;
            let results: RefCell<Vec<GitDiff>> = RefCell::new(Vec::new());

            diff.print(git2::DiffFormat::Patch, |delta, hunk, line| {
                let mut results = results.borrow_mut();

                // When we see a new hunk, create a new GitDiff entry
                if let Some(hunk) = hunk {
                    if let Some(path) = delta.new_file().path() {
                        // Check if we need to create a new entry
                        let should_create = results.last().is_none_or(|last| {
                            last.file_path != path.to_string_lossy()
                                || last.new_start != hunk.new_start()
                        });

                        if should_create {
                            results.push(GitDiff {
                                file_path: path.to_string_lossy().to_string(),
                                old_start: hunk.old_start(),
                                new_start: hunk.new_start(),
                                content: String::new(),
                            });
                        }
                    }
                }

                // Append line content to the current diff
                if let Some(last) = results.last_mut() {
                    let prefix = match line.origin() {
                        '+' => "+",
                        '-' => "-",
                        ' ' => " ",
                        _ => "",
                    };
                    if let Ok(content) = std::str::from_utf8(line.content()) {
                        last.content.push_str(prefix);
                        last.content.push_str(content);
                    }
                }

                true
            })
            .map_err(|e| AetherError::GitError(format!("Failed to print diff: {}", e)))?;

            Ok(results.into_inner())
        })
        .await
        .map_err(|e| AetherError::GitError(format!("Task join error: {}", e)))?
    }

    async fn current_branch(&self, repo_path: &Path) -> Result<String> {
        let path = repo_path.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let repo = Self::open_repo(&path)?;

            let head = repo
                .head()
                .map_err(|e| AetherError::GitError(format!("Failed to get HEAD: {}", e)))?;

            if head.is_branch() {
                Ok(head.shorthand().unwrap_or("HEAD").to_string())
            } else {
                // Detached HEAD
                Ok("HEAD".to_string())
            }
        })
        .await
        .map_err(|e| AetherError::GitError(format!("Task join error: {}", e)))?
    }

    async fn is_repo(&self, path: &Path) -> Result<bool> {
        let path = path.to_path_buf();

        tokio::task::spawn_blocking(move || Ok(Repository::discover(&path).is_ok()))
            .await
            .map_err(|e| AetherError::GitError(format!("Task join error: {}", e)))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_test_repo(dir: &Path) -> Repository {
        let repo = Repository::init(dir).unwrap();

        // Configure user for commits
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();

        repo
    }

    #[tokio::test]
    async fn test_is_repo() {
        let temp_dir = TempDir::new().unwrap();
        let git = GitRepository::new();

        // Not a repo initially
        assert!(!git.is_repo(temp_dir.path()).await.unwrap());

        // Initialize repo
        init_test_repo(temp_dir.path());
        assert!(git.is_repo(temp_dir.path()).await.unwrap());
    }

    #[tokio::test]
    async fn test_current_branch() {
        let temp_dir = TempDir::new().unwrap();
        let git = GitRepository::new();

        // Initialize repo and create initial commit
        let repo = init_test_repo(temp_dir.path());

        // Create an initial commit (required for branch to exist)
        let sig = repo.signature().unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
            .unwrap();

        let branch = git.current_branch(temp_dir.path()).await.unwrap();
        // Default branch is usually "master" for git init
        assert!(branch == "master" || branch == "main");
    }

    #[tokio::test]
    async fn test_status_empty_repo() {
        let temp_dir = TempDir::new().unwrap();
        let git = GitRepository::new();

        init_test_repo(temp_dir.path());

        let status = git.status(temp_dir.path()).await.unwrap();
        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn test_status_with_file() {
        let temp_dir = TempDir::new().unwrap();
        let git = GitRepository::new();

        init_test_repo(temp_dir.path());

        // Create an untracked file
        std::fs::write(temp_dir.path().join("test.txt"), "hello").unwrap();

        let status = git.status(temp_dir.path()).await.unwrap();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].path, "test.txt");
    }
}
