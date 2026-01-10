//! Git Tools Configuration
//!
//! Shared configuration for all git tools, including repository path validation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{AetherError, Result};
use crate::services::git::{GitOps, GitRepository};

/// Shared configuration for git tools
///
/// All git tools share this configuration to ensure consistent
/// repository path validation and Git operations access.
#[derive(Debug, Clone)]
pub struct GitConfig {
    /// Allowed repository directories
    ///
    /// All git operations must be within one of these directories.
    /// An empty list allows all repositories (less secure).
    pub allowed_repos: Vec<PathBuf>,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            allowed_repos: vec![],
        }
    }
}

impl GitConfig {
    /// Create a new configuration with allowed repositories
    pub fn new(allowed_repos: Vec<PathBuf>) -> Self {
        Self { allowed_repos }
    }

    /// Create a configuration that allows access to home directory
    pub fn with_home_dir() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            allowed_repos: vec![home],
        }
    }

    /// Check if a repository path is allowed
    ///
    /// Returns true if no restrictions are configured (empty list),
    /// or if the path is within any of the allowed directories.
    pub fn is_repo_allowed(&self, path: &Path) -> bool {
        // If no restrictions, allow all (for backwards compatibility)
        if self.allowed_repos.is_empty() {
            return true;
        }

        // Try to canonicalize the path
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            // If path doesn't exist, find first existing ancestor
            Err(_) => {
                if let Some((ancestor, remaining)) = self.find_existing_ancestor(path) {
                    match ancestor.canonicalize() {
                        Ok(p) => p.join(remaining),
                        Err(_) => return false,
                    }
                } else {
                    return false;
                }
            }
        };

        // Check if path is within any allowed repository
        self.allowed_repos.iter().any(|root| {
            if let Ok(canonical_root) = root.canonicalize() {
                canonical.starts_with(&canonical_root)
            } else {
                // For roots that don't exist yet, do prefix match
                canonical.starts_with(root)
            }
        })
    }

    /// Find the first existing ancestor of a path and the remaining path components
    fn find_existing_ancestor(&self, path: &Path) -> Option<(PathBuf, PathBuf)> {
        let mut current = path.to_path_buf();
        let mut remaining_parts: Vec<_> = Vec::new();

        loop {
            if current.exists() {
                let remaining = remaining_parts
                    .into_iter()
                    .rev()
                    .fold(PathBuf::new(), |acc, part| acc.join(part));
                return Some((current, remaining));
            }

            if let Some(name) = current.file_name() {
                remaining_parts.push(PathBuf::from(name));
            }

            if let Some(parent) = current.parent() {
                if parent == current {
                    break;
                }
                current = parent.to_path_buf();
            } else {
                break;
            }
        }

        None
    }

    /// Validate repository path and return error if not allowed
    pub fn validate_repo(&self, path: &Path) -> Result<()> {
        if !self.is_repo_allowed(path) {
            return Err(AetherError::PermissionDenied {
                message: format!("Repository not allowed: {}", path.display()),
                suggestion: Some(
                    "Check that the path is within allowed_repos configuration".to_string(),
                ),
            });
        }
        Ok(())
    }

    /// Add an allowed repository directory
    pub fn add_allowed_repo(&mut self, repo: PathBuf) {
        self.allowed_repos.push(repo);
    }
}

/// Git tools context
///
/// Provides shared access to Git operations and configuration.
/// Used by all git tool implementations.
pub struct GitContext {
    /// Git operations implementation
    pub git: Arc<dyn GitOps>,
    /// Security configuration
    pub config: Arc<GitConfig>,
}

impl GitContext {
    /// Create a new context with GitRepository implementation
    pub fn new(config: GitConfig) -> Self {
        Self {
            git: Arc::new(GitRepository::new()),
            config: Arc::new(config),
        }
    }

    /// Create a new context with custom GitOps implementation (for testing)
    pub fn with_git(git: Arc<dyn GitOps>, config: GitConfig) -> Self {
        Self {
            git,
            config: Arc::new(config),
        }
    }

    /// Validate repository path against security configuration
    pub fn validate_repo(&self, path: &Path) -> Result<()> {
        self.config.validate_repo(path)
    }

    /// Check if repository path is allowed
    pub fn is_repo_allowed(&self, path: &Path) -> bool {
        self.config.is_repo_allowed(path)
    }
}

impl Clone for GitContext {
    fn clone(&self) -> Self {
        Self {
            git: Arc::clone(&self.git),
            config: Arc::clone(&self.config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_default_allows_all() {
        let config = GitConfig::default();
        // Default config allows all repos (empty allowed_repos)
        assert!(config.is_repo_allowed(Path::new("/tmp/test-repo")));
        assert!(config.is_repo_allowed(Path::new("/home/user/projects")));
    }

    #[test]
    fn test_config_allows_within_root() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);

        // Create a test directory
        let test_repo = temp_dir.path().join("my-repo");
        std::fs::create_dir(&test_repo).unwrap();

        assert!(config.is_repo_allowed(&test_repo));
        assert!(!config.is_repo_allowed(Path::new("/etc/other")));
    }

    #[test]
    fn test_config_denies_outside_root() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);

        assert!(!config.is_repo_allowed(Path::new("/etc/passwd")));
        assert!(!config.is_repo_allowed(Path::new("/tmp/other/repo")));
    }

    #[test]
    fn test_validate_repo_error() {
        let temp_dir = TempDir::new().unwrap();
        let config = GitConfig::new(vec![temp_dir.path().to_path_buf()]);
        let result = config.validate_repo(Path::new("/etc/passwd"));

        assert!(result.is_err());
        match result {
            Err(AetherError::PermissionDenied { message, .. }) => {
                assert!(message.contains("/etc/passwd"));
            }
            _ => panic!("Expected PermissionDenied error"),
        }
    }

    #[test]
    fn test_add_allowed_repo() {
        let mut config = GitConfig::new(vec![PathBuf::from("/home/user/projects")]);
        let temp_dir = TempDir::new().unwrap();

        config.add_allowed_repo(temp_dir.path().to_path_buf());

        // Create a test directory
        let test_repo = temp_dir.path().join("test-repo");
        std::fs::create_dir(&test_repo).unwrap();

        assert!(config.is_repo_allowed(&test_repo));
    }
}
