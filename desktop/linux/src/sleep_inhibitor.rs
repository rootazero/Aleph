use std::process::Stdio;
use std::time::{Duration, Instant};

use aleph_desktop::{
    error::{DesktopError, Result},
    traits::{InhibitorGuard, PowerCapability},
};

pub struct LinuxPower;

impl LinuxPower {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for LinuxPower {
    fn default() -> Self {
        Self::new()
    }
}

/// How long to watch a freshly spawned inhibitor before believing it.
///
/// It holds the lock by *staying alive* (it runs `sleep infinity` under the
/// lock), so an inhibitor that has already exited is an inhibitor that failed.
/// The window only has to outlast the D-Bus round trip the tool makes before it
/// gives up; a healthy one never exits at all, so this costs nothing on a real
/// desktop.
const SETTLE_WINDOW: Duration = Duration::from_millis(400);

/// Poll interval inside [`SETTLE_WINDOW`].
const SETTLE_POLL: Duration = Duration::from_millis(25);

/// Spawn one inhibitor candidate and confirm it is still holding the lock.
///
/// Returns `None` when the binary is missing **or** when it exited on its own —
/// which is the case this function exists for. `systemd-inhibit` exits non-zero
/// (printing "Failed to inhibit: Access denied") without ever running its
/// command when polkit refuses or there is no active logind session; the
/// previous code only caught a *spawn* failure, so on those hosts it returned a
/// guard for a lock nobody held and the caller was told sleep was inhibited.
/// That is the same "only spawn failure counts" shape already corrected in the
/// clipboard write path and in `run_script`'s pwsh fallback.
fn spawn_inhibitor(program: &str, args: &[&str]) -> Option<std::process::Child> {
    // Silence the child's stdio: a denied `systemd-inhibit` writes to stderr,
    // and a daemon whose stderr is a log file would collect that line once per
    // agent turn. The exit status, not the message, is what we read.
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + SETTLE_WINDOW;
    while Instant::now() < deadline {
        match child.try_wait() {
            // Gone already: the lock was refused. Reap and report failure.
            Ok(Some(_)) => return None,
            Ok(None) => std::thread::sleep(SETTLE_POLL),
            // We cannot tell — treat the child as live rather than killing a
            // possibly-good inhibitor over an unreadable status.
            Err(_) => break,
        }
    }
    Some(child)
}

impl PowerCapability for LinuxPower {
    fn inhibit_sleep(&self, _reason: &str) -> Result<InhibitorGuard> {
        let child = spawn_inhibitor(
            "systemd-inhibit",
            &[
                "--what=sleep:idle",
                "--who=Aleph",
                "--why=Preventing sleep during AI operation",
                "--mode=block",
                "sleep",
                "infinity",
            ],
        )
        .or_else(|| {
            spawn_inhibitor(
                "gnome-session-inhibit",
                &[
                    "--inhibit",
                    "idle:suspend",
                    "--app-id",
                    "Aleph",
                    "--reason",
                    "Preventing sleep during AI operation",
                    "--",
                    "sleep",
                    "infinity",
                ],
            )
        })
        .ok_or_else(|| {
            DesktopError::PlatformError(
                "Could not inhibit sleep: neither systemd-inhibit nor gnome-session-inhibit \
                 took the lock. On a headless host or one without an active logind session the \
                 request is refused by policy, and there is nothing to inhibit. Install systemd \
                 or gnome-session, or run Aleph inside a desktop session."
                    .into(),
            )
        })?;

        Ok(InhibitorGuard::new(Box::new(move || {
            // The inhibitor child runs `sleep infinity` — it never exits on
            // its own. Kill it to release the inhibit lock (that is what the
            // guard is *for*), then reap the zombie. `wait_with_output()`
            // here would block this drop forever.
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _ = LinuxPower;
    }

    #[test]
    fn a_missing_binary_is_not_an_inhibitor() {
        assert!(spawn_inhibitor("aleph-no-such-inhibitor-xyz", &[]).is_none());
    }

    #[test]
    fn a_process_that_exits_immediately_is_not_an_inhibitor() {
        // The shape of a refused `systemd-inhibit`: it starts fine, declines to
        // take the lock, and exits without running its command. Reporting a
        // guard for that is the bug — a caller told sleep is inhibited when it
        // is not has no way to find out.
        assert!(spawn_inhibitor("true", &[]).is_none());
        assert!(spawn_inhibitor("false", &[]).is_none());
    }

    #[test]
    fn a_process_that_keeps_running_is_an_inhibitor() {
        let child = spawn_inhibitor("sleep", &["30"]);
        let mut child = child.expect("a live child is a held lock");
        let _ = child.kill();
        let _ = child.wait();
    }
}
