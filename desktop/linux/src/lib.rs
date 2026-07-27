// AT-SPI2 accessibility. Linux-only: the `atspi` dependency is target-gated, so
// the module cannot compile elsewhere — and `ax()` reports `None` there, which
// is what every non-Linux build of this crate already did.
mod automation;
#[cfg(target_os = "linux")]
mod ax;
mod escape_listener;
mod media;
mod permission;
mod pim;
mod sleep_inhibitor;
mod system;

pub use automation::LinuxAutomation;
#[cfg(target_os = "linux")]
pub use ax::LinuxAccessibility;
pub use escape_listener::LinuxEscapeListener;
pub use media::LinuxMedia;
pub use permission::LinuxPermission;
pub use pim::LinuxPim;
pub use sleep_inhibitor::LinuxPower;
pub use system::LinuxSystem;

use aleph_desktop::traits::{
    AccessibilityCapability, AutomationCapability, MediaCapability, PermissionCapability,
    PimCapability, PowerCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;
use aleph_desktop::NativeScreen;

pub struct LinuxPlatform {
    screen: NativeScreen,
    /// `None` when no accessibility bus is reachable — see
    /// [`ax::LinuxAccessibility::probe`] for why an absent capability beats one
    /// that fails on every call.
    #[cfg(target_os = "linux")]
    ax: Option<LinuxAccessibility>,
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
            #[cfg(target_os = "linux")]
            ax: LinuxAccessibility::probe(),
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

    /// AT-SPI2 accessibility, when this session has an accessibility bus.
    ///
    /// Returning `None` on a desktop with accessibility switched off is
    /// deliberate: `type_text`'s focus gate treats an AX error as fail-open but
    /// logs a warning, so a capability that always fails is noisier and no more
    /// useful than an honest absence.
    #[cfg(target_os = "linux")]
    fn ax(&self) -> Option<&dyn AccessibilityCapability> {
        self.ax
            .as_ref()
            .map(|ax| ax as &dyn AccessibilityCapability)
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
