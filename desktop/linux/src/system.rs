//! Linux `SystemCapability` implementation.
//!
//! Uses standard POSIX / freedesktop CLI tools with graceful fallbacks.
//! Tested on Debian/Ubuntu; should work on most distributions.

use std::io::Write;

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct LinuxSystem;

impl LinuxSystem {
    pub fn new() -> Self {
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
        tokio::task::spawn_blocking(|| {
            // Try wl-paste first (Wayland), then xclip/xsel (X11).
            let output = std::process::Command::new("wl-paste")
                .arg("--no-newline")
                .output()
                .or_else(|_| {
                    std::process::Command::new("xclip")
                        .args(["-selection", "clipboard", "-o"])
                        .output()
                })
                .or_else(|_| {
                    std::process::Command::new("xsel")
                        .args(["--clipboard", "--output"])
                        .output()
                })
                .map_err(|e| {
                    DesktopError::InputFailed(format!(
                        "Failed to read clipboard (install wl-clipboard, xclip, or xsel): {e}"
                    ))
                })?;

            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            Ok(ClipboardContent {
                text: Some(text),
                has_image: false,
                image_base64: None,
            })
        })
        .await
        .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || {
            // Try wl-copy first (Wayland), then xclip/xsel (X11).
            let mut child = std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .or_else(|_| {
                    std::process::Command::new("xclip")
                        .args(["-selection", "clipboard"])
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                })
                .or_else(|_| {
                    std::process::Command::new("xsel")
                        .args(["--clipboard", "--input"])
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                })
                .map_err(|e| {
                    DesktopError::InputFailed(format!(
                        "Failed to write clipboard (install wl-clipboard, xclip, or xsel): {e}"
                    ))
                })?;

            if let Some(ref mut stdin) = child.stdin {
                stdin.write_all(text.as_bytes()).map_err(|e| {
                    DesktopError::InputFailed(format!("Clipboard write failed: {e}"))
                })?;
            }

            child
                .wait()
                .map_err(|e| DesktopError::InputFailed(format!("Clipboard process failed: {e}")))?;

            Ok(())
        })
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
            // Try xprintidle first (X11 / XWayland).
            if let Ok(output) = std::process::Command::new("xprintidle").output() {
                if output.status.success() {
                    let ms = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0);
                    return Ok(ms as f64 / 1000.0);
                }
            }

            Err(DesktopError::NotImplemented(
                "Idle detection requires xprintidle (install: sudo apt install xprintidle)".into(),
            ))
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }
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
        let _sys = LinuxSystem::default();
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
}
