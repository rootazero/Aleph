//! Linux `SystemCapability` implementation.
//!
//! Uses standard POSIX / freedesktop CLI tools with graceful fallbacks.
//! Tested on Debian/Ubuntu; should work on most distributions.

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct LinuxSystem;

impl LinuxSystem {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for LinuxSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemCapability for LinuxSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || {
            // Prefer gtk-launch (GNOME/GTK environments) for .desktop files.
            let status = std::process::Command::new("gtk-launch")
                .arg(&app_name)
                .status();

            let status = match status {
                Ok(s) if s.success() => return Ok(()),
                _ => std::process::Command::new("xdg-open")
                    .arg(&app_name)
                    .status()
                    .map_err(|e| {
                        DesktopError::InputFailed(format!("Failed to launch app '{app_name}': {e}"))
                    })?,
            };

            if status.success() {
                Ok(())
            } else {
                Err(DesktopError::InputFailed(format!(
                    "Failed to launch '{app_name}'"
                )))
            }
        })
        .await
        .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || {
            // Try killall first (safer than pkill -f).
            let status = std::process::Command::new("killall")
                .arg(&app_name)
                .status();

            let status = match status {
                Ok(s) if s.success() => return Ok(()),
                _ => std::process::Command::new("pkill")
                    .args(["-f", &app_name])
                    .status()
                    .map_err(|e| {
                        DesktopError::InputFailed(format!("Failed to quit app '{app_name}': {e}"))
                    })?,
            };

            if status.success() {
                Ok(())
            } else {
                Err(DesktopError::InputFailed(format!(
                    "No running application found matching '{app_name}'"
                )))
            }
        })
        .await
        .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        tokio::task::spawn_blocking(|| {
            let output = std::process::Command::new("ps")
                .args(["-eo", "comm,pid", "--no-headers"])
                .output()
                .map_err(|e| {
                    DesktopError::PlatformError(format!("Failed to list running apps: {e}"))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(DesktopError::PlatformError(format!(
                    "ps failed: {}",
                    stderr.trim()
                )));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut apps = Vec::new();
            let mut seen = std::collections::HashSet::new();

            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 {
                    continue;
                }

                let name = parts[0].trim_start_matches("./").to_string();
                let pid = parts[1].parse::<u64>().unwrap_or(0);

                if name.is_empty()
                    || seen.contains(&name)
                    || name.starts_with('[')
                    || name.starts_with("kworker")
                    || name.starts_with("systemd-")
                    || name == "ps"
                    || name == "bash"
                    || name == "sh"
                {
                    continue;
                }

                seen.insert(name.clone());
                apps.push(AppInfo {
                    name,
                    bundle_id: parts[0].to_string(),
                    pid: Some(pid),
                    is_active: false,
                });
            }

            Ok(apps)
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        let title = title.to_string();
        let body = body.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let status = std::process::Command::new("notify-send")
                .args([&title, &body, "--app-name=Aleph"])
                .status()
                .map_err(|e| {
                    DesktopError::PlatformError(format!(
                        "Failed to send notification (install libnotify-bin): {e}"
                    ))
                })?;

            if status.success() {
                Ok(())
            } else {
                Err(DesktopError::PlatformError(
                    "notify-send returned non-zero exit code".into(),
                ))
            }
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        // Delegates to the `clipboard` module, which reads text plus an
        // optional image (base64 PNG) — see desktop/linux/src/clipboard.rs.
        tokio::task::spawn_blocking(crate::clipboard::read)
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || crate::clipboard::write(&text))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        tokio::task::spawn_blocking(|| {
            let os_version = read_os_version().unwrap_or_else(|| "unknown".to_string());
            let hostname = read_hostname().unwrap_or_else(|| "unknown".to_string());
            let username = std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "unknown".to_string());

            Ok(SystemInfo {
                os_name: "Linux".to_string(),
                os_version,
                hostname,
                arch: std::env::consts::ARCH.to_string(),
                username,
            })
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    async fn user_idle_seconds(&self) -> Result<f64> {
        tokio::task::spawn_blocking(|| {
            let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
            let is_wayland = session_type.eq_ignore_ascii_case("wayland");

            // Wayland: prefer GNOME Mutter D-Bus IdleMonitor, which works for any
            // logged-in user on a modern GNOME-on-Wayland session and does not
            // depend on running an X server. xprintidle reports 0 forever on a
            // pure Wayland compositor.
            if is_wayland {
                if let Some(secs) = query_mutter_idle_seconds() {
                    return Ok(secs);
                }
            }

            // X11 / XWayland: xprintidle is the standard tool. Some distros ship
            // it by default; others require `apt install xprintidle`.
            if let Ok(output) = std::process::Command::new("xprintidle").output() {
                if output.status.success() {
                    let ms = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0);
                    return Ok(ms as f64 / 1000.0);
                }
            }

            // Last-chance Wayland fallback even if XDG_SESSION_TYPE was unset.
            if !is_wayland {
                if let Some(secs) = query_mutter_idle_seconds() {
                    return Ok(secs);
                }
            }

            let hint = if is_wayland {
                "Idle detection on this Wayland session needs GNOME's Mutter \
                 IdleMonitor (gdbus / dbus-send + a running GNOME shell)"
            } else {
                "Idle detection requires xprintidle on X11 \
                 (install: sudo apt install xprintidle) \
                 or GNOME Mutter IdleMonitor on Wayland"
            };
            Err(DesktopError::NotImplemented(hint.into()))
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }
}

/// Query GNOME Mutter's `IdleMonitor` over the session bus via `gdbus`.
///
/// Returns the idle time in seconds, or `None` if `gdbus` is unavailable,
/// the `IdleMonitor` object is not present (no GNOME shell), or the reply
/// cannot be parsed. The reply format is `(uint64 N,)` where N is millis.
///
/// Shells out instead of taking a `zbus` dependency: idle detection is a
/// rare query and `gdbus` is part of every GNOME install (it lives in
/// `glib2-tools` / `libglib2.0-bin`).
fn query_mutter_idle_seconds() -> Option<f64> {
    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Mutter.IdleMonitor",
            "--object-path",
            "/org/gnome/Mutter/IdleMonitor/Core",
            "--method",
            "org.gnome.Mutter.IdleMonitor.GetIdletime",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_mutter_idle_reply(&stdout)
}

/// Parse `gdbus`'s textual reply `(uint64 12345,)` into seconds.
fn parse_mutter_idle_reply(reply: &str) -> Option<f64> {
    let trimmed = reply.trim();
    let inner = trimmed.strip_prefix('(')?.strip_suffix(",)")?;
    let inner = inner.trim();
    // Accept the typed form ("uint64 12345") and the bare numeric form ("12345").
    let digits = inner.strip_prefix("uint64").unwrap_or(inner).trim();
    let ms: u64 = digits.parse().ok()?;
    Some(ms as f64 / 1000.0)
}

fn read_os_version() -> Option<String> {
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                return Some(value.trim_matches('"').to_string());
            }
        }
    }

    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}

