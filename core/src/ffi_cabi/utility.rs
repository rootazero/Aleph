//! Utility Functions
//!
//! String management, error handling, configuration reload, logging, and tool listing.

use std::ffi::{c_char, c_int, CString};

use super::{
    is_initialized, AETHER_ERR_INVALID_ARG, AETHER_ERR_NOT_INITIALIZED, AETHER_ERR_UNKNOWN,
    AETHER_SUCCESS,
};

// =============================================================================
// Utility Functions
// =============================================================================

/// Free a string allocated by the library
///
/// # Arguments
/// * `ptr` - Pointer to the string to free
///
/// # Safety
/// Only pass pointers returned by other aether_* functions.
#[no_mangle]
pub unsafe extern "C" fn aether_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr);
    }
}

/// Get the last error message
///
/// # Arguments
/// * `out_message` - Pointer to receive error message
///
/// # Returns
/// * `0` on success
/// * `-1` if no error available
#[no_mangle]
pub unsafe extern "C" fn aether_get_last_error(out_message: *mut *mut c_char) -> c_int {
    if out_message.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Implement error message storage
    match CString::new("No error") {
        Ok(cstr) => {
            *out_message = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Reload configuration from disk
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub extern "C" fn aether_reload_config() -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    // TODO: Implement actual config reload
    tracing::info!("aether_reload_config called");
    AETHER_SUCCESS
}

// =============================================================================
// Tool and Command Functions
// =============================================================================

/// Get list of available tools as JSON
///
/// # Arguments
/// * `out_json` - Pointer to receive tools list as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_list_tools(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual tools list
    let tools = r#"{"tools":[]}"#;

    match CString::new(tools) {
        Ok(cstr) => {
            *out_len = tools.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Get root commands for command completion
///
/// # Arguments
/// * `out_json` - Pointer to receive commands as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_root_commands(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual root commands
    let commands = r#"{"commands":[]}"#;

    match CString::new(commands) {
        Ok(cstr) => {
            *out_len = commands.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

// =============================================================================
// Logging Functions
// =============================================================================

/// Set log level
///
/// # Arguments
/// * `level` - Log level (0=Error, 1=Warn, 2=Info, 3=Debug, 4=Trace)
///
/// # Returns
/// * `0` on success
#[no_mangle]
pub extern "C" fn aether_set_log_level(level: c_int) -> c_int {
    // TODO: Set actual log level
    tracing::info!("aether_set_log_level called with level: {}", level);
    AETHER_SUCCESS
}

/// Get log directory path
///
/// # Arguments
/// * `out_path` - Pointer to receive path
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_log_directory(out_path: *mut *mut c_char) -> c_int {
    if out_path.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual log directory
    let path = if cfg!(windows) {
        std::env::var("APPDATA")
            .map(|p| format!("{}\\Aether\\logs", p))
            .unwrap_or_else(|_| "C:\\Aether\\logs".to_string())
    } else {
        dirs::data_local_dir()
            .map(|p| p.join("aether").join("logs").to_string_lossy().to_string())
            .unwrap_or_else(|| "~/.local/share/aether/logs".to_string())
    };

    match CString::new(path) {
        Ok(cstr) => {
            *out_path = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}
