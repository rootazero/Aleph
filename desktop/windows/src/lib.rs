//! Windows platform implementation for Aleph desktop capabilities.

mod sleep_inhibitor;

pub use sleep_inhibitor::WindowsPower;

use aleph_desktop::traits::{
    AutomationCapability, MediaCapability, PermissionCapability, PimCapability, PowerCapability,
    ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;
use aleph_desktop::NativeScreen;

/// Windows platform with shared `NativeScreen` for screen capabilities.
pub struct WindowsPlatform {
    screen: NativeScreen,
    power: WindowsPower,
}

impl WindowsPlatform {
    /// Create a new `WindowsPlatform` instance.
    pub fn new() -> Self {
        Self {
            screen: NativeScreen::new(),
            power: WindowsPower::new(),
        }
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for WindowsPlatform {
    fn platform_name(&self) -> &str {
        "Windows"
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

    fn power(&self) -> Option<&dyn PowerCapability> {
        Some(&self.power)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let platform = WindowsPlatform::default();
        assert_eq!(platform.platform_name(), "Windows");
    }

    #[test]
    fn screen_is_some() {
        let platform = WindowsPlatform::new();
        assert!(platform.screen().is_some());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
        assert!(platform.permission().is_none());
    }
}
