mod automation;
mod escape_listener;
mod media;
mod permission;
mod pim;
mod sleep_inhibitor;
mod system;

pub use automation::LinuxAutomation;
pub use escape_listener::LinuxEscapeListener;
pub use media::LinuxMedia;
pub use permission::LinuxPermission;
pub use pim::LinuxPim;
pub use sleep_inhibitor::LinuxPower;
pub use system::LinuxSystem;

use aleph_desktop::traits::{
    AutomationCapability, MediaCapability, PermissionCapability, PimCapability, PowerCapability,
    ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;
use aleph_desktop::NativeScreen;

pub struct LinuxPlatform {
    screen: NativeScreen,
    power: LinuxPower,
    system: LinuxSystem,
    automation: LinuxAutomation,
    escape: LinuxEscapeListener,
    pim: LinuxPim,
    media: LinuxMedia,
    permission: LinuxPermission,
}

impl LinuxPlatform {
    #[must_use]
    pub fn new() -> Self {
        Self {
            screen: NativeScreen::new(),
            power: LinuxPower::new(),
            system: LinuxSystem::new(),
            automation: LinuxAutomation::new(),
            escape: LinuxEscapeListener::new(),
            pim: LinuxPim::new(),
            media: LinuxMedia::new(),
            permission: LinuxPermission::new(),
        }
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for LinuxPlatform {
    fn platform_name(&self) -> &'static str {
        "Linux"
    }

    fn screen(&self) -> Option<&dyn ScreenCapability> {
        Some(&self.screen)
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        Some(&self.pim)
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        Some(&self.system)
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        Some(&self.automation)
    }

    fn permission(&self) -> Option<&dyn PermissionCapability> {
        Some(&self.permission)
    }

    fn media(&self) -> Option<&dyn MediaCapability> {
        Some(&self.media)
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
        let platform = LinuxPlatform::default();
        assert_eq!(platform.platform_name(), "Linux");
    }

    #[test]
    fn capabilities_wired() {
        let platform = LinuxPlatform::new();
        assert!(platform.screen().is_some());
        assert!(platform.system().is_some());
        assert!(platform.automation().is_some());
        assert!(platform.power().is_some());
        assert!(platform.escape_listener().is_some());
        assert!(platform.pim().is_some());
        assert!(platform.media().is_some());
        assert!(platform.permission().is_some());
    }
}
