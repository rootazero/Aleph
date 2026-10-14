//! Read-only probe: is the current process token elevated (UAC "full" token)?
//!
//! Persistent WFP filters need an elevated token. This module ONLY reads
//! `TokenElevation` from the current process token; it never requests
//! elevation, changes a token, or touches any other OS state.

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `Ok(true)` when the current process token is elevated. Errors are
/// reported, never collapsed to `false`, so callers can tell "not elevated"
/// from "could not tell".
pub(crate) fn process_is_elevated() -> Result<bool, String> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` is a valid out-pointer.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        // SAFETY: plain thread-local error read.
        return Err(format!("OpenProcessToken failed: {:#010x}", unsafe {
            GetLastError()
        }));
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut returned: u32 = 0;
    // SAFETY: `elevation` is a correctly sized TOKEN_ELEVATION and `token` is an open TOKEN_QUERY handle.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: read the error before CloseHandle can overwrite it; close the handle we opened.
    let err = if ok == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(format!(
            "GetTokenInformation(TokenElevation) failed: {err:#010x}"
        ));
    }
    Ok(elevation.TokenIsElevated != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe must be able to answer (either value is valid on CI).
    #[test]
    fn probe_answers_without_error() {
        process_is_elevated().expect("TokenElevation query on own token");
    }
}
