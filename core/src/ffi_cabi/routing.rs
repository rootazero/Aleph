//! Routing Configuration Functions
//!
//! Model routing configuration management.

use std::ffi::{c_char, c_int, CStr, CString};

use super::{
    is_initialized, AETHER_ERR_INVALID_ARG, AETHER_ERR_INVALID_UTF8, AETHER_ERR_NOT_INITIALIZED,
    AETHER_ERR_UNKNOWN, AETHER_SUCCESS,
};

// =============================================================================
// Routing Configuration Functions
// =============================================================================

/// Get routing configuration as JSON
///
/// # Arguments
/// * `out_json` - Pointer to receive config as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_routing_config(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual routing config
    let config = r#"{
        "cost_strategy": "balanced",
        "default_model": "claude-3-5-sonnet",
        "pipeline_enabled": true,
        "task_routing": {
            "code_generation": "default",
            "code_review": "default",
            "image_analysis": "default",
            "video_understanding": "default",
            "long_document": "default",
            "quick_tasks": "claude-3-haiku",
            "privacy_sensitive": "default",
            "reasoning": "claude-3-opus"
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

/// Update routing configuration
///
/// # Arguments
/// * `config_json` - Configuration as JSON (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_update_routing_config(config_json: *const c_char) -> c_int {
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

    // TODO: Update actual routing config via ffi/config.rs
    tracing::info!("aether_update_routing_config: {}", config_str);
    AETHER_SUCCESS
}
