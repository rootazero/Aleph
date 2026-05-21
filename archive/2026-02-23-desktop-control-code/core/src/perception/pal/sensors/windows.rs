//! Windows sensor skeleton (not yet implemented)

use async_trait::async_trait;
use image::DynamicImage;

use crate::perception::pal::{
    sensor::SystemSensor,
    types::{Platform, SensorCapabilities, UINodeTree},
};
use crate::AlephError;

/// Windows sensor (not yet implemented)
pub struct WindowsSensor;

impl WindowsSensor {
    pub fn new() -> Result<Self, AlephError> {
        Ok(Self)
    }
}

#[async_trait]
impl SystemSensor for WindowsSensor {
    async fn get_focused_app(&self) -> Result<String, AlephError> {
        Err(AlephError::Other {
            message: "Windows sensor not yet implemented".to_string(),
            suggestion: Some("Coming in Phase 7".to_string()),
        })
    }

    async fn capture_ui_tree(&self, _app_id: &str) -> Result<UINodeTree, AlephError> {
        Err(AlephError::Other {
            message: "Windows sensor not yet implemented".to_string(),
            suggestion: Some("Coming in Phase 7".to_string()),
        })
    }

    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError> {
        Err(AlephError::Other {
            message: "Windows sensor not yet implemented".to_string(),
            suggestion: Some("Coming in Phase 7".to_string()),
        })
    }

    fn is_available(&self) -> bool {
        false
    }

    fn capabilities(&self) -> SensorCapabilities {
        SensorCapabilities {
            has_structured_api: false,
            has_screenshot: false,
            has_event_notifications: false,
            platform: Platform::Windows,
        }
    }

    fn name(&self) -> &'static str {
        "WindowsSensor (not implemented)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_sensor_creation() {
        let sensor = WindowsSensor::new();
        assert!(sensor.is_ok());
    }

    #[test]
    fn test_windows_sensor_not_available() {
        let sensor = WindowsSensor::new().unwrap();
        assert!(!sensor.is_available());
    }
}
