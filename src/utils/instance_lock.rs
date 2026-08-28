//! Cross-process singleton lock for a given Aleph data directory.
//!
//! Uses `fs2::FileExt::try_lock_exclusive` on `<data_dir>/aleph.lock` for the
//! actual mutual exclusion. The holder's PID (+ start time) is recorded in an
//! UNLOCKED sidecar `<data_dir>/aleph.lock.pid`, so a contending second
//! instance can read who holds the lock on every platform: reading the PID out
//! of the locked file itself fails with os error 33 on Windows, where `fs2`
//! uses `LockFileEx` and an exclusive lock blocks reads from all other handles.
//! The lock is automatically released by the OS when the holder process exits
//! (graceful, panic, SIGKILL — all release).

use std::fs::File;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::atomic_io::write_atomic;
use super::process_alive::{process_matches, process_start_time};

const LOCK_FILENAME: &str = "aleph.lock";
/// Sidecar that records the current holder's PID (+ start time). Kept separate
/// from `aleph.lock` and never locked, so a second instance can read the holder
/// back even while the first holds the exclusive lock (see module docs on the
/// Windows `LockFileEx` read-blocking behavior).
const HOLDER_FILENAME: &str = "aleph.lock.pid";

#[derive(Debug)]
pub struct InstanceLock {
    #[allow(dead_code)] // Held for OS-level lock lifetime via Drop on `File`.
    file: File,
    holder_path: PathBuf,
    holder_pid: u32,
}

