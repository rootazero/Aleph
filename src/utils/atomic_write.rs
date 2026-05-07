//! Atomic file write via temp + rename.
//!
//! On POSIX (Linux, macOS) `rename(2)` over an existing target on the same
//! filesystem is atomic — readers see either the previous complete content
//! or the new complete content, never a partial write. On Windows the
//! replacement is best-effort and may briefly be observable as a missing
//! target. The temp file is created alongside `path` so the rename never
//! crosses a filesystem boundary.

use crate::error::AlephError;
use std::path::Path;
use tokio::fs;

/// Write `content` atomically to `path` using a temp-file-and-rename strategy.
/// Readers either see the previous complete content or the new complete content,
/// never a half-written file.
///
/// Implementation: write to a randomly-named temp file in the same directory,
/// fsync, then rename. POSIX rename is atomic within a single filesystem,
/// so readers either see the old file or the new file but never a partial write.
pub async fn atomic_write_file(path: &Path, content: &str) -> Result<(), AlephError> {
    let parent = path.parent().ok_or_else(|| AlephError::ConfigError {
        message: format!("Path has no parent directory: {path:?}"),
        suggestion: None,
    })?;

    let tmp = tempfile::Builder::new()
        .prefix(".aleph_atomic_")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|e| AlephError::ConfigError {
            message: format!("Failed to create temp file: {e}"),
            suggestion: None,
        })?;
    let tmp_path = tmp.path().to_path_buf();

    fs::write(&tmp_path, content)
        .await
        .map_err(|e| AlephError::ConfigError {
            message: format!("Failed to write {tmp_path:?}: {e}"),
            suggestion: None,
        })?;

    let file = fs::File::open(&tmp_path)
        .await
        .map_err(|e| AlephError::ConfigError {
            message: format!("Failed to open temp file for sync: {e}"),
            suggestion: None,
        })?;
    file.sync_all().await.map_err(|e| AlephError::ConfigError {
        message: format!("Failed to sync temp file: {e}"),
        suggestion: None,
    })?;

    fs::rename(&tmp_path, path)
        .await
        .map_err(|e| AlephError::ConfigError {
            message: format!("Failed to rename {tmp_path:?} -> {path:?}: {e}"),
            suggestion: None,
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn writes_content_atomically() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("foo.md");
        atomic_write_file(&path, "hello world").await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "hello world"
        );
    }

    #[tokio::test]
    async fn overwrites_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("foo.md");
        tokio::fs::write(&path, "old").await.unwrap();
        atomic_write_file(&path, "new").await.unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "new");
    }

    #[tokio::test]
    async fn no_temp_files_left_on_success() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("foo.md");
        atomic_write_file(&path, "hi").await.unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1, "only the final file should remain");
    }
}
