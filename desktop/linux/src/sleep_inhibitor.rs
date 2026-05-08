use aleph_desktop::{
    error::{DesktopError, Result},
    traits::{InhibitorGuard, PowerCapability},
};

pub struct LinuxPower;

impl LinuxPower {
    pub fn new() -> Self {
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
        let child = std::process::Command::new("systemd-inhibit")
            .args([
                "--what=sleep:idle",
                "--who=Aleph",
                "--why=Preventing sleep during AI operation",
                "--mode=block",
                "sleep",
                "infinity",
            ])
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
                    .spawn()
            })
            .map_err(|e| {
                DesktopError::PlatformError(format!(
                    "Failed to inhibit sleep (install systemd or gnome-session): {e}"
                ))
            })?;

        Ok(InhibitorGuard::new(Box::new(move || {
            let _ = child.wait_with_output();
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _power = LinuxPower::default();
    }
}
