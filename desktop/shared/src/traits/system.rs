//! System-level capability (app management, clipboard, notifications).

use async_trait::async_trait;

use crate::system_types::{AppInfo, ClipboardContent, SystemInfo};
use crate::Result;

/// System-level operations: app lifecycle, clipboard, notifications, system info.
#[async_trait]
pub trait SystemCapability: Send + Sync {
    /// Launch an application by name or bundle ID.
    async fn launch_app(&self, app_name: &str) -> Result<()>;

    /// Quit a running application by name or bundle ID.
    async fn quit_app(&self, app_name: &str) -> Result<()>;

    /// Restart an application by name or bundle ID.
    async fn restart_app(&self, app_name: &str) -> Result<()> {
        self.quit_app(app_name).await?;
        // Brief delay so the app can release locks / ports before relaunch.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        self.launch_app(app_name).await
    }

    /// Open a file or URL with the OS default application — the "double-click it"
    /// primitive (`open` / `xdg-open` / `start`).
    ///
    /// `target` is an absolute filesystem path or a URL. Unlike [`launch_app`](Self::launch_app),
    /// which resolves an *application*, this opens a *document* with whatever
    /// handler the OS registered for it (e.g. an `.html` file → default browser).
    ///
    /// The default delegates to the cross-platform [`crate::action::open`]; it
    /// runs on a blocking thread because the underlying handler is a subprocess.
    async fn open_path(&self, target: &str) -> Result<()> {
        let target = target.to_string();
        tokio::task::spawn_blocking(move || crate::action::open(&target))
            .await
            .map_err(|e| crate::DesktopError::InputFailed(format!("open_path task join error: {e}")))?
    }

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

    /// Seconds since last user input (keyboard/mouse).
    /// Returns `NotImplemented` on platforms without idle detection.
    async fn user_idle_seconds(&self) -> Result<f64> {
        Err(crate::DesktopError::NotImplemented(
            "user_idle_seconds not available on this platform".into(),
        ))
    }
}
