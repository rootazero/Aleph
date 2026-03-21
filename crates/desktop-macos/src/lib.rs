//! macOS platform implementation for Aleph desktop capabilities.

use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;

/// macOS platform — stub implementation.
///
/// All capability methods currently return `None`. Real implementations
/// will be added incrementally using Apple frameworks (Vision, AppKit, etc.).
pub struct MacOSPlatform {
    _private: (),
}

impl MacOSPlatform {
    /// Create a new `MacOSPlatform` instance.
    pub fn new() -> Self {
        Self { _private: () }
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
        let platform = MacOSPlatform::default();
        assert_eq!(platform.platform_name(), "macOS");
    }

    #[test]
    fn all_capabilities_return_none() {
        let platform = MacOSPlatform::new();
        assert!(platform.screen().is_none());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
