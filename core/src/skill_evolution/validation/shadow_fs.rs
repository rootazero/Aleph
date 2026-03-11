//! Shadow Filesystem — read from source, write to overlay.
//!
//! Provides an isolated filesystem view where all reads transparently
//! proxy to the original workspace (read-only) and all writes redirect
//! to a temporary overlay directory.

use std::path::{Path, PathBuf};
use tokio::fs;
use anyhow::Result;

/// Shadow filesystem: reads from source, writes to overlay.
pub struct ShadowFs {
    source_dir: PathBuf,
    overlay_dir: PathBuf,
}

impl ShadowFs {
    /// Create a new shadow filesystem.
    pub fn new(source_dir: PathBuf, overlay_dir: PathBuf) -> Self {
        Self {
            source_dir,
            overlay_dir,
        }
    }

    /// Resolve a relative path for reading. Checks overlay first, then source.
    pub fn resolve_read(&self, relative: &Path) -> PathBuf {
        let overlay_path = self.overlay_dir.join(relative);
        if overlay_path.exists() {
            overlay_path
        } else {
            self.source_dir.join(relative)
        }
    }

    /// Resolve a relative path for writing. Always goes to overlay.
    pub fn resolve_write(&self, relative: &Path) -> PathBuf {
        self.overlay_dir.join(relative)
    }

    /// Read a file through the shadow FS.
    pub async fn read(&self, relative: &Path) -> Result<String> {
        let path = self.resolve_read(relative);
        Ok(fs::read_to_string(&path).await?)
    }

    /// Write a file through the shadow FS (always to overlay).
    pub async fn write(&self, relative: &Path, content: &str) -> Result<()> {
        let path = self.resolve_write(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, content).await?;
        Ok(())
    }

    /// List files modified in the overlay.
    pub async fn modified_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        Self::collect_files(&self.overlay_dir, &self.overlay_dir, &mut files).await?;
        Ok(files)
    }

    /// Source directory (read-only workspace).
    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    /// Overlay directory (writable sandbox).
    pub fn overlay_dir(&self) -> &Path {
        &self.overlay_dir
    }

    /// Recursively collect relative file paths under a directory.
    async fn collect_files(
        base: &Path,
        dir: &Path,
        out: &mut Vec<PathBuf>,
    ) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                Box::pin(Self::collect_files(base, &path, out)).await?;
            } else {
                let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
                out.push(relative);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_from_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        fs::write(source.join("test.txt"), "from source").await.unwrap();

        let sfs = ShadowFs::new(source, overlay);
        let content = sfs.read(Path::new("test.txt")).await.unwrap();
        assert_eq!(content, "from source");
    }

    #[tokio::test]
    async fn write_goes_to_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let sfs = ShadowFs::new(source.clone(), overlay.clone());
        sfs.write(Path::new("new.txt"), "written data").await.unwrap();

        // File should be in overlay, not source
        assert!(overlay.join("new.txt").exists());
        assert!(!source.join("new.txt").exists());
    }

    #[tokio::test]
    async fn overlay_overrides_source() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        fs::write(source.join("file.txt"), "original").await.unwrap();
        fs::write(overlay.join("file.txt"), "modified").await.unwrap();

        let sfs = ShadowFs::new(source, overlay);
        let content = sfs.read(Path::new("file.txt")).await.unwrap();
        assert_eq!(content, "modified");
    }

    #[tokio::test]
    async fn modified_files_lists_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let overlay = tmp.path().join("overlay");
        fs::create_dir_all(&source).await.unwrap();
        fs::create_dir_all(&overlay).await.unwrap();

        let sfs = ShadowFs::new(source, overlay);
        sfs.write(Path::new("a.txt"), "a").await.unwrap();
        sfs.write(Path::new("sub/b.txt"), "b").await.unwrap();

        let files = sfs.modified_files().await.unwrap();
        assert_eq!(files.len(), 2);
    }
}
