//! Atomic + locked I/O wrapper around `secrets.vault`.
//!
//! Defense-in-depth: even if the singleton lock is bypassed, every
//! vault write here serialises via fs2 fcntl on `secrets.vault.lock`
//! and writes through atomic temp+rename so readers always see either
//! the complete old or complete new file.

use std::path::{Path, PathBuf};

use crate::utils::atomic_io::{with_file_lock, write_atomic};

const VAULT_FILENAME: &str = "secrets.vault";

pub struct VaultIo {
    path: PathBuf,
    lock_path: PathBuf,
}

impl VaultIo {
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self::new_with_path(data_dir.join(VAULT_FILENAME))
    }

    #[must_use]
    pub fn new_with_path(path: PathBuf) -> Self {
        let lock_path = {
            let mut p = path.as_os_str().to_owned();
            p.push(".lock");
            PathBuf::from(p)
        };
        Self { path, lock_path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns `Ok(None)` if the vault file does not yet exist.
    pub fn read(&self) -> std::io::Result<Option<Vec<u8>>> {
        with_file_lock(&self.lock_path, |_| match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        })
    }

    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        with_file_lock(&self.lock_path, |_| write_atomic(&self.path, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_returns_none_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let io = VaultIo::new(dir.path());
        assert!(io.read().unwrap().is_none());
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let io = VaultIo::new(dir.path());
        io.write(b"payload").unwrap();
        assert_eq!(io.read().unwrap().as_deref(), Some(b"payload" as &[u8]));
    }

    #[test]
    fn overwrite_keeps_only_one_data_file() {
        let dir = tempfile::tempdir().unwrap();
        let io = VaultIo::new(dir.path());
        io.write(b"one").unwrap();
        io.write(b"two").unwrap();
        let files: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with(".aleph_atomic_"))
            .collect();
        assert!(files.contains(&"secrets.vault".to_string()));
        assert!(files.contains(&"secrets.vault.lock".to_string()));
        assert_eq!(files.len(), 2);
    }
}
