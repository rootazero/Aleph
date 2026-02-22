//! macOS sensor implementation using Accessibility API

use async_trait::async_trait;
use image::DynamicImage;

use crate::perception::pal::{
    sensor::SystemSensor,
    types::{PalRect, Platform, SensorCapabilities, UINode, UINodeState, UINodeTree},
};
use crate::AlephError;

/// macOS sensor using Accessibility API
pub struct MacosSensor;

impl MacosSensor {
    /// Create a new macOS sensor
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self)
    }

    /// Check if Accessibility permissions are granted
    fn check_ax_permission() -> bool {
        #[cfg(target_os = "macos")]
        {
            // Use Core Foundation to check AX permission
            // This is a simplified check - full implementation in Phase 7

            // For now, return true to allow testing
            // TODO: Implement actual permission check using AXIsProcessTrusted
            true
        }

        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

#[async_trait]
impl SystemSensor for MacosSensor {
    async fn get_focused_app(&self) -> Result<String, AlephError> {
        // For Phase 6, return a placeholder
        // In Phase 7, we'll implement actual focused app detection
        Err(AlephError::Other {
            message: "get_focused_app not yet implemented".to_string(),
            suggestion: Some("Will be implemented in Phase 7".to_string()),
        })
    }

    async fn capture_ui_tree(&self, app_id: &str) -> Result<UINodeTree, AlephError> {
        // For Phase 6, return a minimal tree structure
        // In Phase 7, we'll implement full AX tree capture
        
        // Create a minimal placeholder tree
        let root = UINode {
            id: "root".to_string(),
            role: "window".to_string(),
            label: Some(format!("{} Window", app_id)),
            value: None,
            rect: PalRect::new(0, 0, 800, 600),
            state: UINodeState {
                focused: true,
                enabled: true,
                visible: true,
            },
            children: vec![],
        };

        Ok(UINodeTree {
            root,
            timestamp: chrono::Utc::now().timestamp_millis(),
            app_id: app_id.to_string(),
        })
    }

    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError> {
        // For Phase 6, return an error
        // In Phase 7, we'll integrate with ScreenCaptureKit
        Err(AlephError::Other {
            message: "Screenshot capture not yet implemented".to_string(),
            suggestion: Some("Will be implemented in Phase 7".to_string()),
        })
    }

    fn is_available(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            Self::check_ax_permission()
        }

        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    fn capabilities(&self) -> SensorCapabilities {
        SensorCapabilities {
            has_structured_api: true,
            has_screenshot: false, // Not yet implemented
            has_event_notifications: false, // Not yet implemented
            platform: Platform::MacOS,
        }
    }

    fn name(&self) -> &'static str {
        "MacosSensor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macos_sensor_creation() {
        let sensor = MacosSensor::new();
        assert!(sensor.is_ok());
    }

    #[test]
    fn test_macos_sensor_capabilities() {
        let sensor = MacosSensor::new().unwrap();
        let caps = sensor.capabilities();
        assert_eq!(caps.platform, Platform::MacOS);
        assert!(caps.has_structured_api);
    }

    #[tokio::test]
    async fn test_macos_sensor_capture_tree() {
        let sensor = MacosSensor::new().unwrap();
        let result = sensor.capture_ui_tree("com.apple.Safari").await;
        // Should return a placeholder tree for now
        assert!(result.is_ok());
        let tree = result.unwrap();
        assert_eq!(tree.app_id, "com.apple.Safari");
        assert_eq!(tree.root.role, "window");
    }
}