fn read_hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .or_else(|| {
            std::process::Command::new("uname")
                .arg("-n")
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _ = LinuxSystem;
    }

    #[test]
    fn test_system_info() {
        let sys = LinuxSystem::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let info = rt.block_on(sys.system_info()).unwrap();
        assert_eq!(info.os_name, "Linux");
        assert!(!info.hostname.is_empty());
        assert!(!info.arch.is_empty());
    }

    #[test]
    fn parses_mutter_idle_typed_reply() {
        // gdbus typically prints the type tag.
        let out = parse_mutter_idle_reply("(uint64 12345,)\n").unwrap();
        assert!((out - 12.345).abs() < 1e-6);
    }

    #[test]
    fn parses_mutter_idle_bare_numeric_reply() {
        // Some glib versions omit the tag.
        let out = parse_mutter_idle_reply("(2500,)").unwrap();
        assert!((out - 2.5).abs() < 1e-6);
    }

    #[test]
    fn parses_mutter_idle_zero() {
        let out = parse_mutter_idle_reply("(uint64 0,)").unwrap();
        assert_eq!(out, 0.0);
    }

    #[test]
    fn rejects_malformed_mutter_idle_reply() {
        assert!(parse_mutter_idle_reply("garbage").is_none());
        assert!(parse_mutter_idle_reply("(uint64 not_a_number,)").is_none());
        assert!(parse_mutter_idle_reply("()").is_none());
    }
}
