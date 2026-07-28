//! Notifications via osascript.
//!
//! `UNUserNotificationCenter` requires `block2` crate for the completion handler
//! and a bundle identifier (not available when running as CLI). For now, we use
//! osascript which works universally. `UNUserNotificationCenter` support will be
//! added when Aleph ships as a bundled .app (Direction 2).

use std::time::Duration;

use aleph_desktop::script_exec::output_capped;
use aleph_desktop::{DesktopError, Result};

/// Hard ceiling for the `osascript` call.
///
/// A notification is a fire-and-forget nicety on the R5 push path; when
/// Notification Center is wedged the *caller* must not be. This was the last
/// unbounded `.output()` on the macOS limb.
const NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Send a system notification via osascript.
pub async fn send_notification(title: &str, body: &str) -> Result<()> {
    let escaped_title = escape_applescript(title);
    let escaped_body = escape_applescript(body);
    let script = format!("display notification \"{escaped_body}\" with title \"{escaped_title}\"");

    let mut cmd = tokio::process::Command::new("osascript");
    cmd.arg("-e").arg(&script);
    let output = output_capped(cmd, NOTIFICATION_TIMEOUT).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::InputFailed(format!(
            "notification failed: {stderr}"
        )));
    }
    Ok(())
}

/// Escape a string for embedding inside an AppleScript double-quoted literal.
///
/// Beyond `\` and `"`, a raw newline is fatal: an AppleScript string literal
/// cannot contain a literal newline, so a multi-line title/body made osascript
/// return a compile error and the whole notification failed (R5 push summaries
/// routinely carry multi-line text). `\n`/`\r` are rewritten to their escape
/// sequences, which AppleScript interprets as the corresponding control chars.
/// Mirrors `escapeAppleScript` in the Swift helper.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_send_notification_happy_path() {
        let result = send_notification("test title", "test body").await;
        assert!(result.is_ok());
    }

    #[test]
    fn escape_applescript_rewrites_newlines_and_metachars() {
        assert_eq!(escape_applescript("a\nb"), "a\\nb");
        assert_eq!(escape_applescript("a\r\nb"), "a\\r\\nb");
        assert_eq!(escape_applescript(r"back\slash"), r"back\\slash");
        assert_eq!(escape_applescript("q\"uote"), "q\\\"uote");
        // Order matters: the backslash pass must run first so the escapes it
        // introduces for \n/\r are not themselves doubled.
        assert_eq!(escape_applescript("x\\\ny"), "x\\\\\\ny");
    }

    #[tokio::test]
    async fn test_send_notification_multiline_body_ok() {
        let result = send_notification("title", "line one\nline two").await;
        assert!(result.is_ok(), "multi-line body must not fail: {result:?}");
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
