//! Configuration Functions
//!
//! Configuration loading and provider management.

use std::ffi::{c_char, c_int, CStr, CString};

use super::{
    is_initialized, AETHER_ERR_INVALID_ARG, AETHER_ERR_INVALID_UTF8, AETHER_ERR_NOT_INITIALIZED,
    AETHER_ERR_UNKNOWN, AETHER_SUCCESS,
};

// =============================================================================
// Configuration Functions
// =============================================================================

/// Load configuration and return as JSON string
///
/// # Arguments
/// * `out_json` - Pointer to receive the JSON string pointer
/// * `out_len` - Pointer to receive the JSON string length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
///
/// # Safety
/// The caller must free the returned string using `aether_free_string`.
#[no_mangle]
pub unsafe extern "C" fn aether_load_config(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Load actual config
    // For now, return a placeholder JSON
    let config_json = r#"{"version":"0.1.0","default_hotkey":"DoubleTap+leftShift"}"#;

    match CString::new(config_json) {
        Ok(cstr) => {
            *out_len = config_json.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Get the default provider name
///
/// # Arguments
/// * `out_provider` - Pointer to receive the provider name pointer
///
/// # Returns
/// * `0` on success
/// * Error code on failure
///
/// # Safety
/// The caller must free the returned string using `aether_free_string`.
#[no_mangle]
pub unsafe extern "C" fn aether_get_default_provider(out_provider: *mut *mut c_char) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_provider.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual default provider
    let provider = "openai";

    match CString::new(provider) {
        Ok(cstr) => {
            *out_provider = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Set the default provider
///
/// # Arguments
/// * `provider_name` - The provider name (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_set_default_provider(provider_name: *const c_char) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if provider_name.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let name = match CStr::from_ptr(provider_name).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Set actual default provider
    tracing::info!("aether_set_default_provider called with: {}", name);
    AETHER_SUCCESS
}

/// Update provider configuration
///
/// # Arguments
/// * `name` - Provider name (UTF-8 encoded, null-terminated)
/// * `config_json` - Provider config as JSON (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_update_provider(
    name: *const c_char,
    config_json: *const c_char,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if name.is_null() || config_json.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    let config_str = match CStr::from_ptr(config_json).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Update actual provider
    tracing::info!(
        "aether_update_provider called: {} with config: {}",
        name_str,
        config_str
    );
    AETHER_SUCCESS
}

/// Delete a provider
///
/// # Arguments
/// * `name` - Provider name (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_delete_provider(name: *const c_char) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if name.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Delete actual provider
    tracing::info!("aether_delete_provider called: {}", name_str);
    AETHER_SUCCESS
}

/// Test provider connection
///
/// # Arguments
/// * `provider_name` - Provider name (UTF-8 encoded, null-terminated)
/// * `config_json` - Provider config as JSON (UTF-8 encoded, null-terminated)
/// * `out_success` - Pointer to receive success flag (1 = success, 0 = failure)
/// * `out_message` - Pointer to receive result message
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_test_provider_connection(
    provider_name: *const c_char,
    config_json: *const c_char,
    out_success: *mut c_int,
    out_message: *mut *mut c_char,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if provider_name.is_null()
        || config_json.is_null()
        || out_success.is_null()
        || out_message.is_null()
    {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Implement actual connection test
    *out_success = 1;
    match CString::new("Connection successful") {
        Ok(cstr) => {
            *out_message = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}
