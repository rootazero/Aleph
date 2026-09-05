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
    // rust-doctor-disable-next-line unsafe-block-audit
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        true
    } else {
        // EPERM => process exists but we lack permission; ESRCH => it is gone.
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(not(unix))]
fn is_alive_impl(pid: i32) -> bool {
    let Ok(pid_u32) = u32::try_from(pid) else {
        return false;
    };
    with_process(pid_u32, |_| ()).is_some()
}

/// Wall-clock start time (seconds since the Unix epoch) of the process with
/// `pid`, or `None` if it isn't running or the platform won't report it.
///
/// This is the anti-PID-reuse signal: a recycled PID points at a *different*
/// process with a *different* start time. `sysinfo` reports it on Linux,
/// macOS and Windows alike — no `ps` subprocess (the approach taken by some
/// reference daemons, which is both Unix-only and a fork per check).
#[must_use]
pub fn process_start_time(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let pid_u32 = u32::try_from(pid).ok()?;
    with_process(pid_u32, sysinfo::Process::start_time)
}

/// Returns `true` if `pid` is alive AND (when `expected_start` is known and the
/// platform reports a start time) its start time matches `expected_start`.
///
/// Fail-safe: when there is no `expected_start` to compare against (a legacy
/// lock file written before start times were recorded), a live process is
/// enough — the pre-existing behaviour. So adopting it can only *tighten* a
/// diagnostic against PID reuse, never loosen it.
#[must_use]
pub fn process_matches(pid: i32, expected_start: Option<u64>) -> bool {
    if pid <= 0 {
        return false;
    }
    let Ok(pid_u32) = u32::try_from(pid) else {
        return false;
    };
    // One process lookup yields both existence and start time, so there is no
    // separate liveness probe (the previous `is_process_alive` +
    // `process_start_time` pair refreshed `sysinfo` twice on non-Unix).
    match with_process(pid_u32, sysinfo::Process::start_time) {
        None => false, // not running
        Some(actual) => match expected_start {
            // PID-reuse-resistant: a recycled PID has a different start time.
            Some(expected) => expected == actual,
            // Legacy lock file with no recorded start time -> liveness suffices.
            None => true,
        },
    }
}

/// Refresh a single PID and project a value out of its `Process`, or `None` if
/// it isn't running. Centralizes the `sysinfo` boilerplate (mirrors the 0.39
/// idiom in `gateway::memory_monitor`).
fn with_process<T>(pid: u32, f: impl FnOnce(&sysinfo::Process) -> T) -> Option<T> {
    with_process_specifics(pid, default_refresh_kind(), f)
}

/// What [`with_process`] asks `sysinfo` for. `sysinfo::System::refresh_processes`
/// picks this set itself; it is spelled out here because
/// [`with_process_specifics`] takes the kind as an argument, and a second
/// caller passing a NARROWER set must not silently change what this one gets.
fn default_refresh_kind() -> sysinfo::ProcessRefreshKind {
    use sysinfo::{ProcessRefreshKind, UpdateKind};
    ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_disk_usage()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_tasks()
}

/// [`with_process`], but the caller says which fields it needs.
///
/// One implementation, two callers: this module wants a start time, and
/// `gateway::pty::foreground` wants name/cmd/exe/cwd for the terminal's
/// foreground process. A second copy of the `System::new()` +
/// `refresh_processes_specifics` dance would be the same fact written twice
/// (判据 §1) — and the two would drift on exactly the axis that matters,
/// which fields get refreshed.
///
/// The refresh is scoped to ONE pid. That scoping is the cost story: a full
/// `ProcessesToUpdate::All` scan walks every process on the machine, which is
/// what the PTY probe's rate gate exists to avoid paying at 60 Hz.
pub(crate) fn with_process_specifics<T>(
    pid: u32,
    refresh_kind: sysinfo::ProcessRefreshKind,
    f: impl FnOnce(&sysinfo::Process) -> T,
) -> Option<T> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    // Refresh just this PID — far cheaper than a full process scan.
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh_kind);
    sys.process(pid).map(f)
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

    #[test]
    fn process_matches_falls_back_and_detects_reuse() {
        let me = i32::try_from(std::process::id()).expect("pid fits i32");
        // No recorded start time -> bare liveness check (legacy behaviour).
        assert!(process_matches(me, None));
        // A dead PID never matches, regardless of the expected start time.
        assert!(!process_matches(0, None));
        assert!(!process_matches(i32::MAX - 1, Some(123)));
        // When a start time is available, an exact match passes and any other
        // value (a recycled PID's different start time) fails.
        if let Some(start) = process_start_time(me) {
            assert!(process_matches(me, Some(start)));
            assert!(!process_matches(me, Some(start.wrapping_add(7))));
        }
    }
}
