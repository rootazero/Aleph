//! Canvas Tool Types
//!
//! Core types for the Canvas visual rendering system.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ============================================================================
// Canvas Actions
// ============================================================================

/// Canvas action types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CanvasAction {
    /// Show the canvas window with optional placement
    Present,
    /// Hide the canvas window
    Hide,
    /// Navigate to a URL
    Navigate,
    /// Execute JavaScript in the canvas
    Eval,
    /// Capture a screenshot of the canvas
    Snapshot,
    /// Push A2UI JSONL updates
    A2uiPush,
    /// Reset A2UI state
    A2uiReset,
}

impl CanvasAction {
    /// Get the action name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Hide => "hide",
            Self::Navigate => "navigate",
            Self::Eval => "eval",
            Self::Snapshot => "snapshot",
            Self::A2uiPush => "a2ui_push",
            Self::A2uiReset => "a2ui_reset",
        }
    }
}

impl std::fmt::Display for CanvasAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// Snapshot Format
// ============================================================================

/// Screenshot format options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotFormat {
    /// PNG format (lossless)
    #[default]
    Png,
    /// JPEG format (lossy)
    Jpg,
    /// JPEG format alias
    Jpeg,
}

impl SnapshotFormat {
    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpg | Self::Jpeg => "image/jpeg",
        }
    }

    /// Get the file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg | Self::Jpeg => "jpg",
        }
    }
}

// ============================================================================
// Window Placement
// ============================================================================

/// Canvas window placement configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WindowPlacement {
    /// X coordinate (pixels from left)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    /// Y coordinate (pixels from top)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// Window width in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Window height in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl Default for WindowPlacement {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: Some(800),
            height: Some(600),
        }
    }
}

// ============================================================================
// Tool Arguments
// ============================================================================

/// Arguments for the canvas tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CanvasToolArgs {
    /// The action to perform
    pub action: CanvasAction,

    /// URL to navigate to (for navigate action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// JavaScript code to execute (for eval action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub javascript: Option<String>,

    /// A2UI JSONL content (for a2ui_push action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonl: Option<String>,

    /// Window placement (for present action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<WindowPlacement>,

    /// Snapshot format (for snapshot action)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<SnapshotFormat>,

    /// Maximum width for snapshot scaling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,

    /// JPEG quality (0-100) for snapshot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,

    /// Surface ID for A2UI operations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
}

