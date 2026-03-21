//! macOS `SystemCapability` implementation using `tokio::process::Command`.

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;
use tokio::process::Command;

/// macOS system capability implementation.
///
/// Uses `open`, `osascript`, `pbcopy`, `pbpaste`, and `sw_vers` under the hood.
pub struct MacOSSystem {
    _private: (),
}

impl MacOSSystem {
    /// Create a new `MacOSSystem` instance.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MacOSSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Escape double quotes in a string for safe embedding in AppleScript.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[async_trait]
impl SystemCapability for MacOSSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        let output = Command::new("open")
            .arg("-a")
            .arg(app_name)
            .output()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to run open: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "open -a {app_name} failed: {stderr}"
            )));
        }
        Ok(())
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        let escaped = escape_applescript(app_name);
        let script = format!("tell application \"{escaped}\" to quit");

        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to run osascript: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "quit_app({app_name}) failed: {stderr}"
            )));
        }
        Ok(())
    }

    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        // AppleScript that outputs tab-separated: name\tpid\tbundle_id\tis_frontmost
        let script = r#"
tell application "System Events"
    set appList to ""
    set allProcs to every application process
    repeat with proc in allProcs
        set appName to name of proc
        set appPid to unix id of proc
        set appBundle to bundle identifier of proc
        set appFront to frontmost of proc
        if appBundle is missing value then
            set appBundle to ""
        end if
        set appList to appList & appName & tab & appPid & tab & appBundle & tab & appFront & linefeed
    end repeat
    return appList
end tell
"#;

        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .await
            .map_err(|e| {
                DesktopError::InputFailed(format!("failed to run osascript: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "list_running_apps failed: {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut apps = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 4 {
                continue;
            }
            let name = parts[0].to_string();
            let pid = parts[1].parse::<u64>().ok();
            let bundle_id = parts[2].to_string();
            let is_active = parts[3].eq_ignore_ascii_case("true");

            apps.push(AppInfo {
                name,
                bundle_id,
                pid,
                is_active,
            });
        }

        Ok(apps)
    }

    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        let escaped_title = escape_applescript(title);
        let escaped_body = escape_applescript(body);
        let script = format!(
            "display notification \"{escaped_body}\" with title \"{escaped_title}\""
        );

        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to run osascript: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "send_notification failed: {stderr}"
            )));
        }
        Ok(())
    }

    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        let output = Command::new("pbpaste")
            .output()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to run pbpaste: {e}")))?;

        let text = String::from_utf8_lossy(&output.stdout).to_string();
        let text = if text.is_empty() { None } else { Some(text) };

        Ok(ClipboardContent {
            text,
            has_image: false,
            image_base64: None,
        })
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let mut child = Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| DesktopError::InputFailed(format!("failed to spawn pbcopy: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|e| DesktopError::InputFailed(format!("failed to write to pbcopy: {e}")))?;
            // Drop stdin to close the pipe so pbcopy finishes.
        }

        let status = child
            .wait()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("pbcopy failed: {e}")))?;

        if !status.success() {
            return Err(DesktopError::InputFailed("pbcopy exited with error".into()));
        }
        Ok(())
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        // OS version via sw_vers
        let os_version = {
            let output = Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .await
                .map_err(|e| {
                    DesktopError::InputFailed(format!("failed to run sw_vers: {e}"))
                })?;
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };

        // Hostname via hostname crate
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        // Architecture from Rust std
        let arch = std::env::consts::ARCH.to_string();

        // Username from environment
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "unknown".to_string());

        Ok(SystemInfo {
            os_name: "macOS".to_string(),
            os_version,
            hostname,
            arch,
            username,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_system_info() {
        let sys = MacOSSystem::new();
        let info = sys.system_info().await.unwrap();
        assert_eq!(info.os_name, "macOS");
        assert!(!info.hostname.is_empty());
        assert!(!info.username.is_empty());
    }

    #[tokio::test]
    async fn test_clipboard_roundtrip() {
        let sys = MacOSSystem::new();
        let test_text = "aleph-test-clipboard-12345";
        sys.clipboard_write(test_text).await.unwrap();
        let content = sys.clipboard_read().await.unwrap();
        assert_eq!(content.text.as_deref(), Some(test_text));
    }
}
