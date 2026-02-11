//! Perception system health check

use serde::{Deserialize, Serialize};

/// Perception system health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionHealth {
    /// Accessibility API is enabled and accessible
    pub accessibility_enabled: bool,
    /// Screen recording permission granted
    pub screen_recording_enabled: bool,
    /// Input monitoring permission granted
    pub input_monitoring_enabled: bool,
    /// Overall platform support level
    pub platform_support: PlatformSupport,
    /// List of available sensor names
    pub available_sensors: Vec<String>,
    /// User-friendly recommendations for fixing issues
    pub recommendations: Vec<String>,
}

/// Platform support level
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PlatformSupport {
    /// All features available
    Full,
    /// Some features missing (e.g., missing permissions)
    Partial,
    /// Only basic features available (e.g., Wayland limitations)
    Degraded,
    /// No GUI support (headless environment)
    None,
}

impl PerceptionHealth {
    /// Check perception system health on current platform
    pub async fn check() -> Self {
        // Platform-specific implementation
        #[cfg(target_os = "macos")]
        return Self::check_macos().await;

        #[cfg(target_os = "windows")]
        return Self::check_windows().await;

        #[cfg(target_os = "linux")]
        return Self::check_linux().await;

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        return Self::unsupported();
    }

    #[cfg(target_os = "macos")]
    async fn check_macos() -> Self {
        // TODO: Implement actual permission checks
        // For now, return placeholder values
        Self {
            accessibility_enabled: false,
            screen_recording_enabled: false,
            input_monitoring_enabled: false,
            platform_support: PlatformSupport::Partial,
            available_sensors: vec!["MacosSensor".to_string()],
            recommendations: vec![
                "Enable Accessibility: System Settings → Privacy & Security → Accessibility → Add Aleph".to_string(),
                "Enable Screen Recording: System Settings → Privacy & Security → Screen Recording → Add Aleph".to_string(),
            ],
        }
    }

    #[cfg(target_os = "windows")]
    async fn check_windows() -> Self {
        Self {
            accessibility_enabled: false,
            screen_recording_enabled: false,
            input_monitoring_enabled: false,
            platform_support: PlatformSupport::None,
            available_sensors: vec!["WindowsSensor (not implemented)".to_string()],
            recommendations: vec![
                "Windows sensor not yet implemented. Coming in Phase 7.".to_string(),
            ],
        }
    }

    #[cfg(target_os = "linux")]
    async fn check_linux() -> Self {
        let has_display =
            std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();

        if !has_display {
            return Self {
                accessibility_enabled: false,
                screen_recording_enabled: false,
                input_monitoring_enabled: false,
                platform_support: PlatformSupport::None,
                available_sensors: vec![],
                recommendations: vec![
                    "No display server detected. Aleph requires X11 or Wayland.".to_string(),
                    "Run Aleph on a desktop environment with GUI.".to_string(),
                ],
            };
        }

        Self {
            accessibility_enabled: false,
            screen_recording_enabled: false,
            input_monitoring_enabled: false,
            platform_support: PlatformSupport::None,
            available_sensors: vec!["LinuxSensor (not implemented)".to_string()],
            recommendations: vec![
                "Linux sensor not yet implemented. Coming in Phase 7.".to_string(),
                "Install at-spi2-core package for accessibility support.".to_string(),
            ],
        }
    }

    fn unsupported() -> Self {
        Self {
            accessibility_enabled: false,
            screen_recording_enabled: false,
            input_monitoring_enabled: false,
            platform_support: PlatformSupport::None,
            available_sensors: vec![],
            recommendations: vec!["Platform not supported for UI perception".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_check() {
        let health = PerceptionHealth::check().await;
        // Should return valid data on all platforms
        assert!(matches!(
            health.platform_support,
            PlatformSupport::Full
                | PlatformSupport::Partial
                | PlatformSupport::Degraded
                | PlatformSupport::None
        ));
    }

    #[test]
    fn test_platform_support_serialization() {
        let support = PlatformSupport::Full;
        let json = serde_json::to_string(&support).unwrap();
        let deserialized: PlatformSupport = serde_json::from_str(&json).unwrap();
        assert_eq!(support, deserialized);
    }
}
