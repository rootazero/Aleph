use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unique identifier for a browser tab.
pub type TabId = String;

/// Information about an open browser tab.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TabInfo {
    pub id: TabId,
    pub url: String,
    pub title: String,
}

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

/// Snapshot of the page's accessibility (ARIA) tree.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AriaSnapshot {
    /// Flat list of accessible elements.
    pub elements: Vec<AriaElement>,

    /// Title of the page.
    pub page_title: Option<String>,

    /// URL of the page.
    pub page_url: Option<String>,

    /// Reference ID of the currently focused element, if any.
    pub focused_ref: Option<String>,
}

/// A single element in the ARIA snapshot tree.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AriaElement {
    /// Unique reference ID for targeting this element.
    pub ref_id: String,

    /// ARIA role (e.g. "button", "textbox", "link").
    pub role: String,

    /// Accessible name.
    pub name: Option<String>,

    /// Current value (for inputs, sliders, etc.).
    pub value: Option<String>,

    /// ARIA states (e.g. "expanded", "checked", "disabled").
    #[serde(default)]
    pub state: Vec<String>,

    /// Bounding rectangle in viewport coordinates.
    pub bounds: Option<ElementRect>,

    /// Child elements.
    #[serde(default)]
    pub children: Vec<AriaElement>,
}

/// Bounding rectangle of an element (viewport pixels).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ElementRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Target for a browser action (click, hover, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionTarget {
    /// Target an element by its ARIA snapshot ref ID.
    Ref {
        ref_id: String,
    },

    /// Target an element by a CSS selector.
    Selector {
        css: String,
    },

    /// Target a specific viewport coordinate.
    Coordinates {
        x: f64,
        y: f64,
    },
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

/// Result of a screenshot capture.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotResult {
    /// Base64-encoded image data.
    pub data_base64: String,

    /// Width of the captured image in pixels.
    pub width: u32,

    /// Height of the captured image in pixels.
    pub height: u32,

    /// Image format (e.g. "png", "jpeg").
    pub format: String,
}

/// Kind of web storage.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Local,
    Session,
}
