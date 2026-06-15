//! Cross-process singleton lock for a given Aleph data directory.
//!
//! Uses `fs2::FileExt::try_lock_exclusive` on `<data_dir>/aleph.lock`.
//! The lock is automatically released by the OS when the holder process
//! exits (graceful, panic, SIGKILL — all release).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::process_alive::{process_matches, process_start_time};

const LOCK_FILENAME: &str = "aleph.lock";

#[derive(Debug)]
pub struct InstanceLock {
    #[allow(dead_code)] // Held for OS-level lock lifetime via Drop on `File`.
    file: File,
    path: PathBuf,
    holder_pid: u32,
}

impl InstanceLock {
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub const fn holder_pid(&self) -> u32 {
        self.holder_pid
    }

    /// Consume the lock and return the underlying file handle. The OS-level
    /// fs2 lock is released only when this `File` is dropped.
    #[must_use]
    pub fn into_file(self) -> File {
        self.file
    }

    /// Rewrite the lock file's holder PID to the *current* process id.
    ///
    /// Call this after `fork()`/daemonization: the flock is held on a fd that
    /// survives `fork()`, so the daemonized grandchild still owns the lock —
    /// but the lock file *content* still names the original (now-exited) parent
    /// PID that called `try_acquire`. Without this, `diagnose_holder` and the
    /// PID readback in `try_acquire` mistake the live daemon for a stale /
    /// orphaned lock, and a second `start` can print "safe to `rm`" advice for
    /// a lock that is in fact held by a running process.
    pub fn rewrite_holder_pid(&mut self) -> std::io::Result<()> {
        let pid = std::process::id();
        write_holder(&mut self.file, pid)?;
        self.holder_pid = pid;
        Ok(())
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
            // Got the lock — record our PID (+ start time) for diagnostics.
            let pid = std::process::id();
            write_holder(&mut file, pid)?;
            Ok(AcquireOutcome::Acquired(InstanceLock {
                file,
                path: lock_path,
                holder_pid: pid,
            }))
        }
        Err(_) => {
            // Lock is held by someone else. Read the holder for diagnostics.
            let mut buf = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut buf)?;
            let (pid, expected_start) = parse_holder(&buf);
            if pid > 0 && process_matches(pid, expected_start) {
                Ok(AcquireOutcome::HeldByLive { pid, lock_path })
            } else if pid > 0 {
                Ok(AcquireOutcome::HeldByOrphaned { pid, lock_path })
            } else {
                Ok(AcquireOutcome::HeldByLive { pid: 0, lock_path })
            }
        }
    }
}

/// Read holder metadata from the lock file WITHOUT competing for the lock.
/// Returns None if the lock file does not exist.
#[must_use]
pub fn diagnose_holder(data_dir: &Path) -> Option<HolderDiagnostic> {
    let lock_path = data_dir.join(LOCK_FILENAME);
    let mut file = std::fs::File::open(&lock_path).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    if buf.trim().is_empty() {
        return None;
    }
    let (pid, expected_start) = parse_holder(&buf);
    Some(HolderDiagnostic {
        pid,
        process_alive: process_matches(pid, expected_start),
        lock_path,
    })
}

/// Write the holder record: line 1 = PID, line 2 = the holder's process start
/// time (seconds since epoch) when the platform reports one. The start time is
/// what makes the live/orphaned classification immune to PID reuse — a recycled
/// PID has a different start time. When unavailable we write the PID alone,
/// keeping the legacy single-line format.
fn write_holder(file: &mut File, pid: u32) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    match process_start_time(pid as i32) {
        Some(start) => writeln!(file, "{pid}\n{start}")?,
        None => writeln!(file, "{pid}")?,
    }
    file.sync_all()?;
    Ok(())
}

/// Parse a holder record written by [`write_holder`]. Line 1 is the PID; an
/// optional line 2 is the recorded start time. Legacy single-line files yield
/// `start = None`, which makes [`process_matches`] fall back to a bare liveness
/// check. A bad PID line yields `-1` (treated as "no live holder").
fn parse_holder(buf: &str) -> (i32, Option<u64>) {
    let mut lines = buf.lines();
    let pid = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(-1);
    let start = lines.next().and_then(|l| l.trim().parse().ok());
    (pid, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_holder_reads_pid_and_optional_start() {
        assert_eq!(parse_holder("123\n456\n"), (123, Some(456)));
        assert_eq!(parse_holder("123\n"), (123, None)); // legacy single-line
        assert_eq!(parse_holder("123"), (123, None));
        assert_eq!(parse_holder(""), (-1, None)); // empty / garbage
        assert_eq!(parse_holder("abc\n456"), (-1, Some(456)));
    }

    #[test]
    fn acquired_lock_records_a_recoverable_pid() {
        // The written holder record must round-trip through parse_holder back
        // to the current PID (the start-time line is platform-dependent).
        let dir = tempfile::tempdir().unwrap();
        match try_acquire(dir.path()).unwrap() {
            AcquireOutcome::Acquired(lock) => {
                let buf = std::fs::read_to_string(lock.lock_path()).unwrap();
                let (pid, _start) = parse_holder(&buf);
                assert_eq!(pid as u32, std::process::id());
            }
            other => panic!("first acquire should succeed, got {other:?}"),
        }
    }

    #[test]
    fn first_acquire_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = try_acquire(dir.path()).unwrap();
        assert!(matches!(outcome, AcquireOutcome::Acquired(_)));
    }

    // POSIX-only: reading the holder PID back while the lock is held relies on
    // `flock` being advisory. On Windows `fs2` uses `LockFileEx`, whose
    // exclusive lock blocks reads from every other handle (even same-process),
    // so the PID readback cannot succeed while the lock is held. Mutual
    // exclusion itself still works on Windows; only this diagnostic does not.
    #[cfg(not(windows))]
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

    // POSIX-only: see `second_acquire_in_same_process_returns_held_by_live` —
    // Windows `LockFileEx` blocks reading the holder PID while the lock is held.
    #[cfg(not(windows))]
    #[test]
    fn diagnose_holder_returns_pid_when_held() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = try_acquire(dir.path()).unwrap();
        let diag = diagnose_holder(dir.path()).expect("file should exist");
        assert_eq!(diag.pid as u32, std::process::id());
        assert!(diag.process_alive);
    }
}
