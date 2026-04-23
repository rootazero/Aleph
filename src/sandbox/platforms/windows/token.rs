use std::ffi::c_void;
use std::path::Path;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::HLOCAL;
use windows_sys::Win32::Foundation::LUID;
use windows_sys::Win32::Security::AdjustTokenPrivileges;
use windows_sys::Win32::Security::Authorization::SetEntriesInAclW;
use windows_sys::Win32::Security::Authorization::EXPLICIT_ACCESS_W;
use windows_sys::Win32::Security::Authorization::GRANT_ACCESS;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_SID;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_UNKNOWN;
use windows_sys::Win32::Security::Authorization::TRUSTEE_W;
use windows_sys::Win32::Security::CopySid;
use windows_sys::Win32::Security::CreateRestrictedToken;
use windows_sys::Win32::Security::CreateWellKnownSid;
use windows_sys::Win32::Security::GetLengthSid;
use windows_sys::Win32::Security::GetTokenInformation;
use windows_sys::Win32::Security::LookupPrivilegeValueW;
use windows_sys::Win32::Security::SetTokenInformation;
use windows_sys::Win32::Security::ACL;
use windows_sys::Win32::Security::SID_AND_ATTRIBUTES;
use windows_sys::Win32::Security::TOKEN_ADJUST_DEFAULT;
use windows_sys::Win32::Security::TOKEN_ADJUST_PRIVILEGES;
use windows_sys::Win32::Security::TOKEN_ADJUST_SESSIONID;
use windows_sys::Win32::Security::TOKEN_ASSIGN_PRIMARY;
use windows_sys::Win32::Security::TOKEN_DUPLICATE;
use windows_sys::Win32::Security::TOKEN_PRIVILEGES;
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const DISABLE_MAX_PRIVILEGE: u32 = 0x01;
const LUA_TOKEN: u32 = 0x04;
const WRITE_RESTRICTED: u32 = 0x08;
const GENERIC_ALL: u32 = 0x1000_0000;
const WIN_WORLD_SID: i32 = 1;
const SE_GROUP_LOGON_ID: u32 = 0xC0000000;

#[repr(C)]
struct TokenDefaultDaclInfo {
    default_dacl: *mut ACL,
}

/// Convert a Rust string to a wide (UTF-16) string for Windows APIs.
fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Set a permissive default DACL so sandboxed processes can create pipes/IPC.
unsafe fn set_default_dacl(h_token: HANDLE, sids: &[*mut c_void]) -> Result<(), String> {
    if sids.is_empty() {
        return Ok(());
    }

    let entries: Vec<EXPLICIT_ACCESS_W> = sids
        .iter()
        .map(|sid| EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: 0,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: *sid as *mut u16,
            },
        })
        .collect();

    let mut p_new_dacl: *mut ACL = std::ptr::null_mut();
    let res = SetEntriesInAclW(
        entries.len() as u32,
        entries.as_ptr(),
        std::ptr::null_mut(),
        &mut p_new_dacl,
    );

    if res != ERROR_SUCCESS {
        return Err(format!("SetEntriesInAclW failed: {res}"));
    }

    let mut info = TokenDefaultDaclInfo {
        default_dacl: p_new_dacl,
    };
    let ok = SetTokenInformation(
        h_token,
        6, // TokenDefaultDacl
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<TokenDefaultDaclInfo>() as u32,
    );

    if ok == 0 {
        let err = GetLastError();
        if !p_new_dacl.is_null() {
            LocalFree(p_new_dacl as HLOCAL);
        }
        return Err(format!(
            "SetTokenInformation(TokenDefaultDacl) failed: {err}"
        ));
    }

    if !p_new_dacl.is_null() {
        LocalFree(p_new_dacl as HLOCAL);
    }

    Ok(())
}

/// Create a well-known "World" SID (Everyone).
pub unsafe fn world_sid() -> Result<Vec<u8>, String> {
    let mut size: u32 = 0;
    CreateWellKnownSid(
        WIN_WORLD_SID,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut size,
    );

    let mut buf: Vec<u8> = vec![0u8; size as usize];
    let ok = CreateWellKnownSid(
        WIN_WORLD_SID,
        std::ptr::null_mut(),
        buf.as_mut_ptr() as *mut c_void,
        &mut size,
    );

    if ok == 0 {
        return Err(format!("CreateWellKnownSid failed: {}", GetLastError()));
    }

    Ok(buf)
}

