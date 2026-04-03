//! Application launch (platform-specific).

use tracing::info;

use crate::error::{DesktopError, Result};

/// Launch an application by name or bundle ID.
///
/// - **macOS**: `open -b <bundle_id>` (or `open -a <app_name>` if not a bundle ID)
/// - **Linux**: `xdg-open <app_name>`
/// - **Windows**: `cmd /C start "" "<app_name>"`
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the application cannot be launched.
/// - [`DesktopError::NotImplemented`] on unsupported platforms.
pub fn launch_app(app_name: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};

        let ws = NSWorkspace::sharedWorkspace();
        let ns_name = NSString::from_str(app_name);

        #[allow(deprecated)]
        let url = if app_name.contains('.') {
            ws.URLForApplicationWithBundleIdentifier(&ns_name)
        } else {
            ws.fullPathForApplication(&ns_name)
                .map(|p| NSURL::fileURLWithPath(&p))
        };

        let url = url.ok_or_else(|| {
            DesktopError::InputFailed(format!("Application '{}' not found", app_name))
        })?;

        if !ws.openURL(&url) {
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}'",
                app_name
            )));
        }

        info!(app_name, "App launched (macOS)");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("xdg-open")
            .arg(app_name)
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}",
                app_name,
                stderr.trim()
            )));
        }

        info!(app_name, "App launched (Linux)");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "start", "", app_name])
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}",
                app_name,
                stderr.trim()
            )));
        }

        info!(app_name, "App launched (Windows)");
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app_name;
        Err(DesktopError::NotImplemented(
            "launch_app not supported on this platform".into(),
        ))
    }
}
