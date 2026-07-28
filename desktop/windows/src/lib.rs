//! Windows platform implementation for Aleph desktop capabilities.

mod automation;
mod ax;
mod escape_listener;
mod media;
mod permission;
mod pim;
mod sleep_inhibitor;
mod system;

pub use automation::WindowsAutomation;
pub use ax::WindowsAccessibility;
pub use escape_listener::WindowsEscapeListener;
pub use media::WindowsMedia;
pub use permission::WindowsPermission;
pub use pim::WindowsPim;
pub use sleep_inhibitor::WindowsPower;
pub use system::WindowsSystem;

use aleph_desktop::traits::{
    AccessibilityCapability, AutomationCapability, MediaCapability, PermissionCapability,
    PimCapability, PowerCapability, ScreenCapability, SystemCapability,
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
    pim: WindowsPim,
    permission: WindowsPermission,
    media: WindowsMedia,
    ax: WindowsAccessibility,
}

impl WindowsPlatform {
    /// Create a new `WindowsPlatform` instance.
    ///
    /// This is the single per-OS construction point (`builtin_registry`'s
    /// constructor picks it by target), which makes it the one place that can
    /// settle the process's DPI awareness before any window, DC or UI Automation
    /// object exists. Without that opt-in, Win32 hands this process *virtualized*
    /// window and accessibility geometry while screen captures stay at full
    /// resolution — a 1.5× mismatch on the 150 %-scaled panel that is the Windows
    /// default, and one nothing downstream can repair. See
    /// [`aleph_desktop::win_dpi`].
    #[must_use]
    pub fn new() -> Self {
        aleph_desktop::ensure_process_dpi_aware();

        Self {
            screen: NativeScreen::new(),
            power: WindowsPower::new(),
            system: WindowsSystem::new(),
            automation: WindowsAutomation::new(),
            escape: WindowsEscapeListener::new(),
            pim: WindowsPim::new(),
            permission: WindowsPermission::new(),
            media: WindowsMedia::new(),
            ax: WindowsAccessibility::new(),
        }
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for WindowsPlatform {
    fn platform_name(&self) -> &'static str {
        "Windows"
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

    fn ax(&self) -> Option<&dyn AccessibilityCapability> {
        Some(&self.ax)
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
        assert!(platform.pim().is_some());
        assert!(platform.permission().is_some());
        assert!(platform.media().is_some());
        // AX is now wired (UI Automation) — previously `None` on Windows, which
        // silently disabled `desktop_som` and the `desktop_ax_*` tools.
        assert!(platform.ax().is_some());
    }
}
