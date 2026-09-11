//! Atomic file writes + advisory file locks.
//!
//! `write_atomic` writes via a sibling `.aleph_atomic_<rand>` file + fsync + rename so
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

/// Write bytes to `path` atomically: write to a sibling `.aleph_atomic_<rand>` file,
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

    // `make_in` with a plain `create_new` open rather than `tempfile_in`,
    // and `std::fs::rename` rather than `NamedTempFile::persist`, on purpose:
    //
    // * `tempfile_in` marks the file FILE_ATTRIBUTE_TEMPORARY on Windows and
    //   relies on `persist` to clear that again; a file that keeps the
    //   attribute past the rename is a file Windows may decline to flush.
    // * `persist` is `MoveFileEx(REPLACE_EXISTING)`, which fails with
    //   ERROR_ACCESS_DENIED whenever *any* handle has the destination open —
    //   a reader polling the file, an indexer, Defender — even one opened
    //   with every share flag. `std::fs::rename` uses POSIX-semantics rename
    //   on Windows (Rust ≥ 1.85) and replaces an open destination; the old
    //   handle keeps reading the old bytes. The acp persistence worker lost
    //   its final write to a 10 ms poll before this was the case.
    //
    // `make_in` still owns the random-name-and-retry-on-collision part, and
    // `TempPath` still removes the temp file if the rename fails.
    //
    // 0600 on Unix is what `tempfile_in` gave every file this function ever
    // wrote — `secrets.vault` among them — so the mode is kept explicit here
    // rather than left to the umask.
    let open_fresh = |candidate: &Path| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(candidate)
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".aleph_atomic_")
        .make_in(parent, open_fresh)?;
    tmp.write_all(bytes)?;
    tmp.as_file_mut().sync_all()?;
    // Closes the handle before the rename; the path stays owned so a failed
    // rename still cleans up.
    let tmp_path = tmp.into_temp_path();
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        // Best-effort cleanup of the leaked temp file. We log rather than
        // swallow because a directory populated with .aleph_atomic_* stubs
        // is an ops surprise the caller should be able to find.
        let leaked = tmp_path.display().to_string();
        if let Err(cleanup_err) = tmp_path.close() {
            tracing::warn!(
                temp_file = %leaked,
                error = %cleanup_err,
                "write_atomic: failed to remove leaked temp file after rename error",
            );
        }
        return Err(e);
    }
    // After a successful rename the temp path no longer exists; `TempPath`'s
    // drop tolerates that.
    drop(tmp_path);
    // fsync the parent directory so the rename survives a crash. On Linux
    // the new directory entry lives in the parent dir's metadata; without
    // this sync, the file can be on disk but absent from the directory
    // listing after a power cut. Other platforms return EINVAL on
    // dir-fsync — we treat that as best-effort rather than fail the write.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
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

/// "A peer currently holds the lock" — the only error the retry loop may
/// absorb. Derived from fs2's own contention error rather than from
/// `ErrorKind::WouldBlock`: on Windows fs2 reports contention as
/// `ERROR_LOCK_VIOLATION` (os error 33), which std classifies as
/// `Uncategorized`, so a `kind()` comparison never retried there and every
/// contended acquisition failed immediately. On Unix the contention error
/// is `EWOULDBLOCK`, whose kind already *is* `WouldBlock`, so this predicate
/// is a superset of the old one on every platform.
///
/// Shared with `instance_lock`, which must make the same call in the other
/// direction: a lock failure that is *not* contention has no peer to blame.
pub(crate) fn is_lock_contended(err: &std::io::Error) -> bool {
    err.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

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
            Err(e) if is_lock_contended(&e) => {
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

    /// A reader that happens to have the destination open must not make the
    /// write fail. On Windows this is exactly what `tempfile::persist`
    /// (`MoveFileEx`) does — ERROR_ACCESS_DENIED whenever any handle holds
    /// the target, even one opened with every share flag — and the acp
    /// persistence worker lost its final write to a 10 ms poll because of
    /// it. On Unix the rename never cared, so this passes trivially there;
    /// the guard is for the platform where it can go red.
    #[test]
    fn write_atomic_replaces_a_target_another_handle_holds_open() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo.bin");
        std::fs::write(&path, b"old").unwrap();
        let mut reader = File::open(&path).unwrap();
        write_atomic(&path, b"new").expect("replace must succeed while the old file is open");
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        // The old handle keeps reading the file it opened, as POSIX promises.
        let mut still_old = String::new();
        reader.read_to_string(&mut still_old).unwrap();
        assert_eq!(still_old, "old");
    }

    /// `tempfile_in` created every file this function wrote with mode 0600;
    /// the hand-rolled open that replaced it must not quietly widen that to
    /// the umask default — `secrets.vault` goes through here.
    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_owner_only_mode_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.bin");
        write_atomic(&path, b"s3cret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "atomic writes must stay owner-only, got {mode:o}"
        );
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

    /// The negative half of the retry gate: only fs2's contention error may
    /// be absorbed into the deadline loop. A permission or not-found error
    /// from the lock file must surface immediately, not after a 5 s spin —
    /// otherwise a bad ACL on `<data>/*.lock` would turn every caller into
    /// a 5 s stall that then reports `TimedOut` with a misleading "peer
    /// held" message.
    #[test]
    fn is_lock_contended_rejects_non_contention_os_errors() {
        #[cfg(windows)]
        let (denied, not_found) = (5, 2); // ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND
        #[cfg(unix)]
        let (denied, not_found) = (13, 2); // EACCES, ENOENT
        for code in [denied, not_found] {
            let err = std::io::Error::from_raw_os_error(code);
            assert!(
                !is_lock_contended(&err),
                "os error {code} ({err}) must not be retried as contention"
            );
        }
        assert!(
            is_lock_contended(&fs2::lock_contended_error()),
            "fs2's own contention error must be retried"
        );
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