impl InstanceLock {
    /// Rewrite the holder record's PID to the *current* process id.
    ///
    /// Call this after `fork()`/daemonization: the flock is held on a fd that
    /// survives `fork()`, so the daemonized grandchild still owns the lock —
    /// but the sidecar still names the original (now-exited) parent PID that
    /// called `try_acquire`. Without this, `diagnose_holder` and the PID
    /// readback in `try_acquire` mistake the live daemon for a stale /
    /// orphaned lock, and a second `start` can print "safe to `rm`" advice for
    /// a lock that is in fact held by a running process.
    pub fn rewrite_holder_pid(&mut self) -> std::io::Result<()> {
        let pid = std::process::id();
        write_holder(&self.holder_path, pid)?;
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
    let holder_path = data_dir.join(HOLDER_FILENAME);

    // Refuse a symlink at `lock_path`. The path is fixed (`<data_dir>/aleph.lock`)
    // and only this process should ever create the file, so a symlink there is
    // either an attacker-planted redirect or a previous failed install — both
    // of which must NOT be silently followed. Following the symlink would
    // (a) lock an attacker-controlled file (DoS: lock against the real
    // aleph.lock becomes a lock against their file), or (b) let the sidecar
    // rename target an unexpected inode. `O_NOFOLLOW` is POSIX; Windows has
    // no portable equivalent, so non-Unix falls back to the previous behavior.
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing to use lock file at {}: a symlink is present \
                         where a regular file is required (possible tampering)",
                        lock_path.display()
                    ),
                ));
            }
            Err(e) => return Err(e),
        }
    };
    #[cfg(not(unix))]
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;

    match file.try_lock_exclusive() {
        Ok(()) => {
            // Got the lock — record our PID (+ start time) in the unlocked
            // sidecar so a contending second instance can read it back on
            // every platform.
            let pid = std::process::id();
            write_holder(&holder_path, pid)?;
            Ok(AcquireOutcome::Acquired(InstanceLock {
                file,
                holder_path,
                holder_pid: pid,
            }))
        }
        Err(_) => {
            // Lock is held by someone else. Read the holder from the unlocked
            // sidecar (reading the locked file itself would fail with os error
            // 33 on Windows, where the exclusive lock blocks all other reads).
            let (pid, expected_start) = read_holder(&holder_path);
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

/// Read holder metadata from the sidecar WITHOUT competing for the lock.
///
/// Returns:
/// - `Ok(Some(diag))` — the sidecar exists, was readable, and parsed cleanly.
/// - `Ok(None)`        — the sidecar is determinately absent or empty (a clean
///                       "no holder" answer; the operator genuinely has no
///                       lock to clear).
/// - `Err(io::Error)`  — the sidecar exists but the filesystem refused to
///                       answer (`EACCES`, AV lock, ACL revoke, ...). The
///                       answer is *unknown* — NOT "no lock held" — and the
///                       caller is expected to surface this as a diagnostic
///                       warning rather than fold it into the reassuring
///                       absent path. A doctor that reports `[ok] No lock
///                       held` for an unreadable holder file is the line in
///                       front of the exact vault-data-loss condition that
///                       AGENTS.md names.
pub fn diagnose_holder(data_dir: &Path) -> std::io::Result<Option<HolderDiagnostic>> {
    let holder_path = data_dir.join(HOLDER_FILENAME);
    let buf = match std::fs::read_to_string(&holder_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if buf.trim().is_empty() {
        return Ok(None);
    }
    let (pid, expected_start) = parse_holder(&buf);
    Ok(Some(HolderDiagnostic {
        pid,
        process_alive: process_matches(pid, expected_start),
        lock_path: data_dir.join(LOCK_FILENAME),
    }))
}

/// Write the holder record to the unlocked sidecar: line 1 = PID, line 2 = the
/// holder's process start time (seconds since epoch) when the platform reports
/// one. The start time is what makes the live/orphaned classification immune to
/// PID reuse — a recycled PID has a different start time. When unavailable we
/// write the PID alone, keeping the legacy single-line format. The write is
/// atomic (temp + rename) so a concurrent reader never observes a half-written
/// record.
fn write_holder(holder_path: &Path, pid: u32) -> std::io::Result<()> {
    let record = match process_start_time(pid as i32) {
        Some(start) => format!("{pid}\n{start}\n"),
        None => format!("{pid}\n"),
    };
    write_atomic(holder_path, record.as_bytes())
}

/// Read the holder record from the unlocked sidecar. Returns `(-1, None)` when
/// the sidecar is absent, empty, or unreadable — every "no live holder" case
/// collapses to a non-positive PID the callers already treat as free.
fn read_holder(holder_path: &Path) -> (i32, Option<u64>) {
    match std::fs::read_to_string(holder_path) {
        Ok(buf) => parse_holder(&buf),
        Err(_) => (-1, None),
    }
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
        // The holder record lands in the unlocked sidecar and must round-trip
        // through parse_holder back to the current PID (the start-time line is
        // platform-dependent). Cross-platform: the sidecar is never locked.
        let dir = tempfile::tempdir().unwrap();
        match try_acquire(dir.path()).unwrap() {
            AcquireOutcome::Acquired(_lock) => {
                let buf = std::fs::read_to_string(dir.path().join(HOLDER_FILENAME)).unwrap();
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

    // The holder PID lives in the unlocked sidecar, so the readback while the
    // lock is held now works on every platform (including Windows, where the
    // exclusive `LockFileEx` would block reads of the lock file itself).
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
        assert!(matches!(
            diagnose_holder(dir.path()),
            Ok(None) // sidecar absent → "no holder" answer, not a read error
        ));
    }

    // Cross-platform now: `diagnose_holder` reads the unlocked sidecar, so it
    // resolves the holder PID even while the exclusive lock is held.
    #[test]
    fn diagnose_holder_returns_pid_when_held() {
        let dir = tempfile::tempdir().unwrap();
        let _hold = try_acquire(dir.path()).unwrap();
        let diag = diagnose_holder(dir.path())
            .expect("sidecar should be readable")
            .expect("sidecar should hold a holder record");
        assert_eq!(diag.pid as u32, std::process::id());
        assert!(diag.process_alive);
    }

    /// A holder sidecar that exists but is unreadable (EACCES, AV lock, ...)
    /// must propagate the IO error, NOT be silently folded into the absent
    /// path. The `stale_lock` doctor check depends on this: a `[ok] No lock
    /// held` for an unreadable sidecar is the reassuring line in front of the
    /// vault-data-loss condition.
    #[cfg(unix)]
    #[test]
    fn diagnose_holder_propagates_io_error_for_unreadable_sidecar() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let holder = dir.path().join(HOLDER_FILENAME);
        std::fs::write(&holder, b"123\n456\n").unwrap();
        // Drop read permission on the sidecar itself. The directory stays
        // traversable so the *existence* check passes; only the read fails.
        std::fs::set_permissions(&holder, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = diagnose_holder(dir.path());
        // Restore permissions so the tempdir cleanup can succeed even if the
        // assertion below fails.
        let restore =
            std::fs::set_permissions(&holder, std::fs::Permissions::from_mode(0o600));
        let _ = restore;

        match result {
            Err(e) => assert_ne!(e.kind(), std::io::ErrorKind::NotFound),
            Ok(opt) => panic!(
                "an unreadable sidecar must surface as Err, not Ok({opt:?}); \
                 folding into the absent path is the bug"
            ),
        }
    }

    /// A symlink planted at `aleph.lock` is either an attacker redirect or a
    /// previous failed install — either way, locking it (which would otherwise
    /// follow the link and lock the attacker-controlled target inode) must be
    /// refused, not silently followed.
    #[cfg(unix)]
    #[test]
    fn try_acquire_refuses_a_symlink_at_the_lock_path() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(LOCK_FILENAME);
        // Plant a regular file elsewhere and symlink the lock path at it.
        // `target_file` is a real 0700 file owned by us, so the only thing
        // that distinguishes the attack from a benign lock file is the symlink.
        let target = dir.path().join("attacker_target");
        std::fs::write(&target, b"x").unwrap();
        std::os::unix::fs::symlink(&target, &lock_path).unwrap();

        let err = try_acquire(dir.path()).expect_err("a symlink at the lock path must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            err.to_string().contains("symlink"),
            "refusal must name the defect, got: {err}"
        );
    }
}
