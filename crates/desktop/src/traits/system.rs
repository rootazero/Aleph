//! System-level capability (app management, clipboard, notifications).

use async_trait::async_trait;

use crate::system_types::*;
use crate::Result;

/// System-level operations: app lifecycle, clipboard, notifications, system info.
#[async_trait]
pub trait SystemCapability: Send + Sync {
    /// Launch an application by name or bundle ID.
    async fn launch_app(&self, app_name: &str) -> Result<()>;

    /// Quit a running application by name or bundle ID.
    async fn quit_app(&self, app_name: &str) -> Result<()>;

    /// List currently running applications.
    async fn list_running_apps(&self) -> Result<Vec<AppInfo>>;

    /// Send a system notification.
    async fn send_notification(&self, title: &str, body: &str) -> Result<()>;

    /// Read the current clipboard content.
    async fn clipboard_read(&self) -> Result<ClipboardContent>;

    /// Write text to the clipboard.
    async fn clipboard_write(&self, text: &str) -> Result<()>;

    /// Get high-level system information.
    async fn system_info(&self) -> Result<SystemInfo>;
}
