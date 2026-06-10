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
pub fn with_file_lock<T, F>(lock_path: &Path, f: F) -> std::io::Result<T>
where
    F: FnOnce(&FileLockGuard) -> std::io::Result<T>,
{
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock_exclusive()?;
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
}
