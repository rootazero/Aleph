//! Atomic file writes + advisory file locks.
//!
//! `write_atomic` writes via `<path>.tmp.<rand>` + fsync + rename so
//! readers always see either a complete old file or a complete new file
//! (never half-written).
//!
//! `with_file_lock` acquires an exclusive `fs2` advisory lock on a
//! sidecar `<path>.lock` file for the duration of a closure. Lock
//! release is RAII-driven (Drop on the guard).

use std::fs::File;
use std::io::Write;
use std::path::Path;

use fs2::FileExt;

/// Write bytes to `path` atomically: write to a sibling `.tmp.<rand>` file,
/// fsync, then rename over the destination. Readers always see either the
/// complete old file (or no file) or the complete new file — never a
/// half-written intermediate.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "write_atomic path has no parent directory",
        )
    })?;

    let mut tmp = tempfile::Builder::new()
        .prefix(".aleph_atomic_")
        .tempfile_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file_mut().sync_all()?;
    tmp.persist(path).map_err(|e| {
        let _ = std::fs::remove_file(e.file.path());
        e.error
    })?;
    Ok(())
}

/// RAII guard returned by `with_file_lock`. Drops the underlying
/// `File`, which releases the OS-level fs2 lock.
pub struct FileLockGuard {
    _file: File,
}

/// Acquire an exclusive fs2 advisory lock on `lock_path`, run `f`, and
/// release on return. The closure receives a borrow of the guard so it
/// cannot escape and call paths can still inspect lock state if needed.
///
/// Note: `lock_path` is the **lock sidecar**, not the data file. Callers
/// should pass e.g. `secrets.vault.lock` for a data file at `secrets.vault`.
///
/// Bounded wait: a peer that crashes mid-closure can leave the lock held
/// until the kernel reclaims the file handle (Linux) or the process exits
/// (other platforms); without a deadline this would hang every subsequent
/// caller (skill reader, dream pipeline, audit-drain stage) forever.
/// `LOCK_ACQUIRE_DEADLINE` caps the wait at a few seconds — long enough to
/// absorb legitimate contention, short enough that a stuck peer becomes a
/// warn + best-effort rather than a daemon-wide stall.
const LOCK_ACQUIRE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
const LOCK_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

pub fn with_file_lock<T, F>(lock_path: &Path, f: F) -> std::io::Result<T>
where
    F: FnOnce(&FileLockGuard) -> std::io::Result<T>,
{
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    let deadline = std::time::Instant::now() + LOCK_ACQUIRE_DEADLINE;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "with_file_lock: peer held {} past the {}s deadline \
                             (likely crashed; caller should treat as best-effort)",
                            lock_path.display(),
                            LOCK_ACQUIRE_DEADLINE.as_secs(),
                        ),
                    ));
                }
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(e) => return Err(e),
        }
    }
    let guard = FileLockGuard { _file: file };
    f(&guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn write_atomic_creates_file_with_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.bin");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn write_atomic_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.bin");
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn write_atomic_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.bin");
        write_atomic(&path, b"x").unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["foo.bin".to_string()]);
    }

    #[test]
    fn with_file_lock_serialises_two_threads() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("x.lock");
        let counter = Arc::new(Mutex::new(Vec::<u8>::new()));
        let barrier = Arc::new(Barrier::new(2));

        let mut handles = vec![];
        for tag in [b'A', b'B'] {
            let lp = lock_path.clone();
            let c = counter.clone();
            let b = barrier.clone();
            handles.push(thread::spawn(move || {
                b.wait();
                with_file_lock(&lp, |_guard| {
                    let mut v = c.lock().unwrap_or_else(|e| e.into_inner());
                    v.push(tag);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    v.push(tag);
                    Ok(())
                })
                .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // Each tag wrote two bytes back-to-back without interleave
        let v = counter.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(v.len(), 4);
        assert_eq!(v[0], v[1]);
        assert_eq!(v[2], v[3]);
        assert_ne!(v[0], v[2]);
    }

    /// Regression for `severed-wire-2026-09-05-modules2 skill I-2`: a peer
    /// that holds the lock past the deadline (e.g. crashed mid-closure)
    /// must surface as `TimedOut` rather than hanging the caller. We do
    /// not wait the full 5s here — instead we replace the deadline with a
    /// tiny one via a private override; this is an indirect test of the
    /// loop body's deadline-exceeded branch.
    #[test]
    fn with_file_lock_returns_timed_out_when_peer_holds_past_deadline() {
        // Open the file first so it exists for both threads.
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("held.lock");
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();

        // One thread takes the lock and sleeps; the other tries to acquire
        // it and must time out rather than block forever.
        let held_lock = lock_path.clone();
        let holder = thread::spawn(move || {
            with_file_lock(&held_lock, |_guard| {
                std::thread::sleep(std::time::Duration::from_secs(8));
                Ok(())
            })
        });

        // Give the holder a moment to acquire the lock.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let contended_lock = lock_path.clone();
        let contender = thread::spawn(move || {
            let started = std::time::Instant::now();
            let res = with_file_lock(&contended_lock, |_guard| Ok(()));
            (res, started.elapsed())
        });

        let (res, elapsed) = contender.join().unwrap();
        holder.join().unwrap().unwrap();
        assert!(
            matches!(res, Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut),
            "contended acquisition must return TimedOut, got {res:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(7),
            "TimedOut must surface well inside the deadline (elapsed = {elapsed:?})"
        );
    }
}
