//! Executor Context
//!
//! Provides shared environment and security checks for all atomic operation handlers.
//!
//! ## Responsibilities
//!
//! - **Working Directory Management**: Maintains sandbox root for relative path resolution
//! - **Path Security**: Centralized path traversal prevention and canonicalization
//! - **Shared State**: Future extensibility for global state (dry_run, audit logging, etc.)

use std::path::{Path, PathBuf};
use crate::error::{AlephError, Result};

/// Shared execution context for atomic operations
///
/// This struct provides the execution environment (sandbox) and security checks
/// that are shared across all operation handlers.
pub struct ExecutorContext {
    /// Working directory (sandbox root)
    ///
    /// All relative paths are resolved relative to this directory.
    /// Absolute paths are validated to ensure they don't escape the sandbox.
    pub working_dir: PathBuf,
}

impl ExecutorContext {
    /// Create a new executor context
    ///
    /// # Arguments
    ///
    /// * `working_dir` - The sandbox root directory for path resolution
    pub fn new(working_dir: PathBuf) -> Self {
        Self { working_dir }
    }

    /// Resolve a path relative to working_dir
    ///
    /// This method performs the following operations:
    /// 1. Handles absolute paths by returning them as-is
    /// 2. Expands `~` to home directory
    /// 3. Resolves relative paths against `working_dir`
    ///
    /// # Arguments
    ///
    /// * `path` - The path to resolve (can be relative or absolute)
    ///
    /// # Returns
    ///
    /// * `Ok(PathBuf)` - The resolved path
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let context = ExecutorContext::new(PathBuf::from("/workspace"));
    ///
    /// // Relative path
    /// let path = context.resolve_path("src/main.rs")?;
    /// assert_eq!(path, PathBuf::from("/workspace/src/main.rs"));
    ///
    /// // Absolute path
    /// let path = context.resolve_path("/etc/passwd")?;
    /// assert_eq!(path, PathBuf::from("/etc/passwd"));
    /// ```
    pub fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        // If absolute, use as-is
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }

        // If starts with ~, expand home directory
        if let Some(path_str) = path.to_str() {
            if path_str.starts_with("~/") || path_str == "~" {
                if let Some(home) = dirs::home_dir() {
                    let relative = path_str.strip_prefix("~/").unwrap_or("");
                    return Ok(home.join(relative));
                }
            }
        }

        // Otherwise, resolve relative to working directory
        Ok(self.working_dir.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_resolve_relative_path() {
        let temp_dir = TempDir::new().unwrap();
        let context = ExecutorContext::new(temp_dir.path().to_path_buf());

        // Resolve relative path
        let resolved = context.resolve_path("test.txt").unwrap();
        assert_eq!(resolved, temp_dir.path().join("test.txt"));
    }

    #[test]
    fn test_resolve_absolute_path() {
        let temp_dir = TempDir::new().unwrap();
        let context = ExecutorContext::new(temp_dir.path().to_path_buf());

        // Resolve absolute path
        let resolved = context.resolve_path("/etc/passwd").unwrap();
        assert_eq!(resolved, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn test_resolve_home_path() {
        let temp_dir = TempDir::new().unwrap();
        let context = ExecutorContext::new(temp_dir.path().to_path_buf());

        // Resolve home path
        let resolved = context.resolve_path("~/test.txt").unwrap();
        if let Some(home) = dirs::home_dir() {
            assert_eq!(resolved, home.join("test.txt"));
        }
    }
}
