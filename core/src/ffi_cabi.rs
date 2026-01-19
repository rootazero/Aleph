//! C ABI exports for Windows platform
//!
//! This module provides C-compatible FFI exports for use with csbindgen,
//! enabling Windows applications to call Rust functions via P/Invoke.
//!
//! # Usage
//!
//! Build with the `cabi` feature to enable these exports:
//! ```bash
//! cargo build --release --no-default-features --features cabi
//! ```
//!
//! The csbindgen tool will generate `NativeMethods.g.cs` containing
//! C# P/Invoke declarations for these functions.
//!
//! # Error Codes
//!
//! | Code | Meaning |
//! |------|---------|
//! | 0    | Success |
//! | -1   | Invalid argument (null pointer) |
//! | -2   | Invalid UTF-8 |
//! | -3   | Core not initialized |
//! | -4   | Already initialized |
//! | -5   | Config error |
//! | -6   | Provider error |
//! | -7   | Memory error |
//! | -8   | Cancelled |
//! | -99  | Unknown error |

use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Version string for the Aether core library
static VERSION: &str = env!("CARGO_PKG_VERSION");

/// Flag indicating if core is initialized
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Config path stored after initialization
static CONFIG_PATH: OnceLock<String> = OnceLock::new();

// =============================================================================
// Error Codes
// =============================================================================

pub const AETHER_SUCCESS: c_int = 0;
pub const AETHER_ERR_INVALID_ARG: c_int = -1;
pub const AETHER_ERR_INVALID_UTF8: c_int = -2;
pub const AETHER_ERR_NOT_INITIALIZED: c_int = -3;
pub const AETHER_ERR_ALREADY_INITIALIZED: c_int = -4;
pub const AETHER_ERR_CONFIG: c_int = -5;
pub const AETHER_ERR_PROVIDER: c_int = -6;
pub const AETHER_ERR_MEMORY: c_int = -7;
pub const AETHER_ERR_CANCELLED: c_int = -8;
pub const AETHER_ERR_UNKNOWN: c_int = -99;

// =============================================================================
// Callback Types
// =============================================================================

/// Callback function type for state changes
/// @param state The new state value (see HaloState enum)
pub type StateChangeCallback = extern "C" fn(state: c_int);

/// Callback function type for streaming text
/// @param text Pointer to the text chunk (UTF-8 encoded, null-terminated)
pub type StreamTextCallback = extern "C" fn(text: *const c_char);

/// Callback function type for completion
/// @param response Pointer to the complete response (UTF-8 encoded, null-terminated)
pub type CompleteCallback = extern "C" fn(response: *const c_char);

/// Callback function type for errors
/// @param message Pointer to the error message (UTF-8 encoded, null-terminated)
/// @param code Error code
pub type ErrorCallback = extern "C" fn(message: *const c_char, code: c_int);

/// Callback function type for tool execution
/// @param tool_name Name of the tool being executed
/// @param status Tool status (0=started, 1=completed, 2=failed)
/// @param result Tool result or error message
pub type ToolCallback = extern "C" fn(tool_name: *const c_char, status: c_int, result: *const c_char);

/// Callback function type for memory stored notification
pub type MemoryStoredCallback = extern "C" fn();

// =============================================================================
// Registered Callbacks (thread-safe storage)
// =============================================================================

use std::sync::Mutex;

struct Callbacks {
    state: Option<StateChangeCallback>,
    stream: Option<StreamTextCallback>,
    complete: Option<CompleteCallback>,
    error: Option<ErrorCallback>,
    tool: Option<ToolCallback>,
    memory_stored: Option<MemoryStoredCallback>,
}

static CALLBACKS: Mutex<Callbacks> = Mutex::new(Callbacks {
    state: None,
    stream: None,
    complete: None,
    error: None,
    tool: None,
    memory_stored: None,
});

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
    if INITIALIZED.load(Ordering::SeqCst) { 1 } else { 0 }
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

// =============================================================================
// Callback Registration Functions
// =============================================================================

/// Register a callback for state changes
///
/// # Arguments
/// * `callback` - Function pointer to call when state changes
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub extern "C" fn aether_register_state_callback(callback: StateChangeCallback) {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.state = Some(callback);
    }
}

/// Register a callback for streaming text
///
/// # Arguments
/// * `callback` - Function pointer to call when streaming text is received
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub extern "C" fn aether_register_stream_callback(callback: StreamTextCallback) {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.stream = Some(callback);
    }
}

/// Register a callback for completion
///
/// # Arguments
/// * `callback` - Function pointer to call when processing completes
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub extern "C" fn aether_register_complete_callback(callback: CompleteCallback) {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.complete = Some(callback);
    }
}

/// Register a callback for errors
///
/// # Arguments
/// * `callback` - Function pointer to call when an error occurs
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub extern "C" fn aether_register_error_callback(callback: ErrorCallback) {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.error = Some(callback);
    }
}

/// Register a callback for tool execution
///
/// # Arguments
/// * `callback` - Function pointer to call when a tool is executed
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub extern "C" fn aether_register_tool_callback(callback: ToolCallback) {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.tool = Some(callback);
    }
}

/// Register a callback for memory stored notification
///
/// # Arguments
/// * `callback` - Function pointer to call when memory is stored
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub extern "C" fn aether_register_memory_stored_callback(callback: MemoryStoredCallback) {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.memory_stored = Some(callback);
    }
}

/// Clear all registered callbacks
#[no_mangle]
pub extern "C" fn aether_clear_callbacks() {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.state = None;
        cbs.stream = None;
        cbs.complete = None;
        cbs.error = None;
        cbs.tool = None;
        cbs.memory_stored = None;
    }
}

