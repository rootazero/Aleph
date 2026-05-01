//! Atomic file write via temp + rename. Cross-process safe.

use crate::error::AlephError;
use std::path::{Path, PathBuf};
use tokio::fs;

/// Write `content` atomically to `path` using a temp-file-and-rename strategy.
/// Readers either see the previous complete content or the new complete content,
/// never a half-written file.
///
/// Implementation: write to `<path>.tmp` first, then rename. POSIX rename is
/// atomic within a single filesystem, so readers either see the old file or
/// the new file but never a partial write.
pub async fn atomic_write_file(path: &Path, content: &str) -> Result<(), AlephError> {
    let mut tmp_os = path.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp = PathBuf::from(tmp_os);
    fs::write(&tmp, content)
        .await
        .map_err(|e| AlephError::ConfigError {
            message: format!("Failed to write {tmp:?}: {e}"),
            suggestion: None,
        })?;
    fs::rename(&tmp, path)
        .await
        .map_err(|e| AlephError::ConfigError {
            message: format!("Failed to rename {tmp:?} -> {path:?}: {e}"),
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
