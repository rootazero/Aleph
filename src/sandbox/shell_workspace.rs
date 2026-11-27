//! One directory identity for fixed-input admission, mount and revocation.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Keep the launch directory open to prevent its inode from being reused.
pub(crate) struct ShellWorkspace {
    path: PathBuf,
    directory: File,
}

impl ShellWorkspace {
    pub(crate) fn pin(path: &Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let directory = Self::open(&path)?;
        Ok(Self { path, directory })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn directory(&self) -> &File {
        &self.directory
    }

    pub(crate) fn is_current(&self, live_path: &Path) -> io::Result<bool> {
        let path = live_path.canonicalize()?;
        if path != self.path {
            return Ok(false);
        }
        let pinned = self.directory.metadata()?;
        let current = Self::open(&path)?.metadata()?;
        Ok((pinned.dev(), pinned.ino()) == (current.dev(), current.ino()))
    }

    fn open(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
    }
}
