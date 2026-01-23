//! Search Provider Functions
//!
//! List, configure, and test search providers.

use std::ffi::{c_char, c_int, CStr, CString};

use super::{
    is_initialized, AETHER_ERR_INVALID_ARG, AETHER_ERR_INVALID_UTF8, AETHER_ERR_NOT_INITIALIZED,
    AETHER_ERR_UNKNOWN, AETHER_SUCCESS,
};

// =============================================================================
// Search Provider Functions
// =============================================================================

/// List all search providers as JSON
///
/// # Arguments
/// * `out_json` - Pointer to receive providers list as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_list_search_providers(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual search providers from ffi/search.rs
    let providers = r#"{
        "providers": [
            {"id": "tavily", "name": "Tavily", "category": "web", "enabled": false},
            {"id": "brave", "name": "Brave Search", "category": "web", "enabled": false},
            {"id": "google", "name": "Google CSE", "category": "web", "enabled": false},
            {"id": "bing", "name": "Bing Search", "category": "web", "enabled": false},
            {"id": "searxng", "name": "SearXNG", "category": "web", "enabled": false},
            {"id": "github", "name": "GitHub", "category": "code", "enabled": false}
        ],
        "default_provider": null
    }"#;

    match CString::new(providers) {
        Ok(cstr) => {
            *out_len = providers.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Get search provider configuration
///
/// # Arguments
/// * `provider_id` - Provider ID (UTF-8 encoded, null-terminated)
/// * `out_json` - Pointer to receive config as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_search_provider_config(
    provider_id: *const c_char,
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if provider_id.is_null() || out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let _id_str = match CStr::from_ptr(provider_id).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Get actual provider config
    let config =
        r#"{"api_key": null, "max_results": 10, "safe_search": true, "is_default": false}"#;

    match CString::new(config) {
        Ok(cstr) => {
            *out_len = config.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Update search provider configuration
///
/// # Arguments
/// * `provider_id` - Provider ID (UTF-8 encoded, null-terminated)
/// * `config_json` - Configuration as JSON (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_update_search_provider(
    provider_id: *const c_char,
    config_json: *const c_char,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if provider_id.is_null() || config_json.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let id_str = match CStr::from_ptr(provider_id).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    let config_str = match CStr::from_ptr(config_json).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Update actual search provider config
    tracing::info!(
        "aether_update_search_provider: {} with {}",
        id_str,
        config_str
    );
    AETHER_SUCCESS
}

/// Test search provider connection
///
/// # Arguments
/// * `provider_id` - Provider ID (UTF-8 encoded, null-terminated)
/// * `api_key` - API key to test (UTF-8 encoded, null-terminated)
/// * `out_success` - Pointer to receive success flag (1 = success, 0 = failure)
/// * `out_message` - Pointer to receive result message
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_test_search_provider(
    provider_id: *const c_char,
    api_key: *const c_char,
    out_success: *mut c_int,
    out_message: *mut *mut c_char,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if provider_id.is_null() || api_key.is_null() || out_success.is_null() || out_message.is_null()
    {
        return AETHER_ERR_INVALID_ARG;
    }

    let key_str = match CStr::from_ptr(api_key).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Implement actual connection test
    if key_str.is_empty() {
        *out_success = 0;
        match CString::new("API key is required") {
            Ok(cstr) => {
                *out_message = cstr.into_raw();
                AETHER_SUCCESS
            }
            Err(_) => AETHER_ERR_UNKNOWN,
        }
    } else {
        *out_success = 1;
        match CString::new("Connection successful") {
            Ok(cstr) => {
                *out_message = cstr.into_raw();
                AETHER_SUCCESS
            }
            Err(_) => AETHER_ERR_UNKNOWN,
        }
    }
}
