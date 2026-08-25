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
#[cfg_attr(windows, allow(unused_imports))]
use std::time::{Duration, Instant};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use serde::Serialize;

/// Process boot timestamp. Captured once via [`mark_boot`] so uptime can be
/// derived without threading the boot time through every caller.
///
/// `FailsClosed`: the single reader is
/// [`snapshot_shutdown_context`], which writes `uptime_secs: None` and — via
/// `skip_serializing_if` — omits the field entirely. Uptime is reported as
/// unknown rather than wrong, and no other behaviour changes.
///
/// This slot is also the wiring check's process sentinel — see [`booted`]. It
/// is the one member of the roster whose value nobody needs; what Task 12 reads
/// is whether it is there at all.
static BOOT_INSTANT: CapabilitySlot<Instant> =
    CapabilitySlot::new("gateway/boot-instant", MissingSemantics::FailsClosed);

/// Initialize the boot-time marker. Safe to call more than once; only the
/// first call wins. Call this from `aleph-server::main()` right after argv
/// parsing so the uptime number is meaningful.
pub fn mark_boot() {
    let _ = BOOT_INSTANT.install(Instant::now());
}

/// True iff this process ran `aleph-server start` far enough to reach
/// [`mark_boot`] (the first statement after argv parsing).
///
/// The `core/capability-wiring` check keys its three-state verdict on this: a
/// cold `aleph-server doctor` process installs nothing, so reporting its empty
/// roster as a problem — or as a pass — would both be fiction.
///
/// ⚠️ Deliberately `BOOT_INSTANT.get().is_some()` and NOT
/// `matches!(outcome(), Some(Outcome::Installed))`. The two agree today and
/// would diverge the day someone calls `decline` on this slot — and the honest
/// answer to "did this process boot" is about the VALUE, not the stamp.
#[must_use]
pub fn booted() -> bool {
    BOOT_INSTANT.get().is_some()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn boot_instant_slot() -> &'static dyn SlotStatus {
    &BOOT_INSTANT
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
            parts.push(format!("uptime_secs={u}"));
        }
        if let Some(s) = self.signal_num {
            parts.push(format!("signal_num={s}"));
        }
        if let Some(ref c) = self.parent_command {
            // Replace spaces with `_` so the line stays grep-tokenizable.
            // Truncate to 200 chars to keep the line bounded.
            let mut c = c.replace(' ', "_");
            if c.len() > 200 {
                // Truncate on a char boundary: `parent_command` is lossy-decoded
                // external (ps) output, so byte index 200 may split a multi-byte
                // char and panic.
                let end = (0..=200)
                    .rev()
                    .find(|&i| c.is_char_boundary(i))
                    .unwrap_or(0);
                c.truncate(end);
            }
            parts.push(format!("parent={c}"));
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
        .map_or(0, |d| d.as_secs());
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

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = boot_instant_slot();
        assert_eq!(slot.id(), "gateway/boot-instant");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

    /// `booted()` is how the wiring check tells "this process never ran boot"
    /// from "boot ran and left holes". Without it a cold `aleph-server doctor`
    /// would report every slot missing on a perfectly healthy machine.
    ///
    /// ⚠️ THIS IS THE ONLY TEST IN THE LIB BINARY THAT MAY TOUCH `BOOT_INSTANT`,
    /// and that is why `mark_boot_is_idempotent` was folded into it rather than
    /// left beside it. The brief asked for a second test and told me to grep
    /// first; the grep says `mark_boot_is_idempotent` already called
    /// `mark_boot()`. A separate `assert!(!booted())` would therefore have gone
    /// red only when the other test won the libtest race — a flaky guard, which
    /// teaches people to re-run rather than to look. Its assertions are kept
    /// verbatim below, so nothing was traded away for the determinism.
    #[test]
    fn booted_is_false_before_mark_boot_and_true_after() {
        // The negative half is the meaningful assertion and it is only sound
        // because of the invariant in the doc above: nothing else in this
        // binary reaches the marker, so no ordering can have set it already.
        assert!(!booted());
        mark_boot();
        assert!(booted());

        // Absorbed from `mark_boot_is_idempotent`: the first call above may or
        // may not have been the first writer; if not, a subsequent set is a
        // no-op. Either way the elapsed value must be monotonic.
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
