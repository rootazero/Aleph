use std::process::Stdio;

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

impl PowerCapability for LinuxPower {
    fn inhibit_sleep(&self, _reason: &str) -> Result<InhibitorGuard> {
        // Silence the child's stdio. `spawn()` succeeds whenever the binary
        // exists (systemd is present on most servers), so a headless box with no
        // active logind session still spawns `systemd-inhibit`, which then has
        // its own inhibit request denied by polkit and prints
        // "Failed to inhibit: Access denied" to stderr. Because the child
        // inherits the parent's stdio by default, on a daemon whose stdout/stderr
        // are redirected to a log file that message floods the log — once per
        // agent turn. Redirect the child's stdout/stderr to /dev/null so its
        // chatter never reaches our stream. On a real desktop session the child
        // succeeds silently, so this is a no-op there.
        let child = std::process::Command::new("systemd-inhibit")
            .args([
                "--what=sleep:idle",
                "--who=Aleph",
                "--why=Preventing sleep during AI operation",
                "--mode=block",
                "sleep",
                "infinity",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .or_else(|_| {
                std::process::Command::new("gnome-session-inhibit")
                    .args([
                        "--inhibit",
                        "idle:suspend",
                        "--app-id",
                        "Aleph",
                        "--reason",
                        "Preventing sleep during AI operation",
                        "--",
                        "sleep",
                        "infinity",
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
            })
            .map_err(|e| {
                DesktopError::PlatformError(format!(
                    "Failed to inhibit sleep (install systemd or gnome-session): {e}"
                ))
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
}
