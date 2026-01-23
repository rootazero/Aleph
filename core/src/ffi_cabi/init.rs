//! Initialization Functions
//!
//! Core lifecycle management for the Aether library.

use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::atomic::Ordering;

use super::{
    CONFIG_PATH, INITIALIZED, AETHER_ERR_ALREADY_INITIALIZED, AETHER_ERR_INVALID_ARG,
    AETHER_ERR_INVALID_UTF8, AETHER_SUCCESS, VERSION,
};

// =============================================================================
// Initialization Functions
// =============================================================================

/// Initialize the Aether core library
///
/// # Arguments
/// * `config_path` - Path to the configuration file (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * `-1` if config_path is null
/// * `-2` if config_path is not valid UTF-8
/// * `-4` if already initialized
/// * `-5` if config error
///
/// # Safety
/// The `config_path` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn aether_init(config_path: *const c_char) -> c_int {
    if config_path.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    if INITIALIZED.load(Ordering::SeqCst) {
        return AETHER_ERR_ALREADY_INITIALIZED;
    }

    let path = match CStr::from_ptr(config_path).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // Store config path
    let _ = CONFIG_PATH.set(path.to_string());

    // TODO: Initialize core with config path
    // This will be implemented when integrating with actual core logic
    tracing::info!("aether_init called with path: {}", path);

    INITIALIZED.store(true, Ordering::SeqCst);
    AETHER_SUCCESS
}

/// Check if the core is initialized
///
/// # Returns
/// * `1` if initialized
/// * `0` if not initialized
#[no_mangle]
pub extern "C" fn aether_is_initialized() -> c_int {
    if INITIALIZED.load(Ordering::SeqCst) {
        1
    } else {
        0
    }
}

/// Free resources allocated by the Aether core library
///
/// Should be called when the application is shutting down.
///
/// # Returns
/// * `0` on success
#[no_mangle]
pub extern "C" fn aether_free() -> c_int {
    // TODO: Clean up resources
    tracing::info!("aether_free called");
    INITIALIZED.store(false, Ordering::SeqCst);
    AETHER_SUCCESS
}

/// Get the version string of the Aether core library
///
/// # Returns
/// A pointer to a null-terminated UTF-8 string containing the version.
/// The returned string is statically allocated and should not be freed.
///
/// # Safety
/// The returned pointer is valid for the lifetime of the library.
#[no_mangle]
pub extern "C" fn aether_version() -> *const c_char {
    // Use a static CString to ensure the pointer remains valid
    static VERSION_CSTR: once_cell::sync::Lazy<CString> =
        once_cell::sync::Lazy::new(|| CString::new(VERSION).unwrap());

    VERSION_CSTR.as_ptr()
}
