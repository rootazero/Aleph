//! Linux sensor skeleton (not yet implemented)

use async_trait::async_trait;
use image::DynamicImage;

use crate::perception::pal::{
    sensor::SystemSensor,
    types::{Platform, SensorCapabilities, UINodeTree},
};
use crate::AlephError;

/// Linux sensor (not yet implemented)
pub struct LinuxSensor;

impl LinuxSensor {
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self)
    }

    /// Check if display server is available
    fn has_display() -> bool {
        std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok()
    }
}

#[async_trait]
impl SystemSensor for LinuxSensor {
    async fn get_focused_app(&self) -> Result<String, AlephError> {
        if !Self::has_display() {
            return Err(AlephError::Other {
                message: "No display server detected".to_string(),
                suggestion: Some("Aleph requires X11 or Wayland".to_string()),
            });
        }

        Err(AlephError::Other {
            message: "Linux sensor not yet implemented".to_string(),
            suggestion: Some("Coming in Phase 7".to_string()),
        })
    }

    async fn capture_ui_tree(&self, _app_id: &str) -> Result<UINodeTree, AlephError> {
        if !Self::has_display() {
            return Err(AlephError::Other {
                message: "No display server detected".to_string(),
                suggestion: Some("Aleph requires X11 or Wayland".to_string()),
            });
        }

        Err(AlephError::Other {
            message: "Linux sensor not yet implemented".to_string(),
            suggestion: Some("Coming in Phase 7".to_string()),
        })
    }

    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError> {
        if !Self::has_display() {
            return Err(AlephError::Other {
                message: "No display server detected".to_string(),
                suggestion: Some("Aleph requires X11 or Wayland".to_string()),
            });
        }

        Err(AlephError::Other {
            message: "Linux sensor not yet implemented".to_string(),
            suggestion: Some("Coming in Phase 7".to_string()),
        })
    }

    fn is_available(&self) -> bool {
        Self::has_display()
    }

    fn capabilities(&self) -> SensorCapabilities {
        SensorCapabilities {
            has_structured_api: false,
            has_screenshot: false,
            has_event_notifications: false,
            platform: Platform::Linux,
        }
    }

    fn name(&self) -> &'static str {
        "LinuxSensor (not implemented)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_sensor_creation() {
        let sensor = LinuxSensor::new();
        assert!(sensor.is_ok());
    }

    #[test]
    fn test_linux_sensor_display_detection() {
        let sensor = LinuxSensor::new().unwrap();
        // Availability depends on environment
        let _ = sensor.is_available();
    }
}
