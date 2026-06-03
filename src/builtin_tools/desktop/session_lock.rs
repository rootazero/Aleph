//! File-based session lock for desktop computer use.
//!
//! Prevents multiple agent sessions from simultaneously controlling the
//! desktop. The lock file lives at `<data_dir>/aleph/computer-use.lock`
//! (`dirs::data_dir()`, e.g. `~/Library/Application Support/aleph/` on macOS,
//! `~/.local/share/aleph/` on Linux).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// JSON content stored inside the lock file.
#[derive(Debug, Serialize, Deserialize)]
struct LockInfo {
    session_id: String,
    pid: u32,
    acquired_at: String,
}

/// File-based exclusive lock for a desktop computer-use session.
///
/// Only one session may hold the lock at a time. Stale locks left by dead
/// processes are automatically reclaimed on `acquire()`.
pub struct ComputerUseLock {
    lock_path: PathBuf,
    session_id: String,
    held: bool,
}

impl ComputerUseLock {
    /// Create a new lock handle for `session_id`.
    ///
    /// The lock is **not** acquired yet — call [`acquire`] to take it.
    pub fn new(session_id: impl Into<String>) -> Self {
        let lock_path = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aleph")
            .join("computer-use.lock");

        Self {
            lock_path,
            session_id: session_id.into(),
            held: false,
        }
    }

    /// Create a lock handle pointing to an arbitrary path (for testing).
    #[cfg(test)]
    fn with_path(session_id: impl Into<String>, lock_path: PathBuf) -> Self {
        Self {
            lock_path,
            session_id: session_id.into(),
            held: false,
        }
    }

    /// Try to acquire the lock.
    ///
    /// - Already held by this handle → no-op, `Ok(())`
    /// - No lock file → create and hold, `Ok(())`
    /// - Lock file owned by same `session_id` → mark held, `Ok(())`
    /// - Lock file owned by another session whose PID is dead → force takeover, `Ok(())`
    /// - Lock file owned by another session whose PID is alive → `Err(...)`
    /// - Corrupt lock file → overwrite, `Ok(())`
    pub fn acquire(&mut self) -> Result<(), String> {
        // Re-entrant: already held.
        if self.held {
            debug!(session_id = %self.session_id, "ComputerUseLock re-entrant acquire (already held)");
            return Ok(());
        }

        // Check existing lock file.
        if self.lock_path.exists() {
            match self.read_lock_file() {
                Some(info) if info.session_id == self.session_id => {
                    // Same session; just claim it.
                    debug!(session_id = %self.session_id, "ComputerUseLock reclaiming own lock");
                    self.held = true;
                    return Ok(());
                }
                Some(info) => {
                    // Different session — check if that PID is still alive.
                    if is_pid_alive(info.pid) {
                        return Err(format!(
                            "Desktop is being controlled by another session (session_id={}, pid={})",
                            info.session_id, info.pid
                        ));
                    }
                    // PID is dead; forcibly take over.
                    warn!(
                        stale_session = %info.session_id,
                        stale_pid = info.pid,
                        new_session = %self.session_id,
                        "ComputerUseLock: taking over stale lock from dead process"
                    );
                }
                None => {
                    // Corrupt or unreadable lock file.
                    warn!(
                        path = %self.lock_path.display(),
                        "ComputerUseLock: overwriting corrupt lock file"
                    );
                }
            }
        }

        self.write_lock_file()?;
        self.held = true;
        debug!(session_id = %self.session_id, "ComputerUseLock acquired");
        Ok(())
    }

    /// Release the lock.
    ///
    /// Verifies ownership (by `session_id`) before deleting. No-op if not held.
    pub fn release(&mut self) -> Result<(), String> {
        if !self.held {
            return Ok(());
        }

        // Verify we still own the lock file before deleting.
        if let Some(info) = self.read_lock_file() {
            if info.session_id != self.session_id {
                // Another session took over; don't delete their file.
                warn!(
                    our_session = %self.session_id,
                    file_session = %info.session_id,
                    "ComputerUseLock: lock file belongs to another session, skipping delete"
                );
                self.held = false;
                return Ok(());
            }
        }

        if self.lock_path.exists() {
            fs::remove_file(&self.lock_path)
                .map_err(|e| format!("ComputerUseLock: failed to remove lock file: {e}"))?;
        }

        self.held = false;
        debug!(session_id = %self.session_id, "ComputerUseLock released");
        Ok(())
    }

