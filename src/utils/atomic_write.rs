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

/// Write UTF-8 `content` atomically to `path`. One-line forwarder to
/// [`atomic_write_bytes`], which holds the actual implementation and its
/// guarantees — read the doc there.
pub async fn atomic_write_file(path: &Path, content: &str) -> Result<(), AlephError> {
    atomic_write_bytes(path, content.as_bytes()).await
}

/// Write `content` atomically to `path` using a temp-file-and-rename strategy.
/// Readers either see the previous complete content or the new complete content,
/// never a half-written file.
///
/// Implementation: write to a randomly-named temp file in the same directory,
/// fsync, then rename. POSIX rename is atomic within a single filesystem,
/// so readers either see the old file or the new file but never a partial write.
///
/// When `path` already exists, its permission bits are copied onto the staging
/// file before the rename — `tempfile` creates the staging file `0600`, so
/// without this an atomic overwrite would silently strip an existing file's
/// mode (e.g. drop the executable bit from a `0755` script). A brand-new file
/// keeps the `0600` staging default.
pub async fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<(), AlephError> {
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

    // Reopen with write access for the durability fsync. Windows'
    // `FlushFileBuffers` (what `sync_all` calls) requires a handle with write
    // access and fails with `ERROR_ACCESS_DENIED` (os error 5) on a read-only
    // handle; POSIX tolerates either, so a write handle is correct on both.
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&tmp_path)
        .await
        .map_err(|e| AlephError::ConfigError {
            message: format!("Failed to open temp file for sync: {e}"),
            suggestion: None,
        })?;
    file.sync_all().await.map_err(|e| AlephError::ConfigError {
        message: format!("Failed to sync temp file: {e}"),
        suggestion: None,
    })?;

    // Preserve the destination's existing permissions across the rename.
    // Best-effort: a failure here must not abort an otherwise-good write.
    if let Ok(meta) = fs::metadata(path).await {
        let _ = fs::set_permissions(&tmp_path, meta.permissions()).await;
    }

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
    async fn atomic_write_bytes_round_trips_binary() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.bin");
        let payload = [0u8, 159, 146, 150, 255]; // not valid UTF-8
        atomic_write_bytes(&p, &payload).await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), payload);
    }

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
        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        let mut files = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            files.push(entry.path());
        }
        assert_eq!(files.len(), 1, "only the final file should remain");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preserves_permissions_on_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let path = dir.path().join("script.sh");
        tokio::fs::write(&path, "#!/bin/sh\necho old\n")
            .await
            .unwrap();
        // Mark the file executable, as a shell script would be.
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .await
            .unwrap();

        atomic_write_file(&path, "#!/bin/sh\necho new\n")
            .await
            .unwrap();

        let mode = tokio::fs::metadata(&path)
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "executable bit must survive the atomic overwrite"
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "#!/bin/sh\necho new\n"
        );
    }
}
