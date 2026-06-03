//! Shutdown forensics — capture context when the gateway receives a signal.
//!
//! Ported from hermes-agent `gateway/shutdown_forensics.py`.
//!
//! The gateway's signal handlers run inside the async runtime. We can't
//! safely block them for long, but we DO want a durable record of who/what
//! triggered the shutdown so that "the gateway keeps dying" incidents can be
//! diagnosed after the fact.
//!
//! [`snapshot_shutdown_context`] is a fast (<10 ms), non-blocking probe that
//! returns a structured [`ShutdownContext`] the signal handler can log
//! immediately. Anything that needs to shell out (e.g. `ps -p <ppid>`) is
//! gated behind `with_parent_command = true` and uses a 250 ms timeout via
//! `Command::output` — the caller chooses whether they can tolerate that.

// `Command`/`Stdio` and `Duration` are only used by the Unix `ps`-shell-out
// path; on Windows that probe is compiled out, leaving them unused.
#[cfg_attr(windows, allow(unused_imports))]
use std::process::{Command, Stdio};
use std::sync::OnceLock;
#[cfg_attr(windows, allow(unused_imports))]
use std::time::{Duration, Instant};

use serde::Serialize;

/// Process boot timestamp. Captured once via [`mark_boot`] so uptime can be
/// derived without threading the boot time through every caller.
static BOOT_INSTANT: OnceLock<Instant> = OnceLock::new();

/// Initialize the boot-time marker. Safe to call more than once; only the
/// first call wins. Call this from `aleph-server::main()` right after argv
/// parsing so the uptime number is meaningful.
pub fn mark_boot() {
    let _ = BOOT_INSTANT.set(Instant::now());
}

/// Structured forensics snapshot. Serialized as JSON for grep-friendly
/// single-line logging.
#[derive(Debug, Clone, Serialize)]
pub struct ShutdownContext {
    /// Signal name (e.g. `SIGTERM`, `SIGINT`, `ctrl_c`).
    pub signal: String,
    /// Numeric signal value if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_num: Option<i32>,
    /// Our PID.
    pub pid: u32,
    /// Parent PID (best-effort; 0 if unavailable).
    pub ppid: u32,
    /// Parent command line (only filled when `with_parent_command` was set
    /// AND the `ps` invocation finished in <250 ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_command: Option<String>,
    /// Process uptime in seconds. `None` if [`mark_boot`] was never called.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    /// Wall-clock timestamp (`SystemTime::now()` as Unix epoch seconds).
    pub at_unix_secs: u64,
}

impl ShutdownContext {
    /// Render as a single-line `key=value` block for grep-friendly logs.
    #[must_use]
    pub fn as_log_line(&self) -> String {
        let mut parts = vec![
            format!("signal={}", self.signal),
            format!("pid={}", self.pid),
            format!("ppid={}", self.ppid),
        ];
        if let Some(u) = self.uptime_secs {
            parts.push(format!("uptime_secs={}", u));
        }
        if let Some(s) = self.signal_num {
            parts.push(format!("signal_num={}", s));
        }
        if let Some(ref c) = self.parent_command {
            // Replace spaces with `_` so the line stays grep-tokenizable.
            // Truncate to 200 chars to keep the line bounded.
            let mut c = c.replace(' ', "_");
            if c.len() > 200 {
                c.truncate(200);
            }
            parts.push(format!("parent={}", c));
        }
        format!("[SHUTDOWN] {}", parts.join(" "))
    }
}

/// Capture forensic context for a shutdown signal. **Synchronous and fast.**
///
/// * `signal_label` — human label (e.g. `"SIGTERM"`, `"ctrl_c"`).
/// * `signal_num` — numeric signal value if known. May be `None` on Windows
///   where `ctrl_c` doesn't carry a value.
/// * `with_parent_command` — if `true`, runs `ps -p <ppid> -o args=` with
///   a 250 ms wall-clock budget. Skipped when `ppid == 0`. On any error or
///   timeout the field is silently dropped — forensics is best-effort.
#[must_use]
pub fn snapshot_shutdown_context(
    signal_label: impl Into<String>,
    signal_num: Option<i32>,
    with_parent_command: bool,
) -> ShutdownContext {
    let pid = std::process::id();
    let ppid = parent_pid();
    let parent_command = if with_parent_command && ppid != 0 {
        read_parent_command(ppid)
    } else {
        None
    };
    let uptime_secs = BOOT_INSTANT.get().map(|b| b.elapsed().as_secs());
    let at_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ShutdownContext {
        signal: signal_label.into(),
        signal_num,
        pid,
        ppid,
        parent_command,
        uptime_secs,
        at_unix_secs,
    }
}

#[cfg(unix)]
fn parent_pid() -> u32 {
    // SAFETY: getppid() has no preconditions and never fails.
    unsafe { libc::getppid() as u32 }
}

#[cfg(not(unix))]
fn parent_pid() -> u32 {
    0
}

#[cfg(unix)]
fn read_parent_command(ppid: u32) -> Option<String> {
    // Bounded subprocess — `ps` typically returns in <20 ms. We enforce a
    // 250 ms wall-clock budget by polling `try_wait` in a tight loop.
    let mut child = Command::new("ps")
        .arg("-p")
        .arg(ppid.to_string())
        .arg("-o")
        .arg("args=")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }

    let output = child.wait_with_output().ok()?;
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(not(unix))]
fn read_parent_command(_ppid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_has_pid_and_signal() {
        let ctx = snapshot_shutdown_context("ctrl_c", None, false);
        assert_eq!(ctx.signal, "ctrl_c");
        assert!(ctx.pid > 0);
        // ppid: macOS/Linux always > 0; Windows is hard-coded to 0.
        #[cfg(unix)]
        assert!(ctx.ppid > 0);
    }

    #[test]
    fn log_line_is_grep_friendly() {
        let mut ctx = snapshot_shutdown_context("SIGTERM", Some(15), false);
        ctx.uptime_secs = Some(42);
        let line = ctx.as_log_line();
        assert!(line.starts_with("[SHUTDOWN] "));
        assert!(line.contains("signal=SIGTERM"));
        assert!(line.contains("signal_num=15"));
        assert!(line.contains("uptime_secs=42"));
    }

    #[test]
    fn snapshot_finishes_quickly_without_parent_command() {
        let start = Instant::now();
        let _ = snapshot_shutdown_context("test", None, false);
        // The non-shelling path must be well under 10 ms.
        assert!(start.elapsed() < Duration::from_millis(10));
    }

    #[test]
    fn mark_boot_is_idempotent() {
        mark_boot();
        // First call may or may not have been the first writer; if so a
        // subsequent set is a no-op. Either way the elapsed value must be
        // monotonic.
        let first = BOOT_INSTANT.get().expect("boot recorded").elapsed();
        mark_boot();
        let second = BOOT_INSTANT.get().expect("boot recorded").elapsed();
        assert!(second >= first);
    }

    #[cfg(unix)]
    #[test]
    fn parent_command_lookup_is_bounded() {
        let ppid = parent_pid();
        let start = Instant::now();
        // We don't assert on content (could be the test runner, an IDE,
        // launchd, etc.) — only that the call respects its budget.
        let _ = read_parent_command(ppid);
        assert!(start.elapsed() < Duration::from_millis(300));
    }
}
