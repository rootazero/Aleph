//! Cross-platform system sensor trait for UI perception

use async_trait::async_trait;
use image::DynamicImage;

use crate::AlephError;

use super::types::{SensorCapabilities, UINodeTree};

/// Cross-platform trait for sensing UI state
///
/// This trait provides a unified interface for accessing UI information
/// across different platforms (macOS, Windows, Linux). Implementations
/// use platform-specific APIs (Accessibility API, UI Automation, AT-SPI)
/// to extract structured UI trees and capture screenshots.
#[async_trait]
pub trait SystemSensor: Send + Sync {
    /// Get currently focused application ID
    ///
    /// Returns the bundle ID on macOS (e.g., "com.apple.Safari"),
    /// process name on other platforms (e.g., "firefox").
    async fn get_focused_app(&self) -> Result<String, AlephError>;

    /// Capture UI tree for a specific application
    ///
    /// Uses platform-specific Accessibility APIs to extract the complete
    /// UI hierarchy for the given application. Returns a structured tree
    /// with element roles, labels, values, and screen coordinates.
    ///
    /// # Arguments
    /// * `app_id` - Application identifier (bundle ID or process name)
    ///
    /// # Errors
    /// * `AlephError::not_supported` - Platform doesn't support structured API
    /// * `AlephError::permission` - Missing accessibility permissions
    /// * `AlephError::not_found` - Application not found or not running
    async fn capture_ui_tree(&self, app_id: &str) -> Result<UINodeTree, AlephError>;

    /// Capture screenshot of current screen
    ///
    /// Returns a full-screen screenshot as a DynamicImage. This is used
    /// as a fallback when structured APIs are unavailable or insufficient.
    ///
    /// # Errors
    /// * `AlephError::not_supported` - Platform doesn't support screenshots
    /// * `AlephError::permission` - Missing screen recording permissions
    async fn capture_screenshot(&self) -> Result<DynamicImage, AlephError>;

    /// Check if sensor is available in current environment
    ///
    /// Returns false if:
    /// - Running in headless environment (no display server)
    /// - Missing required permissions
    /// - Platform not supported
    fn is_available(&self) -> bool;

    /// Get sensor capabilities
    ///
    /// Returns information about what this sensor can do on the current
    /// platform (structured API, screenshots, event notifications).
    fn capabilities(&self) -> SensorCapabilities;

    /// Get sensor name for logging
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sensor_trait_object_safety() {
        // Verify trait is object-safe (can be used as Box<dyn SystemSensor>)
        let _: Option<Box<dyn SystemSensor>> = None;
    }
}
