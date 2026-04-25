//! Linux sleep inhibitor stub.
//!
//! Returns `NotImplemented` — callers should treat this as a non-fatal
//! soft failure and proceed without a sleep guard.

use aleph_desktop::{
    error::{DesktopError, Result},
    traits::{InhibitorGuard, PowerCapability},
};

pub struct LinuxPower;

impl LinuxPower {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxPower {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerCapability for LinuxPower {
    fn inhibit_sleep(&self, _reason: &str) -> Result<InhibitorGuard> {
        Err(DesktopError::NotImplemented(
            "sleep inhibitor on Linux".into(),
        ))
    }
}
