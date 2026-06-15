//! Cross-platform process-liveness check — single source of truth for
//! "is PID X alive?".
//!
//! Shared by the daemon lifecycle (`stop` / `status` in `aleph-server`) and the
//! singleton instance lock's orphan-vs-live classification. Previously each
//! site carried its own `#[cfg]` pair with *opposite* non-Unix fallbacks — the
//! daemon assumed dead, the lock assumed alive — and both were wrong on
//! Windows (`stop`/`status` misreported a live server as stale; an orphaned
//! lock from a crashed daemon was never recognized as orphaned). This unifies
//! them onto one correct implementation.
//!
//! Unix uses `kill(pid, 0)`: async-signal-safe, allocation-free, and treats
//! `EPERM` as "alive but not ours". Non-Unix (Windows) uses `sysinfo`, which
//! maps to the Win32 process API.

/// Returns `true` if a process with `pid` currently exists.
///
/// A non-positive `pid` is never a real target and returns `false`, so a
/// corrupted PID file can never escalate into a broadcast signal on Unix.
#[must_use]
pub fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    is_alive_impl(pid)
}

#[cfg(unix)]
fn is_alive_impl(pid: i32) -> bool {
    // SAFETY: `kill(pid, 0)` performs error checking without sending a signal.
    // It is async-signal-safe and the canonical existence probe on Unix.
    if unsafe { libc::kill(pid, 0) } == 0 {
        true
    } else {
        // EPERM => process exists but we lack permission; ESRCH => it is gone.
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(not(unix))]
fn is_alive_impl(pid: i32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let Ok(pid_u32) = u32::try_from(pid) else {
        return false;
    };
    let pid = Pid::from_u32(pid_u32);
    let mut sys = System::new();
    // Refresh just this PID — far cheaper than a full process scan.
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        let me = i32::try_from(std::process::id()).expect("pid fits i32");
        assert!(is_process_alive(me));
    }

    #[test]
    fn non_positive_pid_is_dead() {
        assert!(!is_process_alive(0));
        assert!(!is_process_alive(-1));
    }

    #[test]
    fn almost_certainly_dead_pid_is_dead() {
        // A very large PID that is overwhelmingly unlikely to be live on any
        // platform. The point is that the probe returns a definite `false`
        // rather than a hardcoded fallback.
        assert!(!is_process_alive(i32::MAX - 1));
    }
}
