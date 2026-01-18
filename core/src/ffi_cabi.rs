//! C ABI exports for Windows platform
//!
//! This module provides C-compatible FFI exports for use with csbindgen,
//! enabling Windows applications to call Rust functions via P/Invoke.
//!
//! # Usage
//!
//! Build with the `cabi` feature to enable these exports:
//! ```bash
//! cargo build --release --features cabi
//! ```
//!
//! The csbindgen tool will generate `NativeMethods.g.cs` containing
//! C# P/Invoke declarations for these functions.

use std::ffi::{c_char, c_int, CStr, CString};

/// Version string for the Aether core library
static VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialize the Aether core library
///
/// # Arguments
/// * `config_path` - Path to the configuration file (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Non-zero error code on failure
///
/// # Safety
/// The `config_path` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn aether_init(config_path: *const c_char) -> c_int {
    if config_path.is_null() {
        return -1; // Invalid argument
    }

    let path = match CStr::from_ptr(config_path).to_str() {
        Ok(s) => s,
        Err(_) => return -2, // Invalid UTF-8
    };

    // TODO: Initialize core with config path
    tracing::info!("aether_init called with path: {}", path);

    0 // Success
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
    0
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

/// Callback function type for state changes
pub type StateChangeCallback = extern "C" fn(state: c_int);

/// Callback function type for streaming text
pub type StreamTextCallback = extern "C" fn(text: *const c_char);

/// Callback function type for errors
pub type ErrorCallback = extern "C" fn(message: *const c_char, code: c_int);

/// Registered callbacks (placeholder for actual implementation)
static mut STATE_CALLBACK: Option<StateChangeCallback> = None;
static mut STREAM_CALLBACK: Option<StreamTextCallback> = None;
static mut ERROR_CALLBACK: Option<ErrorCallback> = None;

/// Register a callback for state changes
///
/// # Arguments
/// * `callback` - Function pointer to call when state changes
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub unsafe extern "C" fn aether_register_state_callback(callback: StateChangeCallback) {
    STATE_CALLBACK = Some(callback);
}

/// Register a callback for streaming text
///
/// # Arguments
/// * `callback` - Function pointer to call when streaming text is received
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub unsafe extern "C" fn aether_register_stream_callback(callback: StreamTextCallback) {
    STREAM_CALLBACK = Some(callback);
}

/// Register a callback for errors
///
/// # Arguments
/// * `callback` - Function pointer to call when an error occurs
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub unsafe extern "C" fn aether_register_error_callback(callback: ErrorCallback) {
    ERROR_CALLBACK = Some(callback);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        let version_ptr = aether_version();
        assert!(!version_ptr.is_null());
        let version = unsafe { CStr::from_ptr(version_ptr).to_str().unwrap() };
        assert_eq!(version, "0.1.0");
    }
}
