//! Filesystem Tools Configuration
//!
//! Shared configuration for all filesystem tools, including path security.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{AetherError, Result};
use crate::services::fs::{FileOps, LocalFs};

/// Shared configuration for filesystem tools
///
/// All filesystem tools share this configuration to ensure consistent
/// path security and file system access.
#[derive(Debug, Clone)]
pub struct FilesystemConfig {
    /// Allowed root directories for file operations
    ///
    /// All file operations must be within one of these directories.
    /// An empty list denies all access.
    pub allowed_roots: Vec<PathBuf>,
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            allowed_roots: vec![],
        }
    }
}

impl FilesystemConfig {
    /// Create a new configuration with allowed roots
    pub fn new(allowed_roots: Vec<PathBuf>) -> Self {
        Self { allowed_roots }
    }

    /// Create a configuration that allows access to home directory
    pub fn with_home_dir() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            allowed_roots: vec![home],
        }
    }

    /// Check if a path is within allowed roots
    ///
    /// Returns true if the path is within any of the allowed directories.
    pub fn is_path_allowed(&self, path: &Path) -> bool {
        // If no roots are configured, deny all access
        if self.allowed_roots.is_empty() {
            return false;
        }

        // Try to canonicalize the path, walking up to find existing ancestor
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

        // Check if path is within any allowed root
        self.allowed_roots.iter().any(|root| {
            if let Ok(canonical_root) = root.canonicalize() {
                canonical.starts_with(&canonical_root)
            } else {
                // For roots that don't exist yet, do prefix match
                canonical.starts_with(root)
            }
        })
    }

    /// Find the first existing ancestor of a path and the remaining path components
    ///
    /// Returns (existing_ancestor, remaining_path) where remaining_path is the
    /// relative path from ancestor to the original path.
    fn find_existing_ancestor(&self, path: &Path) -> Option<(PathBuf, PathBuf)> {
        let mut current = path.to_path_buf();
        let mut remaining_parts: Vec<_> = Vec::new();

        loop {
            if current.exists() {
                // Build remaining path from collected parts (in reverse order)
                let remaining = remaining_parts
                    .into_iter()
                    .rev()
                    .fold(PathBuf::new(), |acc, part| acc.join(part));
                return Some((current, remaining));
            }

            // Get the file name and move up
            if let Some(name) = current.file_name() {
                remaining_parts.push(PathBuf::from(name));
            }

            if let Some(parent) = current.parent() {
                if parent == current {
                    // Reached root without finding existing path
                    break;
                }
                current = parent.to_path_buf();
            } else {
                break;
            }
        }

        None
    }

    /// Validate path and return error if not allowed
    pub fn validate_path(&self, path: &Path) -> Result<()> {
        if !self.is_path_allowed(path) {
            return Err(AetherError::PermissionDenied {
                message: format!("Path not allowed: {}", path.display()),
                suggestion: Some(
                    "Check that the path is within allowed_roots configuration".to_string(),
                ),
            });
        }
        Ok(())
    }

    /// Add an allowed root directory
    pub fn add_allowed_root(&mut self, root: PathBuf) {
        self.allowed_roots.push(root);
    }
}

/// Filesystem tools context
///
/// Provides shared access to filesystem operations and configuration.
/// Used by all filesystem tool implementations.
pub struct FilesystemContext {
    /// File system operations implementation
    pub fs: Arc<dyn FileOps>,
    /// Security configuration
    pub config: Arc<FilesystemConfig>,
}

impl FilesystemContext {
    /// Create a new context with LocalFs implementation
    pub fn new(config: FilesystemConfig) -> Self {
        Self {
            fs: Arc::new(LocalFs::new()),
            config: Arc::new(config),
        }
    }

    /// Create a new context with custom FileOps implementation (for testing)
    pub fn with_fs(fs: Arc<dyn FileOps>, config: FilesystemConfig) -> Self {
        Self {
            fs,
            config: Arc::new(config),
        }
    }

    /// Validate path against security configuration
    pub fn validate_path(&self, path: &Path) -> Result<()> {
        self.config.validate_path(path)
    }

    /// Check if path is allowed
    pub fn is_path_allowed(&self, path: &Path) -> bool {
        self.config.is_path_allowed(path)
    }
}

impl Clone for FilesystemContext {
    fn clone(&self) -> Self {
        Self {
            fs: Arc::clone(&self.fs),
            config: Arc::clone(&self.config),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_config_default_denies_all() {
        let config = FilesystemConfig::default();
        assert!(!config.is_path_allowed(Path::new("/tmp/test.txt")));
        assert!(!config.is_path_allowed(Path::new("/etc/passwd")));
    }

    #[test]
    fn test_config_allows_within_root() {
        let temp_dir = TempDir::new().unwrap();
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "test").unwrap();

        assert!(config.is_path_allowed(&test_file));
        assert!(!config.is_path_allowed(Path::new("/etc/passwd")));
    }

    #[test]
    fn test_config_denies_outside_root() {
        let temp_dir = TempDir::new().unwrap();
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);

        assert!(!config.is_path_allowed(Path::new("/etc/passwd")));
        assert!(!config.is_path_allowed(Path::new("/tmp/other/file.txt")));
    }

    #[test]
    fn test_validate_path_error() {
        let config = FilesystemConfig::default();
        let result = config.validate_path(Path::new("/etc/passwd"));

        assert!(result.is_err());
        match result {
            Err(AetherError::PermissionDenied { message, .. }) => {
                assert!(message.contains("/etc/passwd"));
            }
            _ => panic!("Expected PermissionDenied error"),
        }
    }

    #[test]
    fn test_add_allowed_root() {
        let mut config = FilesystemConfig::default();
        let temp_dir = TempDir::new().unwrap();

        config.add_allowed_root(temp_dir.path().to_path_buf());

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "test").unwrap();

        assert!(config.is_path_allowed(&test_file));
    }
}
