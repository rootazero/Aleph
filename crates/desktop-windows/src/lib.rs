//! Windows platform implementation for Aleph desktop capabilities.

use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;

/// Windows platform — stub implementation.
///
/// All capability methods currently return `None`. Real implementations
/// will be added incrementally using Win32/WinRT APIs.
pub struct WindowsPlatform {
    _private: (),
}

impl WindowsPlatform {
    /// Create a new `WindowsPlatform` instance.
    pub fn new() -> Self {
        Self { _private: () }
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
        let platform = WindowsPlatform::default();
        assert_eq!(platform.platform_name(), "Windows");
    }

    #[test]
    fn all_capabilities_return_none() {
        let platform = WindowsPlatform::new();
        assert!(platform.screen().is_none());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
