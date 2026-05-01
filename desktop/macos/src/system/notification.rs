//! Notifications via osascript.
//!
//! UNUserNotificationCenter requires `block2` crate for the completion handler
//! and a bundle identifier (not available when running as CLI). For now, we use
//! osascript which works universally. UNUserNotificationCenter support will be
//! added when Aleph ships as a bundled .app (Direction 2).

use aleph_desktop::{DesktopError, Result};

/// Send a system notification via osascript.
pub async fn send_notification(title: &str, body: &str) -> Result<()> {
    let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escaped_body, escaped_title
    );

    let output = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .await
        .map_err(|e| {
            DesktopError::InputFailed(format!("notification: failed to run osascript: {e}"))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::InputFailed(format!(
            "notification failed: {stderr}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_send_notification_happy_path() {
        let result = send_notification("test title", "test body").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_notification_escapes_backslash() {
        let result = send_notification(r"has\backslash", "body").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_notification_escapes_quotes() {
        let result = send_notification("title with \"quotes\"", "body").await;
        assert!(result.is_ok());
    }
}
