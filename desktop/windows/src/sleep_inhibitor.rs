//! Windows sleep inhibitor via the Power Request API.
//!
//! Each `inhibit_sleep` creates an independent power request and asserts
//! `PowerRequestSystemRequired`; the returned guard clears the request and
//! closes the handle on drop. This mirrors the handle-per-assertion model of
//! the macOS `IOPMAssertion` backend.
//!
//! Why not `SetThreadExecutionState`: that API is *thread-affine* — the
//! assertion is bound to the calling thread and is lost when that thread
//! terminates, and clearing it from a different thread clears the wrong
//! thread's state. Under an async runtime that moves work between worker
//! threads this silently drops the inhibition. `PowerCreateRequest` is
//! process-scoped and handle-based, so it is immune to both problems.

use aleph_desktop::{
    error::Result,
    traits::{InhibitorGuard, PowerCapability},
};

pub struct WindowsPower;

impl WindowsPower {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for WindowsPower {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerCapability for WindowsPower {
    #[cfg(windows)]
    fn inhibit_sleep(&self, reason: &str) -> Result<InhibitorGuard> {
        use aleph_desktop::error::DesktopError;
        use windows::core::PWSTR;
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::Power::{
            PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
        };
        use windows::Win32::System::Threading::{
            POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
        };

        // POWER_REQUEST_CONTEXT_VERSION (documented as 0); lives behind a
        // windows-crate feature we do not otherwise need, so inline the literal.
        const POWER_REQUEST_CONTEXT_VERSION: u32 = 0;

        // Null-terminated UTF-16 reason string. The OS copies it into the
        // request object, so the buffer only needs to outlive the create call.
        let mut wide: Vec<u16> = reason.encode_utf16().chain(std::iter::once(0)).collect();

        let context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: PWSTR(wide.as_mut_ptr()),
            },
        };

        // SAFETY: `context` is a valid REASON_CONTEXT with the SIMPLE_STRING flag
        // set and a live, null-terminated wide string for the duration of the call.
        let handle: HANDLE = unsafe { PowerCreateRequest(&raw const context) }
            .map_err(|e| DesktopError::PlatformError(format!("PowerCreateRequest failed: {e}")))?;

        // SAFETY: `handle` was just produced by a successful PowerCreateRequest.
        if let Err(e) = unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) } {
            // Close the handle we just created before surfacing the error.
            // SAFETY: `handle` is valid and not yet closed.
            let _ = unsafe { CloseHandle(handle) };
            return Err(DesktopError::PlatformError(format!(
                "PowerSetRequest failed: {e}"
            )));
        }

        // Carry the handle across the guard closure as an integer so the closure
        // stays `Send` (the raw `HANDLE` pointer is not).
        let raw = handle.0 as isize;
        Ok(InhibitorGuard::new(move || {
            let handle = HANDLE(raw as *mut core::ffi::c_void);
            // SAFETY: `handle` was created above and has not been closed yet.
            let _ = unsafe { PowerClearRequest(handle, PowerRequestSystemRequired) };
            let _ = unsafe { CloseHandle(handle) };
        }))
    }

    #[cfg(not(windows))]
    fn inhibit_sleep(&self, reason: &str) -> Result<InhibitorGuard> {
        // The Power Request API is Windows-only; on other targets this crate is
        // compiled solely for type-checking, so return a no-op guard.
        let _ = reason;
        Ok(InhibitorGuard::noop())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-API smoke test: on Windows this actually calls
    /// `PowerCreateRequest` + `PowerSetRequest` (neither needs elevation — only
    /// *viewing* requests via `powercfg /requests` does). A wrong request-type
    /// constant or a bad `REASON_CONTEXT` ABI would make `PowerSetRequest`
    /// return an error here, so a green run proves both are correct.
    #[test]
    fn acquire_and_release_succeeds() {
        let power = WindowsPower::new();
        match power.inhibit_sleep("aleph-sleep-inhibitor-unit-test") {
            Ok(guard) => drop(guard), // release + CloseHandle must not panic
            Err(e) => panic!("inhibit_sleep should succeed: {e}"),
        }
    }
}
