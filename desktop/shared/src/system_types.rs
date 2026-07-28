//! Shared types for system-level capabilities.

use serde::{Deserialize, Serialize};

/// Information about a running or installed application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    /// Application name.
    pub name: String,
    /// Bundle identifier (macOS) or executable path.
    pub bundle_id: String,
    /// Process ID if the application is currently running, `None` if not running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u64>,
    /// Whether the application is currently active (frontmost).
    pub is_active: bool,
}

/// An application that exists on this machine, whether or not it is running.
///
/// Distinct from [`AppInfo`], which describes a *process*. The distinction is
/// load-bearing: `launch_app` takes a name or a bundle identifier, and until
/// this existed the only list a model could obtain was of things already
/// running — so "open Numbers" had no path to `com.apple.Numbers` except a
/// guess. `AppInfo` cannot stand in for it, because its `pid`/`is_active` fields
/// have no meaning for something that is not running and reporting them as
/// absent/false would read as a claim about a process rather than about a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    /// Display name, as shown in Finder (the bundle's file stem).
    pub name: String,
    /// Bundle identifier, e.g. `com.apple.Numbers`. Empty when the bundle does
    /// not declare one — the name is then the only usable handle.
    pub bundle_id: String,
    /// Absolute path to the application bundle.
    pub path: String,
}

/// Content read from or written to the system clipboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardContent {
    /// The primary text content, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Whether the clipboard also contains an image.
    pub has_image: bool,
    /// Base64-encoded PNG image data, if an image is present and was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
}

/// High-level system information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    /// Operating system name (e.g. "macOS", "Windows", "Linux").
    pub os_name: String,
    /// OS version string.
    pub os_version: String,
    /// Hostname.
    pub hostname: String,
    /// CPU architecture (e.g. "aarch64", "`x86_64`").
    pub arch: String,
    /// Current user's username.
    pub username: String,
}
