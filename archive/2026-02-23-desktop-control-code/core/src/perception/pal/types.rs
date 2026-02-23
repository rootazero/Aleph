//! Cross-platform types for Platform Abstraction Layer (PAL)

use serde::{Deserialize, Serialize};

/// Complete UI tree snapshot for an application
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UINodeTree {
    /// Root node of the tree
    pub root: UINode,
    /// Timestamp when tree was captured (Unix timestamp in milliseconds)
    pub timestamp: i64,
    /// Application identifier (bundle ID on macOS, process name on others)
    pub app_id: String,
}

/// Single node in the UI tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UINode {
    /// Stable element ID (see ID Stability Strategy in design doc)
    pub id: String,
    /// Element role (button, textfield, window, etc.)
    pub role: String,
    /// Human-readable label (button text, window title, etc.)
    pub label: Option<String>,
    /// Current value (text field content, slider value, etc.)
    pub value: Option<String>,
    /// Screen coordinates and size
    pub rect: PalRect,
    /// Element state flags
    pub state: UINodeState,
    /// Child elements
    pub children: Vec<UINode>,
}

/// Element state flags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UINodeState {
    /// Has keyboard focus
    pub focused: bool,
    /// Can be interacted with
    pub enabled: bool,
    /// Currently visible on screen
    pub visible: bool,
}

/// Rectangle in screen coordinates (PAL-specific, uses i32 for pixel coordinates)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PalRect {
    /// X coordinate (pixels from left edge)
    pub x: i32,
    /// Y coordinate (pixels from top edge)
    pub y: i32,
    /// Width in pixels
    pub width: i32,
    /// Height in pixels
    pub height: i32,
}

impl PalRect {
    /// Create a new rectangle
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Check if point is inside rectangle
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Get center point
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }
}

/// Platform identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
    Unknown,
}

impl Platform {
    /// Detect current platform
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Platform::MacOS;

        #[cfg(target_os = "windows")]
        return Platform::Windows;

        #[cfg(target_os = "linux")]
        return Platform::Linux;

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        return Platform::Unknown;
    }
}

/// Sensor capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorCapabilities {
    /// Can access structured UI tree (Accessibility API)
    pub has_structured_api: bool,
    /// Can capture screenshots
    pub has_screenshot: bool,
    /// Can receive real-time UI change notifications
    pub has_event_notifications: bool,
    /// Platform this sensor runs on
    pub platform: Platform,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_contains() {
        let rect = PalRect::new(10, 20, 100, 50);
        assert!(rect.contains(50, 40));
        assert!(!rect.contains(5, 40));
        assert!(!rect.contains(50, 15));
    }

    #[test]
    fn test_rect_center() {
        let rect = PalRect::new(10, 20, 100, 50);
        assert_eq!(rect.center(), (60, 45));
    }

    #[test]
    fn test_platform_detection() {
        let platform = Platform::current();
        assert!(matches!(
            platform,
            Platform::MacOS | Platform::Windows | Platform::Linux | Platform::Unknown
        ));
    }
}
