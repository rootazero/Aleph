//! Type definitions for System State Bus.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// AX event from the observer.
#[derive(Debug, Clone)]
pub enum AxEvent {
    /// Element value changed
    ValueChanged {
        app_id: String,
        element_id: String,
        new_value: Value,
    },

    /// Focus changed between elements
    FocusChanged {
        app_id: String,
        from: String,
        to: String,
    },

    /// Window created
    WindowCreated {
        app_id: String,
        window_id: String,
    },

    /// Window closed
    WindowClosed {
        app_id: String,
        window_id: String,
    },
}

/// Application state snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    /// Application bundle ID
    pub app_id: String,

    /// UI elements
    pub elements: Vec<Element>,

    /// Application-specific context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_context: Option<Value>,

    /// State source
    pub source: StateSource,

    /// Confidence score (0.0-1.0)
    pub confidence: f32,
}

/// UI element representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    /// Stable element ID
    pub id: String,

    /// AX role (button, textfield, etc.)
    pub role: String,

    /// Element label/title
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Current value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,

    /// Bounding rectangle (screen coordinates)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect: Option<Rect>,

    /// Element state (focused, enabled, etc.)
    #[serde(default)]
    pub state: ElementState,

    /// Data source
    pub source: ElementSource,

    /// Confidence score (for vision-based detection)
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    1.0
}

/// Element state flags.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ElementState {
    #[serde(default)]
    pub focused: bool,

    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub selected: bool,
}

fn default_true() -> bool {
    true
}

/// Rectangle in screen coordinates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// State source type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateSource {
    /// macOS Accessibility API
    Accessibility,
    /// Browser/IDE plugin
    Plugin,
    /// Vision/OCR
    Vision,
}

/// Element source type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElementSource {
    /// From AX API
    Ax,
    /// From OCR
    Ocr,
    /// From vision detection
    Vision,
}

/// Subscription request parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct SubscribeParams {
    /// Topic patterns (glob-style)
    pub patterns: Vec<String>,

    /// Include initial snapshot
    #[serde(default)]
    pub include_snapshot: bool,

    /// Debounce interval (ms)
    #[serde(default)]
    pub debounce_ms: Option<u64>,
}

/// Subscription response.
#[derive(Debug, Clone, Serialize)]
pub struct SubscribeResult {
    /// Subscription ID
    pub subscription_id: String,

    /// Active patterns
    pub active_patterns: Vec<String>,

    /// Initial snapshot (if requested)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_snapshot: Option<Value>,
}

/// Unsubscribe request parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct UnsubscribeParams {
    /// Subscription ID
    pub subscription_id: String,
}
