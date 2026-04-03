//! Linux platform implementation for Aleph desktop capabilities.

use aleph_desktop::traits::{
    AutomationCapability, MediaCapability, PermissionCapability, PimCapability, ScreenCapability,
    SystemCapability,
};
use aleph_desktop::DesktopPlatform;
use aleph_desktop::NativeScreen;

/// Linux platform with shared `NativeScreen` for screen capabilities.
pub struct LinuxPlatform {
    screen: NativeScreen,
}

impl LinuxPlatform {
    /// Create a new `LinuxPlatform` instance.
    pub fn new() -> Self {
        Self {
            screen: NativeScreen::new(),
        }
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for LinuxPlatform {
    fn platform_name(&self) -> &str {
        "Linux"
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

    fn permission(&self) -> Option<&dyn PermissionCapability> {
        None
    }

    fn media(&self) -> Option<&dyn MediaCapability> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let platform = LinuxPlatform::default();
        assert_eq!(platform.platform_name(), "Linux");
    }

    #[test]
    fn screen_is_some() {
        let platform = LinuxPlatform::new();
        assert!(platform.screen().is_some());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