// =============================================================================
// Processing Functions
// =============================================================================

/// Process user input
///
/// # Arguments
/// * `input` - User input text (UTF-8 encoded, null-terminated)
/// * `app_context` - Optional app context/bundle ID (UTF-8 encoded, null-terminated, can be null)
/// * `window_title` - Optional window title (UTF-8 encoded, null-terminated, can be null)
/// * `topic_id` - Optional topic ID for multi-turn conversation (can be null)
/// * `stream` - Whether to stream the response (1 = stream, 0 = wait for complete)
///
/// # Returns
/// * `0` on success (processing started)
/// * Error code on failure
///
/// # Safety
/// All string parameters must be valid null-terminated UTF-8 strings or null.
#[no_mangle]
pub unsafe extern "C" fn aether_process(
    input: *const c_char,
    app_context: *const c_char,
    window_title: *const c_char,
    topic_id: *const c_char,
    stream: c_int,
) -> c_int {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if input.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let input_str = match CStr::from_ptr(input).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    let _app_context_str = if app_context.is_null() {
        None
    } else {
        match CStr::from_ptr(app_context).to_str() {
            Ok(s) => Some(s),
            Err(_) => return AETHER_ERR_INVALID_UTF8,
        }
    };

    let _window_title_str = if window_title.is_null() {
        None
    } else {
        match CStr::from_ptr(window_title).to_str() {
            Ok(s) => Some(s),
            Err(_) => return AETHER_ERR_INVALID_UTF8,
        }
    };

    let _topic_id_str = if topic_id.is_null() {
        None
    } else {
        match CStr::from_ptr(topic_id).to_str() {
            Ok(s) => Some(s),
            Err(_) => return AETHER_ERR_INVALID_UTF8,
        }
    };

    let _stream_enabled = stream != 0;

    // TODO: Implement actual processing
    // For now, simulate a response
    tracing::info!("aether_process called with input: {}", input_str);

    // Simulate state change callback
    if let Ok(cbs) = CALLBACKS.lock() {
        if let Some(state_cb) = cbs.state {
            state_cb(4); // Thinking state
        }
    }

    AETHER_SUCCESS
}

/// Cancel the current processing operation
///
/// # Returns
/// * `0` on success
/// * `-3` if not initialized
#[no_mangle]
pub extern "C" fn aether_cancel() -> c_int {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    // TODO: Implement cancellation logic
    tracing::info!("aether_cancel called");
    AETHER_SUCCESS
}

/// Check if the current operation is cancelled
///
/// # Returns
/// * `1` if cancelled
/// * `0` if not cancelled
#[no_mangle]
pub extern "C" fn aether_is_cancelled() -> c_int {
    // TODO: Implement cancellation check
    0
}

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
    if !INITIALIZED.load(Ordering::SeqCst) {
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
    if !INITIALIZED.load(Ordering::SeqCst) {
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
    if !INITIALIZED.load(Ordering::SeqCst) {
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
    if !INITIALIZED.load(Ordering::SeqCst) {
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
    if !INITIALIZED.load(Ordering::SeqCst) {
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
    if !INITIALIZED.load(Ordering::SeqCst) {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if provider_name.is_null() || config_json.is_null() || out_success.is_null() || out_message.is_null() {
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

// =============================================================================
// Memory Management Functions
// =============================================================================

/// Search memory
///
/// # Arguments
/// * `query` - Search query (UTF-8 encoded, null-terminated)
/// * `limit` - Maximum number of results
/// * `out_json` - Pointer to receive results as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_search_memory(
    query: *const c_char,
    limit: c_int,
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if query.is_null() || out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let _query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Implement actual memory search
    let results = format!(r#"{{"results":[],"limit":{}}}"#, limit);

    match CString::new(results.as_str()) {
        Ok(cstr) => {
            *out_len = results.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Clear all memory
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub extern "C" fn aether_clear_memory() -> c_int {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    // TODO: Implement actual memory clear
    tracing::info!("aether_clear_memory called");
    AETHER_SUCCESS
}

/// Get memory statistics
///
/// # Arguments
/// * `out_json` - Pointer to receive stats as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_memory_stats(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !INITIALIZED.load(Ordering::SeqCst) {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual memory stats
    let stats = r#"{"total_memories":0,"total_apps":0,"database_size_mb":0.0}"#;

    match CString::new(stats) {
        Ok(cstr) => {
            *out_len = stats.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

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
    if !INITIALIZED.load(Ordering::SeqCst) {
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
    if !INITIALIZED.load(Ordering::SeqCst) {
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
    if !INITIALIZED.load(Ordering::SeqCst) {
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

// =============================================================================
// Tests
// =============================================================================

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

    #[test]
    fn test_init_free() {
        unsafe {
            // Reset state
            INITIALIZED.store(false, Ordering::SeqCst);

            let config_path = CString::new("/tmp/test.toml").unwrap();
            let result = aether_init(config_path.as_ptr());
            assert_eq!(result, AETHER_SUCCESS);
            assert_eq!(aether_is_initialized(), 1);

            // Double init should fail
            let result = aether_init(config_path.as_ptr());
            assert_eq!(result, AETHER_ERR_ALREADY_INITIALIZED);

            let result = aether_free();
            assert_eq!(result, AETHER_SUCCESS);
            assert_eq!(aether_is_initialized(), 0);
        }
    }

    #[test]
    fn test_null_checks() {
        unsafe {
            assert_eq!(aether_init(std::ptr::null()), AETHER_ERR_INVALID_ARG);
            assert_eq!(
                aether_process(std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0),
                AETHER_ERR_NOT_INITIALIZED
            );
        }
    }
}
