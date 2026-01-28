//! Browser Control Configuration
//!
//! Configuration types for Chrome/Chromium browser automation via CDP.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Browser service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Whether browser control is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Path to Chrome/Chromium executable (or "auto" for auto-detection)
    #[serde(default = "default_executable")]
    pub executable: String,

    /// CDP (Chrome DevTools Protocol) port
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,

    /// Run browser in headless mode
    #[serde(default = "default_headless")]
    pub headless: bool,

    /// Browser operation timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// User data directory for browser profile
    #[serde(default = "default_user_data_dir")]
    pub user_data_dir: String,

    /// Viewport width
    #[serde(default = "default_viewport_width")]
    pub viewport_width: u32,

    /// Viewport height
    #[serde(default = "default_viewport_height")]
    pub viewport_height: u32,

    /// Additional Chrome arguments
    #[serde(default)]
    pub extra_args: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_executable() -> String {
    "auto".to_string()
}

fn default_cdp_port() -> u16 {
    9222
}

fn default_headless() -> bool {
    true
}

fn default_timeout() -> u64 {
    30
}

fn default_user_data_dir() -> String {
    "~/.aether/browser".to_string()
}

fn default_viewport_width() -> u32 {
    1280
}

fn default_viewport_height() -> u32 {
    720
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            executable: default_executable(),
            cdp_port: default_cdp_port(),
            headless: default_headless(),
            timeout_secs: default_timeout(),
            user_data_dir: default_user_data_dir(),
            viewport_width: default_viewport_width(),
            viewport_height: default_viewport_height(),
            extra_args: Vec::new(),
        }
    }
}

impl BrowserConfig {
    /// Expand the user data directory path (resolve ~)
    pub fn expand_user_data_dir(&self) -> PathBuf {
        if self.user_data_dir.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(&self.user_data_dir[2..]);
            }
        }
        PathBuf::from(&self.user_data_dir)
    }

    /// Find Chrome/Chromium executable
    pub fn find_executable(&self) -> Option<PathBuf> {
        if self.executable != "auto" {
            let path = PathBuf::from(&self.executable);
            if path.exists() {
                return Some(path);
            }
            return None;
        }

        // Check environment variable first
        if let Ok(path) = std::env::var("CHROME_PATH") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Some(path);
            }
        }

        // Platform-specific paths
        #[cfg(target_os = "macos")]
        {
            let paths = [
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
                "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
            ];
            for p in paths {
                let path = PathBuf::from(p);
                if path.exists() {
                    return Some(path);
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            let paths = [
                "/usr/bin/google-chrome",
                "/usr/bin/google-chrome-stable",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
                "/snap/bin/chromium",
            ];
            for p in paths {
                let path = PathBuf::from(p);
                if path.exists() {
                    return Some(path);
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            let paths = [
                r"C:\Program Files\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
                r"C:\Program Files\Chromium\Application\chrome.exe",
            ];
            for p in paths {
                let path = PathBuf::from(p);
                if path.exists() {
                    return Some(path);
                }
            }
        }

        // Try PATH
        if let Ok(output) = std::process::Command::new("which")
            .arg("chromium")
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }

        None
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.cdp_port == 0 {
            return Err("cdp_port must be > 0".to_string());
        }
        if self.timeout_secs == 0 {
            return Err("timeout_secs must be > 0".to_string());
        }
        if self.viewport_width == 0 || self.viewport_height == 0 {
            return Err("viewport dimensions must be > 0".to_string());
        }
        Ok(())
    }
}

/// Browser tab information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabInfo {
    /// Tab/target ID
    pub id: String,
    /// Page URL
    pub url: String,
    /// Page title
    pub title: String,
    /// Whether this tab is active
    pub active: bool,
}

/// Screenshot options
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenshotOptions {
    /// Capture full page (not just viewport)
    #[serde(default)]
    pub full_page: bool,
    /// Image format (png or jpeg)
    #[serde(default = "default_format")]
    pub format: String,
    /// JPEG quality (1-100, only for jpeg format)
    #[serde(default = "default_quality")]
    pub quality: u8,
    /// Optional element selector to screenshot
    pub selector: Option<String>,
}

fn default_format() -> String {
    "png".to_string()
}

fn default_quality() -> u8 {
    80
}

/// Snapshot node from accessibility tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotNode {
    /// Element reference (e1, e2, ...)
    pub ref_id: String,
    /// ARIA role
    pub role: String,
    /// Accessible name
    pub name: String,
    /// Element value (for inputs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Tree depth
    pub depth: u32,
    /// Whether element is interactive
    pub interactive: bool,
}

/// Page snapshot result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSnapshot {
    /// Page URL
    pub url: String,
    /// Page title
    pub title: String,
    /// Snapshot nodes
    pub nodes: Vec<SnapshotNode>,
    /// Total element count
    pub total_elements: usize,
    /// Interactive element count
    pub interactive_count: usize,
    /// Whether snapshot was truncated
    pub truncated: bool,
}

/// Click action options
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClickOptions {
    /// Double click
    #[serde(default)]
    pub double_click: bool,
    /// Mouse button (left, right, middle)
    #[serde(default = "default_button")]
    pub button: String,
    /// Delay after click in milliseconds
    #[serde(default)]
    pub delay_ms: u64,
}

fn default_button() -> String {
    "left".to_string()
}

/// Type action options
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TypeOptions {
    /// Clear existing text before typing
    #[serde(default)]
    pub clear: bool,
    /// Submit form after typing (press Enter)
    #[serde(default)]
    pub submit: bool,
    /// Type slowly with delay between keystrokes
    #[serde(default)]
    pub slowly: bool,
    /// Delay between keystrokes in milliseconds (when slowly=true)
    #[serde(default = "default_keystroke_delay")]
    pub keystroke_delay_ms: u64,
}

fn default_keystroke_delay() -> u64 {
    75
}

/// Browser action result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    /// Whether action succeeded
    pub ok: bool,
    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Additional data
    #[serde(flatten)]
    pub data: serde_json::Value,
}

impl ActionResult {
    pub fn success() -> Self {
        Self {
            ok: true,
            error: None,
            data: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    pub fn success_with_data(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            error: None,
            data,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            data: serde_json::Value::Object(serde_json::Map::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_default() {
        let config = BrowserConfig::default();
        assert!(config.enabled);
        assert_eq!(config.executable, "auto");
        assert_eq!(config.cdp_port, 9222);
        assert!(config.headless);
    }

    #[test]
    fn test_browser_config_validate() {
        let mut config = BrowserConfig::default();
        assert!(config.validate().is_ok());

        config.cdp_port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_action_result() {
        let success = ActionResult::success();
        assert!(success.ok);
        assert!(success.error.is_none());

        let error = ActionResult::error("test error");
        assert!(!error.ok);
        assert_eq!(error.error, Some("test error".to_string()));
    }
}
