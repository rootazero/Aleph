//! Linux platform implementation for Aleph desktop capabilities.

use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;

/// Linux platform — stub implementation.
///
/// All capability methods currently return `None`. Real implementations
/// will be added incrementally using D-Bus, X11/Wayland APIs, etc.
pub struct LinuxPlatform {
    _private: (),
}

impl LinuxPlatform {
    /// Create a new `LinuxPlatform` instance.
    pub fn new() -> Self {
        Self { _private: () }
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
        None
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
        let platform = LinuxPlatform::default();
        assert_eq!(platform.platform_name(), "Linux");
    }

    #[test]
    fn all_capabilities_return_none() {
        let platform = LinuxPlatform::new();
        assert!(platform.screen().is_none());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
