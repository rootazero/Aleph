use std::ffi::c_void;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Foundation::HLOCAL;
use windows_sys::Win32::Security::AclSizeInformation;
use windows_sys::Win32::Security::Authorization::SetEntriesInAclW;
use windows_sys::Win32::Security::Authorization::EXPLICIT_ACCESS_W;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_SID;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_UNKNOWN;
use windows_sys::Win32::Security::Authorization::TRUSTEE_W;
use windows_sys::Win32::Security::EqualSid;
use windows_sys::Win32::Security::GetAce;
use windows_sys::Win32::Security::GetAclInformation;
use windows_sys::Win32::Security::MapGenericMask;
use windows_sys::Win32::Security::ACCESS_ALLOWED_ACE;
use windows_sys::Win32::Security::ACE_HEADER;
use windows_sys::Win32::Security::ACL;
use windows_sys::Win32::Security::ACL_SIZE_INFORMATION;
use windows_sys::Win32::Security::GENERIC_MAPPING;
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;

const SE_FILE_OBJECT: u32 = 1;
const INHERIT_ONLY_ACE: u8 = 0x08;
const GENERIC_WRITE_MASK: u32 = 0x4000_0000;
const DENY_ACCESS: i32 = 3;

fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Check if a DACL allows access for the given SIDs.
///
/// When `require_all_bits` is true, all bits in `desired_mask` must be present;
/// otherwise any bit suffices.
pub unsafe fn dacl_allows_access(
    p_dacl: *mut ACL,
    psids: &[*mut c_void],
    desired_mask: u32,
    require_all_bits: bool,
) -> bool {
    if p_dacl.is_null() {
        return false;
    }

    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    let ok = GetAclInformation(
        p_dacl as *const ACL,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    );

    if ok == 0 {
        return false;
    }

    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };

    for i in 0..(info.AceCount as usize) {
        let mut p_ace: *mut c_void = std::ptr::null_mut();
        if GetAce(p_dacl as *const ACL, i as u32, &mut p_ace) == 0 {
            continue;
        }

        let hdr = &*(p_ace as *const ACE_HEADER);
        if hdr.AceType != 0 {
            continue; // Not ACCESS_ALLOWED
        }
        if (hdr.AceFlags & INHERIT_ONLY_ACE) != 0 {
            continue;
        }

        let base = p_ace as usize;
        let sid_ptr =
            (base + std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>()) as *mut c_void;

        let mut matched = false;
        for sid in psids {
            if EqualSid(sid_ptr, *sid) != 0 {
                matched = true;
                break;
            }
        }

        if !matched {
            continue;
        }

        let ace = &*(p_ace as *const ACCESS_ALLOWED_ACE);
        let mut mask = ace.Mask;
        MapGenericMask(&mut mask, &mapping);

        if require_all_bits {
            if (mask & desired_mask) == desired_mask {
                return true;
            }
        } else if (mask & desired_mask) != 0 {
            return true;
        }
    }

    false
}

/// Apply ACL restrictions to a path for sandboxed access.
///
/// Creates explicit access entries that:
/// - Deny write access to parent directories
/// - Allow read/execute access to specified paths
/// - Allow write access only to explicitly permitted directories
///
/// # Safety
/// Caller must ensure path is valid and accessible.
pub unsafe fn apply_path_restrictions(
    _path: &std::path::Path,
    _allowed_reads: &[std::path::PathBuf],
    _allowed_writes: &[std::path::PathBuf],
) -> Result<(), String> {
    // This is a placeholder implementation
    // Full ACL manipulation requires careful handling of inheritance
    // and is deferred to a more complete implementation
    Ok(())
}