/// Get the current process token with permissions needed for restriction.
unsafe fn get_current_token_for_restriction() -> Result<HANDLE, String> {
    let desired = TOKEN_DUPLICATE
        | TOKEN_QUERY
        | TOKEN_ASSIGN_PRIMARY
        | TOKEN_ADJUST_DEFAULT
        | TOKEN_ADJUST_PRIVILEGES
        | TOKEN_ADJUST_SESSIONID;

    let mut h_token: HANDLE = 0;
    let ok =
        windows_sys::Win32::Security::OpenProcessToken(GetCurrentProcess(), desired, &mut h_token);

    if ok == 0 {
        return Err(format!("OpenProcessToken failed: {}", GetLastError()));
    }

    Ok(h_token)
}

/// Disable all privileges on a token.
unsafe fn disable_all_privileges(h_token: HANDLE) -> Result<(), String> {
    let mut privs: TOKEN_PRIVILEGES = std::mem::zeroed();
    privs.PrivilegeCount = 0;

    let ok = AdjustTokenPrivileges(
        h_token,
        1, // DisableAllPrivileges = TRUE
        &privs,
        0,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );

    if ok == 0 {
        return Err(format!("AdjustTokenPrivileges failed: {}", GetLastError()));
    }

    Ok(())
}

/// Create a restricted token for sandboxing.
///
/// The restricted token:
/// - Disables all privileges
/// - Removes all group SIDs (except logon ID)
/// - Adds WRITE_RESTRICTED flag
/// - Sets a permissive default DACL for IPC
///
/// # Safety
/// Caller must close the returned handle with CloseHandle.
pub unsafe fn create_restricted_token() -> Result<HANDLE, String> {
    let h_token = get_current_token_for_restriction()?;

    // Disable all privileges
    if let Err(e) = disable_all_privileges(h_token) {
        CloseHandle(h_token);
        return Err(e);
    }

    // Get token groups to identify SIDs to restrict
    let mut groups_size: u32 = 0;
    GetTokenInformation(h_token, 2, std::ptr::null_mut(), 0, &mut groups_size); // TokenGroups

    let mut groups_buf: Vec<u8> = vec![0; groups_size as usize];
    let ok = GetTokenInformation(
        h_token,
        2, // TokenGroups
        groups_buf.as_mut_ptr() as *mut c_void,
        groups_size,
        &mut groups_size,
    );

    if ok == 0 {
        let err = GetLastError();
        CloseHandle(h_token);
        return Err(format!("GetTokenInformation(TokenGroups) failed: {err}"));
    }

    let groups = &*(groups_buf.as_ptr() as *const windows_sys::Win32::Security::TOKEN_GROUPS);
    let sid_count = groups.GroupCount as usize;
    let sids: &[*mut c_void] =
        std::slice::from_raw_parts(groups.Groups.as_ptr() as *const *mut c_void, sid_count);

    // Filter out logon ID SIDs - these must not be restricted
    let mut restrict_sids: Vec<*mut c_void> = Vec::new();
    for i in 0..sid_count {
        let attr = groups.Groups[i].Attributes;
        if (attr & SE_GROUP_LOGON_ID) == 0 {
            restrict_sids.push(groups.Groups[i].Sid);
        }
    }

    // Create the restricted token
    let mut h_restricted: HANDLE = 0;
    let ok = CreateRestrictedToken(
        h_token,
        DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED,
        0,
        std::ptr::null(),
        restrict_sids.len() as u32,
        restrict_sids.as_ptr() as *const SID_AND_ATTRIBUTES,
        0,
        std::ptr::null(),
        &mut h_restricted,
    );

    if ok == 0 {
        let err = GetLastError();
        CloseHandle(h_token);
        return Err(format!("CreateRestrictedToken failed: {err}"));
    }

    // Set permissive default DACL for IPC
    let world = world_sid()?;
    let world_ptr: *mut c_void = world.as_ptr() as *mut c_void;
    if let Err(e) = set_default_dacl(h_restricted, &[world_ptr]) {
        CloseHandle(h_token);
        CloseHandle(h_restricted);
        return Err(e);
    }

    CloseHandle(h_token);
    Ok(h_restricted)
}
