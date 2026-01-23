//! Callback Types and Registration
//!
//! This module contains:
//! - Callback function type definitions
//! - Thread-safe callback storage (Callbacks struct)
//! - Callback registration functions

use std::ffi::c_char;
use std::ffi::c_int;
use std::sync::Mutex;

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
pub type ToolCallback =
    extern "C" fn(tool_name: *const c_char, status: c_int, result: *const c_char);

/// Callback function type for memory stored notification
pub type MemoryStoredCallback = extern "C" fn();

/// Callback function type for confirmation required (unified planner integration)
/// @param message Pointer to the confirmation message (UTF-8 encoded, null-terminated)
/// @param plan_json Optional JSON describing the plan that requires confirmation
pub type ConfirmationRequiredCallback =
    extern "C" fn(message: *const c_char, plan_json: *const c_char);

// =============================================================================
// Initialization Callback Types
// =============================================================================

/// Callback for initialization phase started
/// @param phase Phase name (UTF-8, null-terminated)
/// @param current Current phase number (1-based)
/// @param total Total number of phases
pub type InitPhaseStartedCallback = extern "C" fn(phase: *const c_char, current: u32, total: u32);

/// Callback for initialization progress within a phase
/// @param phase Phase name
/// @param progress Progress 0.0 to 1.0
/// @param message Status message (UTF-8, null-terminated)
pub type InitPhaseProgressCallback =
    extern "C" fn(phase: *const c_char, progress: f64, message: *const c_char);

/// Callback for initialization phase completed
/// @param phase Phase name that completed
pub type InitPhaseCompletedCallback = extern "C" fn(phase: *const c_char);

/// Callback for download progress (e.g., embedding model)
/// @param item Item being downloaded
/// @param downloaded Bytes downloaded
/// @param total Total bytes (0 if unknown)
pub type InitDownloadProgressCallback =
    extern "C" fn(item: *const c_char, downloaded: u64, total: u64);

/// Callback for initialization error
/// @param phase Phase where error occurred
/// @param message Error message
/// @param is_retryable 1 if retry might succeed, 0 otherwise
pub type InitErrorCallback =
    extern "C" fn(phase: *const c_char, message: *const c_char, is_retryable: c_int);

// =============================================================================
// Registered Callbacks (thread-safe storage)
// =============================================================================

pub struct Callbacks {
    pub state: Option<StateChangeCallback>,
    pub stream: Option<StreamTextCallback>,
    pub complete: Option<CompleteCallback>,
    pub error: Option<ErrorCallback>,
    pub tool: Option<ToolCallback>,
    pub memory_stored: Option<MemoryStoredCallback>,
    pub confirmation_required: Option<ConfirmationRequiredCallback>,
    // Initialization callbacks
    pub init_phase_started: Option<InitPhaseStartedCallback>,
    pub init_phase_progress: Option<InitPhaseProgressCallback>,
    pub init_phase_completed: Option<InitPhaseCompletedCallback>,
    pub init_download_progress: Option<InitDownloadProgressCallback>,
    pub init_error: Option<InitErrorCallback>,
}

pub static CALLBACKS: Mutex<Callbacks> = Mutex::new(Callbacks {
    state: None,
    stream: None,
    complete: None,
    error: None,
    tool: None,
    memory_stored: None,
    confirmation_required: None,
    // Initialization callbacks
    init_phase_started: None,
    init_phase_progress: None,
    init_phase_completed: None,
    init_download_progress: None,
    init_error: None,
});

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

/// Register a callback for confirmation required (unified planner integration)
///
/// This callback is invoked when the unified planner determines that an action
/// or task graph requires user confirmation before execution.
///
/// # Arguments
/// * `callback` - Function pointer to call when confirmation is required
///
/// # Safety
/// The callback must be valid for the lifetime of the library usage.
#[no_mangle]
pub extern "C" fn aether_register_confirmation_callback(callback: ConfirmationRequiredCallback) {
    if let Ok(mut cbs) = CALLBACKS.lock() {
        cbs.confirmation_required = Some(callback);
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
        cbs.confirmation_required = None;
    }
}
