use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unique identifier for a browser tab.
pub type TabId = String;

/// Configuration for launching or connecting to a browser instance.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserConfig {
    /// How to obtain a browser instance.
    pub mode: LaunchMode,

    /// Custom user data directory for the browser profile.
    pub user_data_dir: Option<String>,

    /// CDP (Chrome DevTools Protocol) port.
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,

    /// Whether to launch in headless mode.
    #[serde(default)]
    pub headless: bool,

    /// Extra command-line arguments passed to the browser binary.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

fn default_cdp_port() -> u16 {
    9222
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            mode: LaunchMode::Auto,
            user_data_dir: None,
            cdp_port: 9222,
            headless: false,
            extra_args: Vec::new(),
        }
    }
}

/// How to obtain a browser instance.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LaunchMode {
    /// Automatically detect and launch a browser.
    Auto,

    /// Connect to an existing browser via a WebSocket endpoint.
    Connect {
        /// WebSocket debugger URL (e.g. ws://127.0.0.1:9222/devtools/browser/...).
        endpoint: String,
    },

    /// Launch a specific browser binary.
    Binary {
        /// Path to the browser executable.
        path: String,
    },
}

/// Target for a browser action (click, hover, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionTarget {
    /// Target an element by its snapshot ref ID (e.g. "e42").
    Ref { ref_id: String },
    /// Target a viewport coordinate.
    Coordinates { x: f64, y: f64 },
}

/// Direction for scrolling.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

/// Options for taking a screenshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotOpts {
    /// Capture the full scrollable page instead of just the viewport.
    #[serde(default)]
    pub full_page: bool,

    /// Image format ("png" or "jpeg").
    #[serde(default = "default_screenshot_format")]
    pub format: String,

    /// Image quality (1-100, only applicable for jpeg).
    #[serde(default = "default_screenshot_quality")]
    pub quality: u8,
}

fn default_screenshot_format() -> String {
    "png".to_string()
}

fn default_screenshot_quality() -> u8 {
    80
}

impl Default for ScreenshotOpts {
    fn default() -> Self {
        Self {
            full_page: false,
            format: "png".to_string(),
            quality: 80,
        }
    }
}

/// Kind of web storage.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Local,
    Session,
}

/// Browser snapshot (text-first).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotOutput {
    /// Raw snapshot text — YAML from playwright-cli, indented-tree from chrome-devtools-mcp.
    pub snapshot_text: String,
    /// Page URL at snapshot time.
    pub page_url: String,
    /// Page title at snapshot time.
    pub page_title: String,
}

/// Screenshot output (raw PNG bytes).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScreenshotOutput {
    pub png_bytes: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_config_defaults() {
        let config = BrowserConfig::default();
        assert_eq!(config.cdp_port, 9222);
        assert!(!config.headless);
        assert!(config.extra_args.is_empty());
        assert!(config.user_data_dir.is_none());
        assert!(matches!(config.mode, LaunchMode::Auto));
    }

    #[test]
    fn test_action_target_serialization() {
        // Ref variant
        let target = ActionTarget::Ref {
            ref_id: "e42".to_string(),
        };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "ref");
        assert_eq!(json["ref_id"], "e42");

        // Coordinates variant
        let target = ActionTarget::Coordinates { x: 100.0, y: 200.0 };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["type"], "coordinates");
        assert_eq!(json["x"], 100.0);
        assert_eq!(json["y"], 200.0);

        // Round-trip deserialization
        let round_trip: ActionTarget = serde_json::from_value(json).unwrap();
        assert!(
            matches!(round_trip, ActionTarget::Coordinates { x, y } if x == 100.0 && y == 200.0)
        );
    }

    #[test]
    fn test_launch_mode_tagged_enum_serialization() {
        // Auto
        let mode = LaunchMode::Auto;
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["type"], "auto");

        // Connect
        let mode = LaunchMode::Connect {
            endpoint: "ws://127.0.0.1:9222".to_string(),
        };
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["type"], "connect");
        assert_eq!(json["endpoint"], "ws://127.0.0.1:9222");

        // Binary
        let mode = LaunchMode::Binary {
            path: "/usr/bin/chromium".to_string(),
        };
        let json = serde_json::to_value(&mode).unwrap();
        assert_eq!(json["type"], "binary");
        assert_eq!(json["path"], "/usr/bin/chromium");

        // Round-trip deserialization
        let round_trip: LaunchMode = serde_json::from_value(json).unwrap();
        assert!(matches!(round_trip, LaunchMode::Binary { path } if path == "/usr/bin/chromium"));
    }

    #[test]
    fn test_snapshot_output_serde_roundtrip() {
        let snap = SnapshotOutput {
            snapshot_text: "- button \"OK\" [ref=e1]".into(),
            page_url: "https://example.com/".into(),
            page_title: "Example".into(),
        };
        let json = serde_json::to_value(&snap).unwrap();
        let back: SnapshotOutput = serde_json::from_value(json).unwrap();
        assert_eq!(back.page_url, "https://example.com/");
    }

    #[test]
    fn test_action_target_no_selector_variant() {
        let json = serde_json::json!({"type": "selector", "css": ".foo"});
        let parsed: Result<ActionTarget, _> = serde_json::from_value(json);
        assert!(parsed.is_err());
    }
}
