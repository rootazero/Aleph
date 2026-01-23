//! Skills Management Functions
//!
//! List, install, delete, and refresh skills.

use std::ffi::{c_char, c_int, CStr, CString};

use super::{
    is_initialized, AETHER_ERR_INVALID_ARG, AETHER_ERR_INVALID_UTF8, AETHER_ERR_NOT_INITIALIZED,
    AETHER_ERR_UNKNOWN, AETHER_SUCCESS,
};

// =============================================================================
// Skills Management Functions
// =============================================================================

/// List all installed skills as JSON
///
/// # Arguments
/// * `out_json` - Pointer to receive skills list as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
///
/// # Safety
/// The caller must free the returned string using `aether_free_string`.
#[no_mangle]
pub unsafe extern "C" fn aether_list_skills(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual skills from core
    let skills = r#"{"skills":[]}"#;

    match CString::new(skills) {
        Ok(cstr) => {
            *out_len = skills.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Install a skill from URL
///
/// # Arguments
/// * `url` - GitHub URL or direct download URL (UTF-8 encoded, null-terminated)
/// * `out_json` - Pointer to receive installed skill info as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_install_skill(
    url: *const c_char,
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if url.is_null() || out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let url_str = match CStr::from_ptr(url).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Install actual skill
    tracing::info!("aether_install_skill called with url: {}", url_str);

    // Return placeholder skill info
    let skill_info = r#"{"id":"new-skill","name":"New Skill","description":"Installed skill"}"#;

    match CString::new(skill_info) {
        Ok(cstr) => {
            *out_len = skill_info.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Install skills from ZIP file
///
/// # Arguments
/// * `zip_path` - Path to ZIP file (UTF-8 encoded, null-terminated)
/// * `out_json` - Pointer to receive installed skill IDs as JSON array
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_install_skills_from_zip(
    zip_path: *const c_char,
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if zip_path.is_null() || out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let path_str = match CStr::from_ptr(zip_path).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Install actual skills from ZIP
    tracing::info!(
        "aether_install_skills_from_zip called with path: {}",
        path_str
    );

    // Return placeholder skill IDs
    let skill_ids = r#"{"skill_ids":[]}"#;

    match CString::new(skill_ids) {
        Ok(cstr) => {
            *out_len = skill_ids.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Delete a skill
///
/// # Arguments
/// * `skill_id` - Skill ID (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_delete_skill(skill_id: *const c_char) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if skill_id.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let id_str = match CStr::from_ptr(skill_id).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Delete actual skill
    tracing::info!("aether_delete_skill called with id: {}", id_str);
    AETHER_SUCCESS
}

/// Get skills directory path
///
/// # Arguments
/// * `out_path` - Pointer to receive path
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_skills_dir(out_path: *mut *mut c_char) -> c_int {
    if out_path.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // Get skills directory path
    let path = if cfg!(windows) {
        std::env::var("APPDATA")
            .map(|p| format!("{}\\Aether\\skills", p))
            .unwrap_or_else(|_| "C:\\Aether\\skills".to_string())
    } else {
        dirs::data_local_dir()
            .map(|p| {
                p.join("aether")
                    .join("skills")
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|| "~/.local/share/aether/skills".to_string())
    };

    match CString::new(path) {
        Ok(cstr) => {
            *out_path = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Refresh skills registry
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub extern "C" fn aether_refresh_skills() -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    // TODO: Refresh actual skills
    tracing::info!("aether_refresh_skills called");
    AETHER_SUCCESS
}
