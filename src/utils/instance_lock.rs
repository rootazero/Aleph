//! Cross-process singleton lock for a given Aleph data directory.
//!
//! Uses `fs2::FileExt::try_lock_exclusive` on `<data_dir>/aleph.lock`.
//! The lock is automatically released by the OS when the holder process
//! exits (graceful, panic, SIGKILL — all release).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;

const LOCK_FILENAME: &str = "aleph.lock";

#[derive(Debug)]
pub struct InstanceLock {
    #[allow(dead_code)] // Held for OS-level lock lifetime via Drop on `File`.
    file: File,
    path: PathBuf,
    holder_pid: u32,
}

impl InstanceLock {
    pub fn lock_path(&self) -> &Path {
        &self.path
    }
    pub fn holder_pid(&self) -> u32 {
        self.holder_pid
    }
}

// Drop releases the OS-level fs2 lock automatically when `file` is dropped.

#[derive(Debug)]
pub enum AcquireOutcome {
    Acquired(InstanceLock),
    HeldByLive { pid: i32, lock_path: PathBuf },
    HeldByOrphaned { pid: i32, lock_path: PathBuf },
}

#[derive(Debug)]
pub struct HolderDiagnostic {
    pub pid: i32,
    pub process_alive: bool,
    pub lock_path: PathBuf,
}

/// Attempt to acquire the singleton lock for `data_dir`. Caller must
/// hold the returned `InstanceLock` for as long as exclusive access is
/// required.
pub fn try_acquire(data_dir: &Path) -> std::io::Result<AcquireOutcome> {
    if !data_dir.exists() {
        std::fs::create_dir_all(data_dir)?;
    }
    let lock_path = data_dir.join(LOCK_FILENAME);

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;

    match file.try_lock_exclusive() {
        Ok(()) => {
            // Got the lock — write our PID for diagnostics.
            let pid = std::process::id();
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            writeln!(file, "{pid}")?;
            file.sync_all()?;
            Ok(AcquireOutcome::Acquired(InstanceLock {
                file,
                path: lock_path,
                holder_pid: pid,
            }))
        }
        Err(_) => {
            // Lock is held by someone else. Read PID for diagnostics.
            let mut buf = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut buf)?;
            let pid: i32 = buf.trim().parse().unwrap_or(0);
            if is_process_alive(pid) {
                Ok(AcquireOutcome::HeldByLive { pid, lock_path })
            } else {
                Ok(AcquireOutcome::HeldByOrphaned { pid, lock_path })
            }
        }
    }
}

/// Read holder metadata from the lock file WITHOUT competing for the lock.
/// Returns None if the lock file does not exist.
pub fn diagnose_holder(data_dir: &Path) -> Option<HolderDiagnostic> {
    let lock_path = data_dir.join(LOCK_FILENAME);
    let mut file = std::fs::File::open(&lock_path).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    let pid: i32 = buf.trim().parse().ok()?;
    Some(HolderDiagnostic {
        pid,
        process_alive: is_process_alive(pid),
        lock_path,
    })
}

#[cfg(unix)]
fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: `kill(pid, 0)` only checks process existence + permissions.
    // Returns 0 if process exists, -1 + ESRCH otherwise. No memory effects.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_alive(_pid: i32) -> bool {
    // Best-effort fallback for non-Unix; always assume alive to err on the
    // safe side (caller will fail back to a "lock held" branch).
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_acquire_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = try_acquire(dir.path()).unwrap();
        assert!(matches!(outcome, AcquireOutcome::Acquired(_)));
    }

    #[test]
    fn second_acquire_in_same_process_returns_held_by_live() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = match try_acquire(dir.path()).unwrap() {
            AcquireOutcome::Acquired(g) => g,
            other => panic!("first acquire should succeed, got {:?}", other),
        };
        let second = try_acquire(dir.path()).unwrap();
        match second {
            AcquireOutcome::HeldByLive { pid, .. } => {
                assert_eq!(pid as u32, std::process::id());
            }
            other => panic!("expected HeldByLive, got {:?}", other),
        }
    }

    #[test]
    fn release_then_reacquire_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let first = try_acquire(dir.path()).unwrap();
        match first {
            AcquireOutcome::Acquired(g) => drop(g),
            _ => panic!(),
        }
        let again = try_acquire(dir.path()).unwrap();
        assert!(matches!(again, AcquireOutcome::Acquired(_)));
    }

    #[test]
    fn diagnose_holder_returns_none_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(diagnose_holder(dir.path()).is_none());
    }

    #[test]
    fn diagnose_holder_returns_pid_when_held() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = try_acquire(dir.path()).unwrap();
        let diag = diagnose_holder(dir.path()).expect("file should exist");
        assert_eq!(diag.pid as u32, std::process::id());
        assert!(diag.process_alive);
    }
}
