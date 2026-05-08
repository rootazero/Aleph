//! Windows platform implementation for Aleph desktop capabilities.

mod automation;
mod escape_listener;
mod sleep_inhibitor;
mod system;

pub use automation::WindowsAutomation;
pub use escape_listener::WindowsEscapeListener;
pub use sleep_inhibitor::WindowsPower;
pub use system::WindowsSystem;

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
    system: WindowsSystem,
    automation: WindowsAutomation,
    escape: WindowsEscapeListener,
}

impl WindowsPlatform {
    /// Create a new `WindowsPlatform` instance.
    pub fn new() -> Self {
        Self {
            screen: NativeScreen::new(),
            power: WindowsPower::new(),
            system: WindowsSystem::new(),
            automation: WindowsAutomation::new(),
            escape: WindowsEscapeListener::new(),
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
        Some(&self.system)
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        Some(&self.automation)
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

    fn escape_listener(&self) -> Option<&dyn aleph_desktop::platform::EscapeAbort> {
        Some(&self.escape)
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
        assert!(platform.system().is_some());
        assert!(platform.automation().is_some());
        assert!(platform.power().is_some());
        assert!(platform.escape_listener().is_some());
        assert!(platform.pim().is_none());
        assert!(platform.permission().is_none());
        assert!(platform.media().is_none());
    }
}
