//! macOS sleep inhibitor via IOPMAssertion.
//!
//! Prevents system idle sleep for the duration of an [`InhibitorGuard`].
//! The assertion type `PreventUserIdleSystemSleep` appears in `pmset -g assertions`.

use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::CFString;

use aleph_desktop::{
    error::{DesktopError, Result},
    traits::{InhibitorGuard, PowerCapability},
};

type IOPMAssertionID = u32;
type IOPMAssertionLevel = u32;

const K_IO_PM_ASSERTION_LEVEL_ON: IOPMAssertionLevel = 255;
const K_IO_PM_ASSERTION_TYPE: &str = "PreventUserIdleSystemSleep";
const K_IO_RETURN_SUCCESS: i32 = 0;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFTypeRef,
        level: IOPMAssertionLevel,
        name: CFTypeRef,
        id: *mut IOPMAssertionID,
    ) -> i32;
    fn IOPMAssertionRelease(id: IOPMAssertionID) -> i32;
}

pub struct MacosPower;

impl MacosPower {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacosPower {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerCapability for MacosPower {
    fn inhibit_sleep(&self, reason: &str) -> Result<InhibitorGuard> {
        let ty = CFString::new(K_IO_PM_ASSERTION_TYPE);
        let name = CFString::new(reason);
        let mut id: IOPMAssertionID = 0;
        // SAFETY: both CFString refs live until after the call; id is a valid out-pointer.
        let status = unsafe {
            IOPMAssertionCreateWithName(
                ty.as_concrete_TypeRef() as CFTypeRef,
                K_IO_PM_ASSERTION_LEVEL_ON,
                name.as_concrete_TypeRef() as CFTypeRef,
                &mut id,
            )
        };
        if status != K_IO_RETURN_SUCCESS {
            return Err(DesktopError::PlatformError(format!(
                "IOPMAssertionCreateWithName failed: {status}"
            )));
        }
        tracing::debug!(target: "power", "inhibitor acquired reason={reason} id={id:#x}");
        let id_copy = id;
        Ok(InhibitorGuard::new(move || {
            // SAFETY: id was produced by a successful create call above.
            let _ = unsafe { IOPMAssertionRelease(id_copy) };
            tracing::debug!(target: "power", "inhibitor released id={id_copy:#x}");
        }))
    }
}
