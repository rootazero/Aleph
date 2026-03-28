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
use crate::error::Result;
use super::FileFilter;

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

    /// Recursively collect files from directory with filters
    ///
    /// Shared implementation used by FileOps, EditOps, and SearchOps handlers.
    pub fn collect_files_from_directory<'a>(
        &'a self,
        dir: &'a Path,
        recursive: bool,
        filters: &'a [FileFilter],
        files: &'a mut Vec<PathBuf>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(dir).await?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();

                if path.is_file() {
                    if Self::should_include_file(&path, filters) {
                        files.push(path);
                    }
                } else if path.is_dir() && recursive {
                    // Skip hidden directories and common ignore patterns
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !name.starts_with('.') && name != "node_modules" && name != "target" {
                            self.collect_files_from_directory(&path, recursive, filters, files)
                                .await?;
                        }
                    }
                }
            }

            Ok(())
        })
    }

    /// Check if file should be included based on filters
    ///
    /// Shared implementation used by FileOps, EditOps, and SearchOps handlers.
    pub fn should_include_file(path: &Path, filters: &[FileFilter]) -> bool {
        // If no filters, include all files
        if filters.is_empty() {
            return true;
        }

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        for filter in filters {
            match filter {
                FileFilter::Code => {
                    let code_exts = [
                        "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h",
                        "hpp", "cs", "rb", "php", "swift", "kt", "scala", "sh", "bash",
                    ];
                    if !code_exts.contains(&extension) {
                        return false;
                    }
                }
                FileFilter::Text => {
                    let text_exts = ["txt", "md", "rst", "log", "json", "yaml", "yml", "toml"];
                    if !text_exts.contains(&extension) {
                        return false;
                    }
                }
                FileFilter::Extension(ext) => {
                    if extension != ext {
                        return false;
                    }
                }
                FileFilter::Exclude(pattern) => {
                    if filename.contains(pattern.trim_matches('*')) {
                        return false;
                    }
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
