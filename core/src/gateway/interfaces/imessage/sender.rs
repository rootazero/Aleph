//! AppleScript Message Sender
//!
//! Sends iMessage/SMS messages using AppleScript and the Messages.app.
//!
//! # Requirements
//!
//! - macOS with Messages.app
//! - Automation permission for the calling application
//!
//! # Usage
//!
//! ```ignore
//! MessageSender::send_text("+15551234567", "Hello!").await?;
//! MessageSender::send_file("+15551234567", Path::new("/path/to/file.png")).await?;
//! ```

use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, error, trace};

/// Error type for message sending
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("AppleScript execution failed: {0}")]
    ScriptFailed(String),

    #[error("Invalid target: {0}")]
    InvalidTarget(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// AppleScript-based message sender
pub struct MessageSender;

impl MessageSender {
    /// Send a text message to a phone number or email
    ///
    /// # Arguments
    ///
    /// * `to` - Target phone number (e.g., "+15551234567") or email
    /// * `text` - Message text to send
    ///
    /// # Example
    ///
    /// ```ignore
    /// MessageSender::send_text("+15551234567", "Hello from Aleph!").await?;
    /// ```
    pub async fn send_text(to: &str, text: &str) -> Result<(), SendError> {
        if to.is_empty() {
            return Err(SendError::InvalidTarget("Empty target".to_string()));
        }

        if text.is_empty() {
            debug!("Skipping empty message to {}", to);
            return Ok(());
        }

        debug!("Sending text to {}: {}...", to, &text[..text.len().min(50)]);

        // Escape the text for AppleScript
        let escaped_text = escape_applescript_string(text);
        let escaped_to = escape_applescript_string(to);

        // Build the AppleScript
        let script = format!(
            r#"
            tell application "Messages"
                set targetService to 1st account whose service type = iMessage
                set targetBuddy to participant "{}" of targetService
                send "{}" to targetBuddy
            end tell
            "#,
            escaped_to, escaped_text
        );

        execute_applescript(&script).await?;
        debug!("Message sent successfully to {}", to);
        Ok(())
    }

    /// Send a file attachment to a phone number or email
    ///
    /// # Arguments
    ///
    /// * `to` - Target phone number or email
    /// * `file_path` - Path to the file to send
    ///
    /// # Example
    ///
    /// ```ignore
    /// MessageSender::send_file("+15551234567", Path::new("/path/to/photo.jpg")).await?;
    /// ```
    pub async fn send_file(to: &str, file_path: &Path) -> Result<(), SendError> {
        if to.is_empty() {
            return Err(SendError::InvalidTarget("Empty target".to_string()));
        }

        if !file_path.exists() {
            return Err(SendError::FileNotFound(
                file_path.to_string_lossy().to_string(),
            ));
        }

        debug!("Sending file to {}: {}", to, file_path.display());

        let escaped_to = escape_applescript_string(to);
        let file_path_str = file_path.to_string_lossy();

        // Build the AppleScript for file sending
        let script = format!(
            r#"
            tell application "Messages"
                set targetService to 1st account whose service type = iMessage
                set targetBuddy to participant "{}" of targetService
                set theFile to POSIX file "{}"
                send theFile to targetBuddy
            end tell
            "#,
            escaped_to, file_path_str
        );

        execute_applescript(&script).await?;
        debug!("File sent successfully to {}", to);
        Ok(())
    }

    /// Send a message to a specific chat by ID
    ///
    /// This is useful for group chats where you have the chat identifier.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - The chat identifier (from chat.db)
    /// * `text` - Message text to send
    pub async fn send_to_chat(chat_id: &str, text: &str) -> Result<(), SendError> {
        if chat_id.is_empty() {
            return Err(SendError::InvalidTarget("Empty chat ID".to_string()));
        }

        debug!("Sending to chat {}: {}...", chat_id, &text[..text.len().min(50)]);

        let escaped_text = escape_applescript_string(text);
        let escaped_chat_id = escape_applescript_string(chat_id);

        // Build the AppleScript for chat-based sending
        let script = format!(
            r#"
            tell application "Messages"
                set targetChat to chat id "{}"
                send "{}" to targetChat
            end tell
            "#,
            escaped_chat_id, escaped_text
        );

        execute_applescript(&script).await?;
        debug!("Message sent to chat {}", chat_id);
        Ok(())
    }

    /// Check if Messages.app is available
    pub async fn is_available() -> bool {
        let script = r#"
            tell application "System Events"
                return exists application process "Messages"
            end tell
        "#;

        match execute_applescript(script).await {
            Ok(output) => output.trim() == "true",
            Err(_) => false,
        }
    }

    /// Open Messages.app if not running
    pub async fn ensure_running() -> Result<(), SendError> {
        let script = r#"
            tell application "Messages"
                activate
            end tell
        "#;

        execute_applescript(script).await?;
        Ok(())
    }

    /// Send a tapback reaction to a message
    ///
    /// # Arguments
    ///
    /// * `chat_id` - The chat identifier
    /// * `message_guid` - The GUID of the message to react to
    /// * `tapback` - The tapback type (love, like, dislike, laugh, emphasize, question)
    /// * `remove` - Whether to remove the tapback
    ///
    /// # Note
    ///
    /// iMessage tapbacks are complex to implement via AppleScript.
    /// This is a best-effort implementation that may not work on all macOS versions.
    pub async fn send_tapback(
        chat_id: &str,
        message_guid: &str,
        tapback: &str,
        remove: bool,
    ) -> Result<(), SendError> {
        if chat_id.is_empty() {
            return Err(SendError::InvalidTarget("Empty chat ID".to_string()));
        }

        debug!(
            "Sending tapback {} to message {} in chat {} (remove: {})",
            tapback, message_guid, chat_id, remove
        );

        // Note: AppleScript doesn't have direct support for sending tapbacks.
        // This would require using the Accessibility API or a more complex approach.
        // For now, we'll return an error indicating this limitation.

        // The proper implementation would use something like:
        // - UI scripting via System Events
        // - Private iMessage framework (SPI)
        // - Or a third-party solution like BlueBubbles

        Err(SendError::ScriptFailed(
            "Tapback reactions via AppleScript are not fully supported. \
             Consider using BlueBubbles or similar for full tapback support."
                .to_string(),
        ))
    }
}

/// Execute an AppleScript and return the output
async fn execute_applescript(script: &str) -> Result<String, SendError> {
    trace!("Executing AppleScript:\n{}", script);

    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        trace!("AppleScript output: {}", stdout);
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        error!("AppleScript failed: {}", stderr);
        Err(SendError::ScriptFailed(stderr))
    }
}

/// Escape a string for use in AppleScript
fn escape_applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_applescript_string() {
        assert_eq!(escape_applescript_string("hello"), "hello");
        assert_eq!(escape_applescript_string("he\"llo"), "he\\\"llo");
        assert_eq!(escape_applescript_string("line1\nline2"), "line1\\nline2");
        assert_eq!(escape_applescript_string("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_escape_special_chars() {
        let input = "Hello \"World\"\nNew line\tTab";
        let escaped = escape_applescript_string(input);
        assert_eq!(escaped, "Hello \\\"World\\\"\\nNew line\\tTab");
    }

    #[tokio::test]
    #[ignore] // Requires macOS with Messages.app
    async fn test_is_available() {
        // This test only works on macOS
        let available = MessageSender::is_available().await;
        println!("Messages.app available: {}", available);
    }
}
