//! Memory File Locking
//!
//! Provides file-based locking for safe concurrent access to memory database.
//! Uses shared locks for read operations and exclusive locks for writes.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;

/// Lock mode for memory operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// Shared lock - allows multiple readers
    Read,
    /// Exclusive lock - single writer, no readers
    Write,
}

/// Error types for locking operations
#[derive(Debug)]
pub enum LockError {
    /// Failed to acquire read lock
    ReadLockFailed,
    /// Failed to acquire write lock - Gateway may be running
    WriteLockFailed {
        hint: String,
    },
    /// IO error
    IoError(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::ReadLockFailed => write!(f, "Failed to acquire read lock"),
            LockError::WriteLockFailed { hint } => {
                write!(f, "Failed to acquire write lock: {}", hint)
            }
            LockError::IoError(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for LockError {}

impl From<std::io::Error> for LockError {
    fn from(err: std::io::Error) -> Self {
        LockError::IoError(err)
    }
}

/// Memory lock guard
///
/// Automatically releases the lock when dropped.
pub struct MemoryLock {
    _lock_file: File,
    mode: LockMode,
    path: PathBuf,
}

impl MemoryLock {
    /// Acquire a lock on the memory database
    ///
    /// # Arguments
    /// * `mode` - Read or Write lock
    /// * `lock_path` - Path to the lock file (typically ~/.aleph/memory.lock)
    ///
    /// # Returns
    /// * `Ok(MemoryLock)` - Lock acquired successfully
    /// * `Err(LockError)` - Failed to acquire lock
    pub fn acquire(mode: LockMode, lock_path: PathBuf) -> Result<Self, LockError> {
        // Ensure parent directory exists
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;

        // Try to acquire the appropriate lock
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = lock_file.as_raw_fd();

            let lock_type = match mode {
                LockMode::Read => libc::LOCK_SH | libc::LOCK_NB,
                LockMode::Write => libc::LOCK_EX | libc::LOCK_NB,
            };

            let result = unsafe { libc::flock(fd, lock_type) };

            if result != 0 {
                return match mode {
                    LockMode::Read => Err(LockError::ReadLockFailed),
                    LockMode::Write => Err(LockError::WriteLockFailed {
                        hint: "Gateway may be running. Use read-only commands (list, show, status) \
                               or stop Gateway first."
                            .into(),
                    }),
                };
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            // On Windows, use exclusive sharing mode for writes
            if mode == LockMode::Write {
                // Try to get exclusive access by reopening
                drop(lock_file);
                let lock_file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .share_mode(0) // Exclusive
                    .open(&lock_path)
                    .map_err(|_| LockError::WriteLockFailed {
                        hint: "Gateway may be running.".into(),
                    })?;

                return Ok(Self {
                    _lock_file: lock_file,
                    mode,
                    path: lock_path,
                });
            }
        }

        Ok(Self {
            _lock_file: lock_file,
            mode,
            path: lock_path,
        })
    }

    /// Acquire a lock using the default lock path (~/.aleph/memory.lock)
    pub fn acquire_default(mode: LockMode) -> Result<Self, LockError> {
        let lock_path = Self::default_lock_path()?;
        Self::acquire(mode, lock_path)
    }

    /// Get the default lock path
    pub fn default_lock_path() -> Result<PathBuf, LockError> {
        let home = dirs::home_dir().ok_or_else(|| {
            LockError::IoError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not determine home directory",
            ))
        })?;

        Ok(home.join(".aleph").join("memory.lock"))
    }

    /// Get the lock mode
    pub fn mode(&self) -> LockMode {
        self.mode
    }

    /// Get the lock file path
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for MemoryLock {
    fn drop(&mut self) {
        // Lock is automatically released when file is closed
        // The file handle in _lock_file will be dropped, releasing the lock
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self._lock_file.as_raw_fd();
            unsafe { libc::flock(fd, libc::LOCK_UN) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_acquire_read_lock() {
        let temp = tempdir().unwrap();
        let lock_path = temp.path().join("test.lock");

        let lock = MemoryLock::acquire(LockMode::Read, lock_path.clone());
        assert!(lock.is_ok());
        assert_eq!(lock.unwrap().mode(), LockMode::Read);
    }

    #[test]
    fn test_acquire_write_lock() {
        let temp = tempdir().unwrap();
        let lock_path = temp.path().join("test.lock");

        let lock = MemoryLock::acquire(LockMode::Write, lock_path.clone());
        assert!(lock.is_ok());
        assert_eq!(lock.unwrap().mode(), LockMode::Write);
    }

    #[test]
    fn test_multiple_read_locks() {
        let temp = tempdir().unwrap();
        let lock_path = temp.path().join("test.lock");

        let lock1 = MemoryLock::acquire(LockMode::Read, lock_path.clone());
        assert!(lock1.is_ok());

        // Second read lock should succeed
        let lock2 = MemoryLock::acquire(LockMode::Read, lock_path.clone());
        assert!(lock2.is_ok());
    }

    #[test]
    fn test_write_lock_blocks_write() {
        let temp = tempdir().unwrap();
        let lock_path = temp.path().join("test.lock");

        let _lock1 = MemoryLock::acquire(LockMode::Write, lock_path.clone()).unwrap();

        // Second write lock should fail
        let lock2 = MemoryLock::acquire(LockMode::Write, lock_path.clone());
        assert!(lock2.is_err());
    }

    #[test]
    fn test_lock_released_on_drop() {
        let temp = tempdir().unwrap();
        let lock_path = temp.path().join("test.lock");

        {
            let _lock = MemoryLock::acquire(LockMode::Write, lock_path.clone()).unwrap();
            // Lock held here
        }
        // Lock released after drop

        // Should be able to acquire again
        let lock = MemoryLock::acquire(LockMode::Write, lock_path.clone());
        assert!(lock.is_ok());
    }
}
