//! macOS platform implementation for Aleph desktop capabilities.

use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::NativeScreen;
use aleph_desktop::DesktopPlatform;

/// macOS platform with shared `NativeScreen` for screen capabilities.
pub struct MacOSPlatform {
    screen: NativeScreen,
}

impl MacOSPlatform {
    /// Create a new `MacOSPlatform` instance.
    pub fn new() -> Self {
        Self {
            screen: NativeScreen::new(),
        }
    }
}

impl Default for MacOSPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for MacOSPlatform {
    fn platform_name(&self) -> &str {
        "macOS"
    }

    fn screen(&self) -> Option<&dyn ScreenCapability> {
        Some(&self.screen)
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        None
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        None
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let platform = MacOSPlatform::default();
        assert_eq!(platform.platform_name(), "macOS");
    }

    #[test]
    fn screen_is_some() {
        let platform = MacOSPlatform::new();
        assert!(platform.screen().is_some());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
