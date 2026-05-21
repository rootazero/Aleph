//! Platform-specific sensor implementations

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "linux")]
pub mod linux;

use crate::perception::pal::SystemSensor;
use crate::AlephError;
use std::sync::Arc;

#[cfg(target_os = "macos")]
pub use macos::MacosSensor;
#[cfg(target_os = "windows")]
pub use windows::WindowsSensor;
#[cfg(target_os = "linux")]
pub use linux::LinuxSensor;

/// Create a platform-specific sensor
///
/// This factory function automatically selects the appropriate sensor
/// implementation based on the current platform.
///
/// # Returns
/// * `Ok(Arc<dyn SystemSensor>)` - Platform sensor
/// * `Err(AlephError)` - Platform not supported
///
/// # Example
/// ```ignore
/// let sensor = create_platform_sensor()?;
/// if sensor.is_available() {
///     let tree = sensor.capture_ui_tree("com.apple.Safari").await?;
/// }
/// ```
pub fn create_platform_sensor() -> Result<Arc<dyn SystemSensor>, AlephError> {
    #[cfg(target_os = "macos")]
    {
        let sensor = MacosSensor::new()?;
        Ok(Arc::new(sensor))
    }

    #[cfg(target_os = "windows")]
    {
        let sensor = WindowsSensor::new()?;
        Ok(Arc::new(sensor))
    }

    #[cfg(target_os = "linux")]
    {
        let sensor = LinuxSensor::new()?;
        Ok(Arc::new(sensor))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err(AlephError::Other {
            message: "Platform not supported for UI perception".to_string(),
            suggestion: Some("Supported platforms: macOS, Windows, Linux".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_platform_sensor() {
        let result = create_platform_sensor();
        // Should succeed on supported platforms
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        assert!(result.is_ok());

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        assert!(result.is_err());
    }

    #[test]
    fn test_sensor_name() {
        if let Ok(sensor) = create_platform_sensor() {
            let name = sensor.name();
            assert!(!name.is_empty());
        }
    }
}
