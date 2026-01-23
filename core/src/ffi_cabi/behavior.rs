//! Behavior Configuration Functions
//!
//! Output mode, PII settings, and formatting configuration.

use std::ffi::{c_char, c_int, CStr, CString};

use super::{
    is_initialized, AETHER_ERR_INVALID_ARG, AETHER_ERR_INVALID_UTF8, AETHER_ERR_NOT_INITIALIZED,
    AETHER_ERR_UNKNOWN, AETHER_SUCCESS,
};

// =============================================================================
// Behavior Configuration Functions
// =============================================================================

/// Get behavior configuration as JSON
///
/// # Arguments
/// * `out_json` - Pointer to receive config as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_behavior_config(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual behavior config
    let config = r#"{
        "output_mode": "typewriter",
        "typing_speed": 50,
        "pii": {
            "enabled": true,
            "scrub_email": true,
            "scrub_phone": true,
            "scrub_ssn": true,
            "scrub_credit_card": true,
            "scrub_ip_address": false
        },
        "formatting": {
            "syntax_highlight": true,
            "markdown": true,
            "code_copy_button": true
        }
    }"#;

    match CString::new(config) {
        Ok(cstr) => {
            *out_len = config.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Update behavior configuration
///
/// # Arguments
/// * `config_json` - Configuration as JSON (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_update_behavior_config(config_json: *const c_char) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if config_json.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let config_str = match CStr::from_ptr(config_json).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Update actual behavior config via ffi/config.rs
    tracing::info!("aether_update_behavior_config: {}", config_str);
    AETHER_SUCCESS
}
