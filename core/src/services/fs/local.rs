//! Local File System Implementation
//!
//! Provides `LocalFs` which implements `FileOps` using tokio::fs for async operations.

use async_trait::async_trait;
use glob::glob;
use std::path::Path;
use tokio::fs;

use super::{DirEntry, FileOps};
use crate::error::{AetherError, Result};

/// Local file system implementation using tokio::fs
#[derive(Debug, Default, Clone)]
pub struct LocalFs;

impl LocalFs {
    /// Create a new LocalFs instance
    pub fn new() -> Self {
        Self
    }

    /// Get metadata for a path and convert to DirEntry
    async fn path_to_entry(path: &Path) -> Result<DirEntry> {
        let metadata = fs::metadata(path).await.map_err(|e| {
            AetherError::IoError(format!("Failed to get metadata for {:?}: {}", path, e))
        })?;

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);

        Ok(DirEntry {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: path.to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
            modified,
        })
    }
}

#[async_trait]
impl FileOps for LocalFs {
    async fn read_file(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AetherError::NotFound(path.to_string_lossy().to_string())
            } else {
                AetherError::IoError(format!("Failed to read file {:?}: {}", path, e))
            }
        })
    }

    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>> {
        fs::read(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AetherError::NotFound(path.to_string_lossy().to_string())
            } else {
                AetherError::IoError(format!("Failed to read file {:?}: {}", path, e))
            }
        })
    }

    async fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    AetherError::IoError(format!(
                        "Failed to create parent directories for {:?}: {}",
                        path, e
                    ))
                })?;
            }
        }

        fs::write(path, content)
            .await
            .map_err(|e| AetherError::IoError(format!("Failed to write file {:?}: {}", path, e)))
    }

    async fn write_file_bytes(&self, path: &Path, content: &[u8]) -> Result<()> {
        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    AetherError::IoError(format!(
                        "Failed to create parent directories for {:?}: {}",
                        path, e
                    ))
                })?;
            }
        }

        fs::write(path, content)
            .await
            .map_err(|e| AetherError::IoError(format!("Failed to write file {:?}: {}", path, e)))
    }

    async fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AetherError::NotFound(path.to_string_lossy().to_string())
            } else {
                AetherError::IoError(format!("Failed to read directory {:?}: {}", path, e))
            }
        })?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| AetherError::IoError(format!("Failed to read directory entry: {}", e)))?
        {
            let entry_path = entry.path();
            match Self::path_to_entry(&entry_path).await {
                Ok(dir_entry) => entries.push(dir_entry),
                Err(e) => {
                    tracing::warn!("Skipping entry {:?}: {}", entry_path, e);
                }
            }
        }

        // Sort by name for consistent ordering
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    async fn exists(&self, path: &Path) -> Result<bool> {
        Ok(fs::try_exists(path).await.unwrap_or(false))
    }

    async fn is_dir(&self, path: &Path) -> Result<bool> {
        match fs::metadata(path).await {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(_) => Ok(false),
        }
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).await.map_err(|e| {
            AetherError::IoError(format!("Failed to create directory {:?}: {}", path, e))
        })
    }

    async fn delete(&self, path: &Path) -> Result<()> {
        let metadata = fs::metadata(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AetherError::NotFound(path.to_string_lossy().to_string())
            } else {
                AetherError::IoError(format!("Failed to get metadata for {:?}: {}", path, e))
            }
        })?;

        if metadata.is_dir() {
            fs::remove_dir_all(path).await
        } else {
            fs::remove_file(path).await
        }
        .map_err(|e| AetherError::IoError(format!("Failed to delete {:?}: {}", path, e)))
    }

    async fn search(&self, base: &Path, pattern: &str) -> Result<Vec<DirEntry>> {
        // Construct full glob pattern
        let full_pattern = base.join(pattern);
        let pattern_str = full_pattern.to_string_lossy().to_string();

        // glob is synchronous, so we wrap it in spawn_blocking
        let entries = tokio::task::spawn_blocking(move || -> Result<Vec<DirEntry>> {
            let mut results = Vec::new();
            let paths = glob(&pattern_str)
                .map_err(|e| AetherError::IoError(format!("Invalid glob pattern: {}", e)))?;

            for path_result in paths {
                match path_result {
                    Ok(path) => {
                        // Get metadata synchronously since we're already in blocking context
                        if let Ok(metadata) = std::fs::metadata(&path) {
                            let modified = metadata
                                .modified()
                                .ok()
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs() as i64);

                            results.push(DirEntry {
                                name: path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default(),
                                path: path.to_string_lossy().to_string(),
                                is_dir: metadata.is_dir(),
                                size: if metadata.is_file() {
                                    metadata.len()
                                } else {
                                    0
                                },
                                modified,
                            });
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Glob match error: {}", e);
                    }
                }
            }

            // Sort by modification time (newest first) then by name
            results.sort_by(|a, b| match (b.modified, a.modified) {
                (Some(b_mod), Some(a_mod)) => b_mod.cmp(&a_mod),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.name.cmp(&b.name),
            });

            Ok(results)
        })
        .await
        .map_err(|e| AetherError::IoError(format!("Task join error: {}", e)))??;

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_local_fs_write_and_read() {
        let temp_dir = TempDir::new().unwrap();
        let fs = LocalFs::new();

        let file_path = temp_dir.path().join("test.txt");
        fs.write_file(&file_path, "Hello, World!").await.unwrap();

        let content = fs.read_file(&file_path).await.unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_local_fs_exists() {
        let temp_dir = TempDir::new().unwrap();
        let fs = LocalFs::new();

        let file_path = temp_dir.path().join("exists.txt");
        assert!(!fs.exists(&file_path).await.unwrap());

        fs.write_file(&file_path, "test").await.unwrap();
        assert!(fs.exists(&file_path).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_fs_list_dir() {
        let temp_dir = TempDir::new().unwrap();
        let fs = LocalFs::new();

        // Create some files
        fs.write_file(&temp_dir.path().join("a.txt"), "a")
            .await
            .unwrap();
        fs.write_file(&temp_dir.path().join("b.txt"), "b")
            .await
            .unwrap();
        fs.create_dir(&temp_dir.path().join("subdir"))
            .await
            .unwrap();

        let entries = fs.list_dir(temp_dir.path()).await.unwrap();
        assert_eq!(entries.len(), 3);

        // Should be sorted by name
        assert_eq!(entries[0].name, "a.txt");
        assert_eq!(entries[1].name, "b.txt");
        assert_eq!(entries[2].name, "subdir");
        assert!(entries[2].is_dir);
    }

    #[tokio::test]
    async fn test_local_fs_delete() {
        let temp_dir = TempDir::new().unwrap();
        let fs = LocalFs::new();

        // Delete file
        let file_path = temp_dir.path().join("to_delete.txt");
        fs.write_file(&file_path, "delete me").await.unwrap();
        assert!(fs.exists(&file_path).await.unwrap());
        fs.delete(&file_path).await.unwrap();
        assert!(!fs.exists(&file_path).await.unwrap());

        // Delete directory
        let dir_path = temp_dir.path().join("to_delete_dir");
        fs.create_dir(&dir_path).await.unwrap();
        fs.write_file(&dir_path.join("file.txt"), "nested")
            .await
            .unwrap();
        fs.delete(&dir_path).await.unwrap();
        assert!(!fs.exists(&dir_path).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_fs_search() {
        let temp_dir = TempDir::new().unwrap();
        let fs = LocalFs::new();

        // Create test files
        fs.write_file(&temp_dir.path().join("file1.txt"), "1")
            .await
            .unwrap();
        fs.write_file(&temp_dir.path().join("file2.txt"), "2")
            .await
            .unwrap();
        fs.write_file(&temp_dir.path().join("other.rs"), "rs")
            .await
            .unwrap();

        // Search for .txt files
        let results = fs.search(temp_dir.path(), "*.txt").await.unwrap();
        assert_eq!(results.len(), 2);

        // Search for .rs files
        let results = fs.search(temp_dir.path(), "*.rs").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "other.rs");
    }
}
