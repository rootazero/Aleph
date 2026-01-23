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
//! # Safety
//!
//! All `unsafe extern "C"` functions in this module follow these safety contracts:
//!
//! - **Pointer parameters**: Must be valid, non-null, and properly aligned unless
//!   documented otherwise. Null checks are performed and return `AETHER_ERR_INVALID_ARG`.
//! - **String parameters** (`*const c_char`): Must be valid null-terminated UTF-8 strings.
//!   Invalid UTF-8 returns `AETHER_ERR_INVALID_UTF8`.
//! - **Output parameters** (`*mut *mut c_char`): Must be valid pointers to receive allocated
//!   strings. Caller is responsible for freeing with `aether_free_string`.
//! - **Memory ownership**: Strings returned via output parameters are allocated by Rust
//!   and must be freed by calling `aether_free_string`. Never free with C's `free()`.
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

mod agent;
mod behavior;
mod callback;
mod config;
mod first_time_init;
mod generation;
mod init;
mod mcp;
mod memory;
mod processing;
mod routing;
mod runtime;
mod search;
mod skills;
mod utility;

use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

// =============================================================================
// Shared Statics
// =============================================================================

/// Version string for the Aether core library
pub(crate) static VERSION: &str = env!("CARGO_PKG_VERSION");

/// Flag indicating if core is initialized
pub(crate) static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Config path stored after initialization
pub(crate) static CONFIG_PATH: OnceLock<String> = OnceLock::new();

/// Check if the core is initialized
pub(crate) fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::SeqCst)
}

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
// Re-exports
// =============================================================================

// Callback types and registration
pub use callback::{
    aether_clear_callbacks, aether_register_complete_callback,
    aether_register_confirmation_callback, aether_register_error_callback,
    aether_register_memory_stored_callback, aether_register_state_callback,
    aether_register_stream_callback, aether_register_tool_callback, CompleteCallback,
    ConfirmationRequiredCallback, ErrorCallback, InitDownloadProgressCallback, InitErrorCallback,
    InitPhaseCompletedCallback, InitPhaseProgressCallback, InitPhaseStartedCallback,
    MemoryStoredCallback, StateChangeCallback, StreamTextCallback, ToolCallback, CALLBACKS,
};

// Initialization
pub use init::{aether_free, aether_init, aether_is_initialized, aether_version};

// Processing
pub use processing::{aether_cancel, aether_is_cancelled, aether_process};

// Configuration
pub use config::{
    aether_delete_provider, aether_get_default_provider, aether_load_config,
    aether_set_default_provider, aether_test_provider_connection, aether_update_provider,
};

// Memory
pub use memory::{aether_clear_memory, aether_get_memory_stats, aether_search_memory};

// Utility
pub use utility::{
    aether_free_string, aether_get_last_error, aether_get_log_directory, aether_get_root_commands,
    aether_list_tools, aether_reload_config, aether_set_log_level,
};

// MCP
pub use mcp::{
    aether_add_mcp_server, aether_delete_mcp_server, aether_export_mcp_config,
    aether_get_mcp_server_status, aether_import_mcp_config, aether_list_mcp_servers,
    aether_update_mcp_server,
};

// Skills
pub use skills::{
    aether_delete_skill, aether_get_skills_dir, aether_install_skill,
    aether_install_skills_from_zip, aether_list_skills, aether_refresh_skills,
};

// Generation
pub use generation::{
    aether_get_generation_provider_config, aether_list_generation_providers,
    aether_test_generation_provider, aether_update_generation_provider,
};

// Routing
pub use routing::{aether_get_routing_config, aether_update_routing_config};

// Behavior
pub use behavior::{aether_get_behavior_config, aether_update_behavior_config};

// Search
pub use search::{
    aether_get_search_provider_config, aether_list_search_providers, aether_test_search_provider,
    aether_update_search_provider,
};

// Agent/Cowork
pub use agent::{aether_get_agent_config, aether_get_policies, aether_update_agent_config};

// Runtime
pub use runtime::{
    aether_check_runtime_updates, aether_install_runtime, aether_is_runtime_installed,
    aether_list_runtimes, aether_set_runtime_auto_update, aether_update_runtime,
};

// First-time initialization
pub use first_time_init::{
    aether_check_embedding_model_exists, aether_clear_init_callbacks,
    aether_needs_first_time_init, aether_register_init_callbacks, aether_run_first_time_init,
};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};

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
                aether_process(
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0
                ),
                AETHER_ERR_NOT_INITIALIZED
            );
        }
    }
}
