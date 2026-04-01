//! Types for the browser tool — action enum, arguments, and output structs.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::browser::ScrollDirection;

/// The action to perform on the browser.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    /// Launch a new browser instance (or connect to an existing one).
    Start,
    /// Stop the running browser instance.
    Stop,
    /// Open a new tab navigating to a URL.
    OpenTab,
    /// Close an existing tab by its tab_id.
    CloseTab,
    /// List all open tabs.
    ListTabs,
    /// Navigate an existing tab to a new URL.
    Navigate,
    /// Click an element identified by ref_id or selector.
    Click,
    /// Type (append) text into a focused or targeted element.
    Type,
    /// Fill (replace) the value of an input element.
    Fill,
    /// Scroll an element or the page in a given direction.
    Scroll,
    /// Hover over an element.
    Hover,
    /// Capture a screenshot of a tab.
    Screenshot,
    /// Take an ARIA accessibility snapshot of a tab (returns ref_ids for targeting).
    Snapshot,
    /// Evaluate arbitrary JavaScript in a tab.
    Evaluate,
}

/// Arguments for the browser tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserArgs {
    /// The browser action to perform.
    pub action: BrowserAction,

    /// Target tab ID (required for most per-tab actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,

    /// URL for open_tab / navigate actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// ARIA snapshot ref_id for targeting an element (preferred over selector).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<String>,

    /// CSS selector for targeting an element (fallback when ref_id is absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,

    /// Text to type or fill into an element.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// JavaScript code to evaluate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub js: Option<String>,

    /// Scroll direction: "up", "down", "left", "right".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<ScrollDirection>,

    /// Whether to launch the browser in headless mode (default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headless: Option<bool>,

    /// Whether to capture the full scrollable page for screenshots (default: false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_page: Option<bool>,
}

/// Output from browser operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserOutput {
    /// Whether the operation succeeded.
    pub success: bool,
    /// Human-readable message (present on errors or informational results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Structured data returned by the operation (tab info, snapshot, screenshot, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl BrowserOutput {
    pub(super) fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: None,
        }
    }

    pub(super) fn ok_data(data: Value) -> Self {
        Self {
            success: true,
            message: None,
            data: Some(data),
        }
    }

    pub(super) fn ok_data_msg(message: impl Into<String>, data: Value) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: Some(data),
        }
    }

    pub(super) fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            data: None,
        }
    }
}