    /// Returns `true` if this handle currently holds the lock.
    pub fn is_held(&self) -> bool {
        self.held
    }

    // --- private helpers ---

    fn read_lock_file(&self) -> Option<LockInfo> {
        let content = fs::read_to_string(&self.lock_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn write_lock_file(&self) -> Result<(), String> {
        // Ensure parent directory exists.
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("ComputerUseLock: failed to create lock directory: {e}"))?;
        }

        let info = LockInfo {
            session_id: self.session_id.clone(),
            pid: std::process::id(),
            acquired_at: chrono::Utc::now().to_rfc3339(),
        };

        let content = serde_json::to_string(&info)
            .map_err(|e| format!("ComputerUseLock: failed to serialize lock info: {e}"))?;

        fs::write(&self.lock_path, content)
            .map_err(|e| format!("ComputerUseLock: failed to write lock file: {e}"))
    }
}

impl Drop for ComputerUseLock {
    fn drop(&mut self) {
        if self.held {
            if let Err(e) = self.release() {
                warn!(error = %e, "ComputerUseLock: error during drop release");
            }
        }
    }
}

/// Check whether a process with the given PID is alive.
#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    // A zero or out-of-range PID is never a real process. `kill(0, 0)` would
    // otherwise target the *caller's* process group and report "alive", and a
    // PID above `i32::MAX` would wrap to a negative `pid_t` (also a group).
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: kill(pid, 0) sends no signal; it only checks process existence.
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    // kill returned -1; check errno to distinguish "no such process" from other errors.
    // ESRCH  = process definitely does not exist → treat as dead.
    // EPERM  = process may exist but we lack permission → conservative: treat as alive.
    // EINVAL = invalid PID → treat as dead.
    let errno = std::io::Error::last_os_error().raw_os_error();
    match errno {
        Some(libc::ESRCH) | Some(libc::EINVAL) => false,
        Some(_) => true, // EPERM or other errors → conservative: treat as alive
        None => false,   // errno unavailable → treat as dead (shouldn't happen)
    }
}

/// Check whether a process with the given PID is alive.
///
/// Opens the process with a query-only access right and reads its exit code:
/// a still-running process reports `STATUS_PENDING` (259). The previous
/// implementation unconditionally returned `false`, which made the desktop
/// session lock a no-op on Windows — a live session's lock could always be
/// stolen by a concurrent one.
#[cfg(windows)]
fn is_pid_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }

    // STATUS_PENDING — GetExitCodeProcess reports this while a process runs.
    const STILL_RUNNING: u32 = 259;

    // SAFETY: `OpenProcess` is called with a query-only access right; the
    // returned handle, when non-null, is closed before this function returns.
    // `GetExitCodeProcess` only writes into a stack local. No handle escapes.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // The process cannot be opened — almost always because it has
            // exited. (A live process owned by a different user could fail
            // with ERROR_ACCESS_DENIED; treating that as dead is acceptable:
            // a wrongly-reclaimed lock self-heals on the next acquire, a
            // permanently-wedged one would not.)
            return false;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && exit_code == STILL_RUNNING
    }
}

// ---------------------------------------------------------------------------
// Process-global, call-time lock acquisition
// ---------------------------------------------------------------------------
//
// The `desktop` tool is a shared singleton (one `DesktopTool` instance serves
// every session — see `executor/builtin_registry`), so the controlling session
// cannot be baked into a struct field at construction time. The lock is instead
// acquired at call time against the live turn's session and held — with
// re-entrant depth counting — only for as long as a desktop call (or a batch
// and its sub-actions) is in flight. When the outermost guard drops, the
// underlying file lock is released so another session can take the desktop.

/// Held computer-use locks, keyed by session id, with an outstanding-guard
/// depth so nested calls from the same session (a batch and its sub-actions)
/// share one file lock and release it only when the last guard drops.
static HELD_LOCKS: LazyLock<Mutex<HashMap<String, (ComputerUseLock, usize)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// RAII guard returned by [`acquire_for_session`]. Releasing the desktop is
/// deferred to `Drop` so every early-return path in the caller frees the lock.
pub struct SessionLockGuard {
    session_id: String,
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        let mut map = HELD_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, depth)) = map.get_mut(&self.session_id) {
            *depth -= 1;
            if *depth == 0 {
                // Removing the entry drops the `ComputerUseLock`, whose own
                // `Drop` calls `release()` and deletes the lock file.
                map.remove(&self.session_id);
            }
        }
    }
}