impl CanvasToolArgs {
    /// Create args for present action
    pub fn present() -> Self {
        Self {
            action: CanvasAction::Present,
            url: None,
            javascript: None,
            jsonl: None,
            placement: None,
            format: None,
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }

    /// Create args for present action with URL
    pub fn present_url(url: impl Into<String>) -> Self {
        Self {
            action: CanvasAction::Present,
            url: Some(url.into()),
            javascript: None,
            jsonl: None,
            placement: None,
            format: None,
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }

    /// Create args for hide action
    pub fn hide() -> Self {
        Self {
            action: CanvasAction::Hide,
            url: None,
            javascript: None,
            jsonl: None,
            placement: None,
            format: None,
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }

    /// Create args for navigate action
    pub fn navigate(url: impl Into<String>) -> Self {
        Self {
            action: CanvasAction::Navigate,
            url: Some(url.into()),
            javascript: None,
            jsonl: None,
            placement: None,
            format: None,
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }

    /// Create args for eval action
    pub fn eval(javascript: impl Into<String>) -> Self {
        Self {
            action: CanvasAction::Eval,
            url: None,
            javascript: Some(javascript.into()),
            jsonl: None,
            placement: None,
            format: None,
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }

    /// Create args for snapshot action
    pub fn snapshot() -> Self {
        Self {
            action: CanvasAction::Snapshot,
            url: None,
            javascript: None,
            jsonl: None,
            placement: None,
            format: Some(SnapshotFormat::Png),
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }

    /// Create args for a2ui_push action
    pub fn a2ui_push(jsonl: impl Into<String>) -> Self {
        Self {
            action: CanvasAction::A2uiPush,
            url: None,
            javascript: None,
            jsonl: Some(jsonl.into()),
            placement: None,
            format: None,
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }

    /// Create args for a2ui_reset action
    pub fn a2ui_reset() -> Self {
        Self {
            action: CanvasAction::A2uiReset,
            url: None,
            javascript: None,
            jsonl: None,
            placement: None,
            format: None,
            max_width: None,
            quality: None,
            surface_id: None,
        }
    }
}

// ============================================================================
// Tool Output
// ============================================================================

/// Output from the canvas tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CanvasToolOutput {
    /// The action that was performed
    pub action: CanvasAction,
    /// Whether the operation succeeded
    pub success: bool,
    /// Error message if operation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Current URL after operation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// JavaScript evaluation result
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_result: Option<String>,
    /// Snapshot data (base64 encoded)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_data: Option<String>,
    /// Snapshot MIME type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_mime: Option<String>,
}

impl CanvasToolOutput {
    /// Create a successful output
    pub fn success(action: CanvasAction) -> Self {
        Self {
            action,
            success: true,
            error: None,
            url: None,
            eval_result: None,
            snapshot_data: None,
            snapshot_mime: None,
        }
    }

    /// Create a successful output with URL
    pub fn success_with_url(action: CanvasAction, url: impl Into<String>) -> Self {
        Self {
            action,
            success: true,
            error: None,
            url: Some(url.into()),
            eval_result: None,
            snapshot_data: None,
            snapshot_mime: None,
        }
    }

    /// Create a successful output with eval result
    pub fn success_with_eval(action: CanvasAction, result: impl Into<String>) -> Self {
        Self {
            action,
            success: true,
            error: None,
            url: None,
            eval_result: Some(result.into()),
            snapshot_data: None,
            snapshot_mime: None,
        }
    }

    /// Create a successful output with snapshot
    pub fn success_with_snapshot(
        action: CanvasAction,
        data: impl Into<String>,
        mime: impl Into<String>,
    ) -> Self {
        Self {
            action,
            success: true,
            error: None,
            url: None,
            eval_result: None,
            snapshot_data: Some(data.into()),
            snapshot_mime: Some(mime.into()),
        }
    }

    /// Create a failed output
    pub fn failed(action: CanvasAction, error: impl Into<String>) -> Self {
        Self {
            action,
            success: false,
            error: Some(error.into()),
            url: None,
            eval_result: None,
            snapshot_data: None,
            snapshot_mime: None,
        }
    }
}

// ============================================================================
// Canvas State
// ============================================================================

/// Current state of the canvas
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CanvasState {
    /// Whether the canvas is currently visible
    pub visible: bool,
    /// Current URL loaded in the canvas
    pub current_url: Option<String>,
    /// Window placement
    pub placement: WindowPlacement,
    /// Active A2UI surface IDs
    pub surfaces: Vec<String>,
}

impl CanvasState {
    /// Create a new canvas state
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the canvas is visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canvas_action_serialization() {
        assert_eq!(
            serde_json::to_string(&CanvasAction::Present).unwrap(),
            r#""present""#
        );
        assert_eq!(
            serde_json::to_string(&CanvasAction::A2uiPush).unwrap(),
            r#""a2ui_push""#
        );
    }

    #[test]
    fn test_canvas_action_deserialization() {
        let action: CanvasAction = serde_json::from_str(r#""navigate""#).unwrap();
        assert_eq!(action, CanvasAction::Navigate);
    }

    #[test]
    fn test_snapshot_format() {
        assert_eq!(SnapshotFormat::Png.mime_type(), "image/png");
        assert_eq!(SnapshotFormat::Jpg.extension(), "jpg");
    }

    #[test]
    fn test_canvas_args_builders() {
        let args = CanvasToolArgs::present_url("http://localhost:8080");
        assert_eq!(args.action, CanvasAction::Present);
        assert_eq!(args.url, Some("http://localhost:8080".to_string()));

        let args = CanvasToolArgs::eval("console.log('test')");
        assert_eq!(args.action, CanvasAction::Eval);
        assert!(args.javascript.is_some());
    }

    #[test]
    fn test_canvas_output_success() {
        let output = CanvasToolOutput::success(CanvasAction::Present);
        assert!(output.success);
        assert!(output.error.is_none());
    }

    #[test]
    fn test_canvas_output_failed() {
        let output = CanvasToolOutput::failed(CanvasAction::Navigate, "Connection failed");
        assert!(!output.success);
        assert_eq!(output.error, Some("Connection failed".to_string()));
    }

    #[test]
    fn test_canvas_output_with_snapshot() {
        let output = CanvasToolOutput::success_with_snapshot(
            CanvasAction::Snapshot,
            "base64data",
            "image/png",
        );
        assert!(output.success);
        assert_eq!(output.snapshot_data, Some("base64data".to_string()));
        assert_eq!(output.snapshot_mime, Some("image/png".to_string()));
    }

    #[test]
    fn test_window_placement_default() {
        let placement = WindowPlacement::default();
        assert_eq!(placement.width, Some(800));
        assert_eq!(placement.height, Some(600));
        assert!(placement.x.is_none());
    }

    #[test]
    fn test_canvas_state() {
        let mut state = CanvasState::new();
        assert!(!state.is_visible());

        state.visible = true;
        state.current_url = Some("http://example.com".to_string());
        assert!(state.is_visible());
    }
}
