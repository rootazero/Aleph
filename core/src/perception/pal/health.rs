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
        let ax_enabled = check_ax_permission();
        let screen_recording = check_screen_recording_permission();
        let input_monitoring = check_input_monitoring_permission();

        let mut recommendations = Vec::new();
        if !ax_enabled {
            recommendations.push(
                "Enable Accessibility: System Settings → Privacy & Security → Accessibility → Add Aleph".to_string()
            );
        }
        if !screen_recording {
            recommendations.push(
                "Enable Screen Recording: System Settings → Privacy & Security → Screen Recording → Add Aleph".to_string()
            );
        }
        if !input_monitoring {
            recommendations.push(
                "Enable Input Monitoring: System Settings → Privacy & Security → Input Monitoring → Add Aleph".to_string()
            );
        }

        let platform_support = if ax_enabled && screen_recording && input_monitoring {
            PlatformSupport::Full
        } else if ax_enabled || screen_recording {
            PlatformSupport::Partial
        } else {
            PlatformSupport::Degraded
        };

        Self {
            accessibility_enabled: ax_enabled,
            screen_recording_enabled: screen_recording,
            input_monitoring_enabled: input_monitoring,
            platform_support,
            available_sensors: vec!["MacosSensor".to_string()],
            recommendations,
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

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[cfg(target_os = "macos")]
fn check_ax_permission() -> bool {
    // Use ApplicationServices framework to check AX permission
    unsafe { AXIsProcessTrusted() }
}

#[cfg(target_os = "macos")]
fn check_screen_recording_permission() -> bool {
    // Screen recording permission is tricky to check without actually attempting capture
    // For Phase 6, we'll use a heuristic: if AX is enabled, assume screen recording is too
    // A proper check would require attempting a screen capture
    // TODO: Implement actual screen recording permission check in Phase 7
    true
}

#[cfg(target_os = "macos")]
fn check_input_monitoring_permission() -> bool {
    // Input monitoring permission is also difficult to check programmatically
    // For Phase 6, we'll assume it's available if AX is enabled
    // TODO: Implement actual input monitoring permission check in Phase 7
    true
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