/// Acquire the desktop computer-use lock for `session_id`, returning an RAII
/// guard that frees it on drop.
///
/// - Re-entrant: a nested call from the same session bumps a depth counter and
///   shares the one underlying file lock; the file is released only when the
///   outermost guard drops.
/// - Mutually exclusive across sessions: if a *different* live session already
///   holds the lock, returns `Err(message)` and acquires nothing.
pub fn acquire_for_session(session_id: &str) -> Result<SessionLockGuard, String> {
    let mut map = HELD_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    match map.get_mut(session_id) {
        Some((_, depth)) => {
            // Already held by this session (e.g. a batch sub-action).
            *depth += 1;
        }
        None => {
            let mut lock = ComputerUseLock::new(session_id.to_string());
            lock.acquire()?;
            map.insert(session_id.to_string(), (lock, 1));
        }
    }
    Ok(SessionLockGuard {
        session_id: session_id.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_lock_path(dir: &TempDir) -> PathBuf {
        dir.path().join("computer-use.lock")
    }

    fn make_lock(session_id: &str, dir: &TempDir) -> ComputerUseLock {
        ComputerUseLock::with_path(session_id, temp_lock_path(dir))
    }

    #[test]
    fn test_acquire_and_release() {
        let dir = TempDir::new().unwrap();
        let mut lock = make_lock("session-abc", &dir);

        assert!(!lock.is_held());
        lock.acquire().expect("acquire should succeed");
        assert!(lock.is_held());
        assert!(temp_lock_path(&dir).exists(), "lock file should exist");

        lock.release().expect("release should succeed");
        assert!(!lock.is_held());
        assert!(!temp_lock_path(&dir).exists(), "lock file should be gone");
    }

    #[test]
    fn test_reentrant_acquire() {
        let dir = TempDir::new().unwrap();
        let mut lock = make_lock("session-reentrant", &dir);

        lock.acquire().expect("first acquire should succeed");
        lock.acquire()
            .expect("second acquire (re-entrant) should succeed");
        assert!(lock.is_held());
    }

    #[test]
    fn test_stale_lock_recovery() {
        let dir = TempDir::new().unwrap();
        let path = temp_lock_path(&dir);

        // Write a fake lock with a definitely-dead PID.
        let stale = LockInfo {
            session_id: "old-session".to_string(),
            pid: 999_999_999,
            acquired_at: "2020-01-01T00:00:00Z".to_string(),
        };
        fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();

        let mut lock = make_lock("new-session", &dir);
        lock.acquire().expect("should take over stale lock");
        assert!(lock.is_held());

        // Verify the lock file now belongs to the new session.
        let content = fs::read_to_string(&path).unwrap();
        let info: LockInfo = serde_json::from_str(&content).unwrap();
        assert_eq!(info.session_id, "new-session");
    }

    #[test]
    fn test_live_lock_blocks() {
        let dir = TempDir::new().unwrap();
        let path = temp_lock_path(&dir);

        // Write a lock owned by a different session but with our own PID (which is alive).
        let live = LockInfo {
            session_id: "other-session".to_string(),
            pid: std::process::id(),
            acquired_at: chrono::Utc::now().to_rfc3339(),
        };
        fs::write(&path, serde_json::to_string(&live).unwrap()).unwrap();

        let mut lock = make_lock("my-session", &dir);
        let result = lock.acquire();
        assert!(result.is_err(), "should be blocked by live lock");
        assert!(!lock.is_held());
    }

    #[test]
    fn test_is_pid_alive_current_process() {
        // The test process itself is, definitionally, alive. Exercises the
        // real liveness check on whichever platform the suite runs on.
        assert!(is_pid_alive(std::process::id()));
    }

    #[test]
    fn test_is_pid_alive_rejects_zero() {
        // PID 0 is never a real process and must never be reported alive —
        // on Unix `kill(0, 0)` would otherwise hit the caller's process group.
        assert!(!is_pid_alive(0));
    }

    #[test]
    fn test_drop_releases_lock() {
        let dir = TempDir::new().unwrap();
        let path = temp_lock_path(&dir);

        {
            let mut lock = make_lock("session-drop", &dir);
            lock.acquire().expect("acquire should succeed");
            assert!(path.exists());
        } // lock is dropped here

        assert!(!path.exists(), "lock file should be gone after drop");
    }
}
