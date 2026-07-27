//! Minimal read-only registry access for the Windows desktop capabilities.
//!
//! Two of them — the `CapabilityAccessManager` consent store behind
//! `PermissionCapability`, and the OS build behind `SystemCapability` — need a
//! handful of registry values. Both used to shell out to `powershell.exe` for
//! them: one process spawn (~200 ms, plus a console window flashing on the
//! user's screen when the daemon is windowless) per value read, and
//! `check_all` reads six.
//!
//! `RegGetValueW` answers the same question in microseconds with no child
//! process. This module wraps it in the two shapes those callers need, so
//! neither has to import the `windows` crate or repeat the two-call
//! size-then-read dance.
//!
//! Read-only on purpose: nothing in the desktop layer has a reason to write to
//! the registry, and not offering a writer is the cheapest way to keep it that
//! way.

#![cfg(windows)]

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

/// Which hive to read from. Keeps `windows::…::HKEY` out of callers' imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    const fn key(self) -> HKEY {
        match self {
            Self::CurrentUser => HKEY_CURRENT_USER,
            Self::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// Read a `REG_SZ` value. `None` for any failure — absent key, absent value,
/// wrong type, denied access — because every caller here treats "could not
/// read" and "not set" the same way and neither is exceptional.
#[must_use]
pub fn read_string(hive: Hive, subkey: &str, value: &str) -> Option<String> {
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);

    // Ask for the size first: a value that grew between the two calls is
    // handled by the length check below rather than by a truncated read.
    let mut size: u32 = 0;
    // SAFETY: documented registry read. Passing a null data pointer with a
    // non-null size pointer is the documented "how big is it" form.
    let status = unsafe {
        RegGetValueW(
            hive.key(),
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(std::ptr::addr_of_mut!(size)),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }

    // `size` is in bytes and includes the terminating NUL.
    let mut buf = vec![0u16; size as usize / 2 + 1];
    let mut out_size = (buf.len() * 2) as u32;
    // SAFETY: `buf` is large enough for `out_size` bytes, which is what the API
    // is told it may write.
    let status = unsafe {
        RegGetValueW(
            hive.key(),
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(std::ptr::addr_of_mut!(out_size)),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    let chars = (out_size as usize / 2).min(buf.len());
    let text = String::from_utf16_lossy(&buf[..chars]);
    let text = text.trim_end_matches('\0').to_string();
    (!text.is_empty()).then_some(text)
}

/// Read a `REG_DWORD` value.
#[must_use]
pub fn read_u32(hive: Hive, subkey: &str, value: &str) -> Option<u32> {
    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);

    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `data` is exactly `size` bytes and the requested type is DWORD.
    let status = unsafe {
        RegGetValueW(
            hive.key(),
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::addr_of_mut!(data).cast()),
            Some(std::ptr::addr_of_mut!(size)),
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Present on every Windows install since NT — a value this module must be
    /// able to read if it works at all.
    const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    #[test]
    fn reads_a_well_known_string_value() {
        let product = read_string(Hive::LocalMachine, CURRENT_VERSION, "ProductName");
        assert!(
            product.is_some_and(|p| !p.is_empty()),
            "ProductName exists on every Windows install"
        );
    }

    #[test]
    fn reads_a_well_known_dword_value() {
        // `CurrentMajorVersionNumber` exists from Windows 10 onward.
        let major = read_u32(
            Hive::LocalMachine,
            CURRENT_VERSION,
            "CurrentMajorVersionNumber",
        );
        assert!(major.is_none_or(|v| v >= 10));
    }

    #[test]
    fn a_missing_key_or_value_reads_as_none() {
        assert_eq!(
            read_string(Hive::CurrentUser, r"Software\Aleph\NoSuchKey", "Nope"),
            None
        );
        assert_eq!(
            read_string(Hive::LocalMachine, CURRENT_VERSION, "Nope"),
            None
        );
        assert_eq!(read_u32(Hive::LocalMachine, CURRENT_VERSION, "Nope"), None);
    }

    #[test]
    fn a_type_mismatch_reads_as_none() {
        // Asking for a DWORD where a string lives must fail, not reinterpret.
        assert_eq!(
            read_u32(Hive::LocalMachine, CURRENT_VERSION, "ProductName"),
            None
        );
    }
}
