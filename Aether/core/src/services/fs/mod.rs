//! File System Operations Module
//!
//! Provides async file system operations through the `FileOps` trait.
//! The `LocalFs` implementation uses tokio::fs for non-blocking operations.

mod local;

pub use local::LocalFs;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::Result;

/// Directory entry information returned by list_dir and search operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    /// File or directory name
    pub name: String,
    /// Full path to the entry
    pub path: String,
    /// Whether this entry is a directory
    pub is_dir: bool,
    /// File size in bytes (0 for directories)
    pub size: u64,
    /// Modification time as Unix timestamp (optional)
    pub modified: Option<i64>,
}

/// Trait for file system operations
///
/// This trait provides a unified interface for file operations that can be
/// implemented by different backends (local filesystem, remote, mock for testing).
#[async_trait]
pub trait FileOps: Send + Sync {
    /// Read file contents as UTF-8 string
    ///
    /// # Arguments
    /// * `path` - Path to the file to read
    ///
    /// # Returns
    /// File contents as string, or error if file doesn't exist or isn't valid UTF-8
    async fn read_file(&self, path: &Path) -> Result<String>;

    /// Read file contents as raw bytes
    ///
    /// # Arguments
    /// * `path` - Path to the file to read
    ///
    /// # Returns
    /// File contents as byte vector, or error if file doesn't exist
    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>>;

    /// Write string content to file
    ///
    /// # Arguments
    /// * `path` - Path to the file to write
    /// * `content` - String content to write
    ///
    /// # Returns
    /// Ok(()) on success, error if write fails
    async fn write_file(&self, path: &Path, content: &str) -> Result<()>;

    /// Write raw bytes to file
    ///
    /// # Arguments
    /// * `path` - Path to the file to write
    /// * `content` - Byte content to write
    ///
    /// # Returns
    /// Ok(()) on success, error if write fails
    async fn write_file_bytes(&self, path: &Path, content: &[u8]) -> Result<()>;

    /// List directory contents
    ///
    /// # Arguments
    /// * `path` - Path to the directory to list
    ///
    /// # Returns
    /// Vector of directory entries, or error if path doesn't exist or isn't a directory
    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>>;

    /// Check if path exists
    ///
    /// # Arguments
    /// * `path` - Path to check
    ///
    /// # Returns
    /// true if path exists, false otherwise
    async fn exists(&self, path: &Path) -> Result<bool>;

    /// Check if path is a directory
    ///
    /// # Arguments
    /// * `path` - Path to check
    ///
    /// # Returns
    /// true if path is a directory, false otherwise
    async fn is_dir(&self, path: &Path) -> Result<bool>;

    /// Create directory and all parent directories
    ///
    /// # Arguments
    /// * `path` - Path of directory to create
    ///
    /// # Returns
    /// Ok(()) on success, error if creation fails
    async fn create_dir(&self, path: &Path) -> Result<()>;

    /// Delete file or directory
    ///
    /// # Arguments
    /// * `path` - Path to delete
    ///
    /// # Returns
    /// Ok(()) on success, error if deletion fails
    async fn delete(&self, path: &Path) -> Result<()>;

    /// Search for files matching a glob pattern
    ///
    /// # Arguments
    /// * `base` - Base directory to search from
    /// * `pattern` - Glob pattern (e.g., "**/*.rs", "*.txt")
    ///
    /// # Returns
    /// Vector of matching directory entries
    async fn search(&self, base: &Path, pattern: &str) -> Result<Vec<DirEntry>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Mock implementation for testing
    pub struct MockFs {
        pub files: std::collections::HashMap<String, String>,
    }

    #[async_trait]
    impl FileOps for MockFs {
        async fn read_file(&self, path: &Path) -> Result<String> {
            self.files
                .get(path.to_string_lossy().as_ref())
                .cloned()
                .ok_or_else(|| crate::error::AetherError::NotFound(path.to_string_lossy().to_string()))
        }

        async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>> {
            self.read_file(path).await.map(|s| s.into_bytes())
        }

        async fn write_file(&self, _path: &Path, _content: &str) -> Result<()> {
            Ok(())
        }

        async fn write_file_bytes(&self, _path: &Path, _content: &[u8]) -> Result<()> {
            Ok(())
        }

        async fn list_dir(&self, _path: &Path) -> Result<Vec<DirEntry>> {
            Ok(vec![])
        }

        async fn exists(&self, path: &Path) -> Result<bool> {
            Ok(self.files.contains_key(path.to_string_lossy().as_ref()))
        }

        async fn is_dir(&self, _path: &Path) -> Result<bool> {
            Ok(false)
        }

        async fn create_dir(&self, _path: &Path) -> Result<()> {
            Ok(())
        }

        async fn delete(&self, _path: &Path) -> Result<()> {
            Ok(())
        }

        async fn search(&self, _base: &Path, _pattern: &str) -> Result<Vec<DirEntry>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn test_mock_fs_read() {
        let mut files = std::collections::HashMap::new();
        files.insert("/test.txt".to_string(), "Hello, World!".to_string());
        let fs: Arc<dyn FileOps> = Arc::new(MockFs { files });

        let content = fs.read_file(Path::new("/test.txt")).await.unwrap();
        assert_eq!(content, "Hello, World!");
    }
}
