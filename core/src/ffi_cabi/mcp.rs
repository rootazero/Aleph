//! MCP Server Management Functions
//!
//! List, add, update, delete, status, export/import for MCP servers.

use std::ffi::{c_char, c_int, CStr, CString};

use super::{
    is_initialized, AETHER_ERR_INVALID_ARG, AETHER_ERR_INVALID_UTF8, AETHER_ERR_NOT_INITIALIZED,
    AETHER_ERR_UNKNOWN, AETHER_SUCCESS,
};

// =============================================================================
// MCP Server Management Functions
// =============================================================================

/// List all MCP servers as JSON
///
/// # Arguments
/// * `out_json` - Pointer to receive servers list as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
///
/// # Safety
/// The caller must free the returned string using `aether_free_string`.
#[no_mangle]
pub unsafe extern "C" fn aether_list_mcp_servers(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Get actual MCP servers from core
    let servers = r#"{"servers":[]}"#;

    match CString::new(servers) {
        Ok(cstr) => {
            *out_len = servers.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Add an MCP server
///
/// # Arguments
/// * `config_json` - Server configuration as JSON (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_add_mcp_server(config_json: *const c_char) -> c_int {
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

    // TODO: Add actual MCP server
    tracing::info!("aether_add_mcp_server called with config: {}", config_str);
    AETHER_SUCCESS
}

/// Update an MCP server configuration
///
/// # Arguments
/// * `config_json` - Server configuration as JSON (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_update_mcp_server(config_json: *const c_char) -> c_int {
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

    // TODO: Update actual MCP server
    tracing::info!(
        "aether_update_mcp_server called with config: {}",
        config_str
    );
    AETHER_SUCCESS
}

/// Delete an MCP server
///
/// # Arguments
/// * `server_id` - Server ID (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_delete_mcp_server(server_id: *const c_char) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if server_id.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let id_str = match CStr::from_ptr(server_id).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Delete actual MCP server
    tracing::info!("aether_delete_mcp_server called with id: {}", id_str);
    AETHER_SUCCESS
}

/// Get MCP server status
///
/// # Arguments
/// * `server_id` - Server ID (UTF-8 encoded, null-terminated)
/// * `out_json` - Pointer to receive status as JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_get_mcp_server_status(
    server_id: *const c_char,
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if server_id.is_null() || out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let _id_str = match CStr::from_ptr(server_id).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Get actual MCP server status
    let status = r#"{"status":"stopped","message":"Server is not running"}"#;

    match CString::new(status) {
        Ok(cstr) => {
            *out_len = status.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Export MCP configuration as claude_desktop_config.json format
///
/// # Arguments
/// * `out_json` - Pointer to receive JSON
/// * `out_len` - Pointer to receive JSON length
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_export_mcp_config(
    out_json: *mut *mut c_char,
    out_len: *mut usize,
) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if out_json.is_null() || out_len.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    // TODO: Export actual MCP config
    let config = r#"{"mcpServers":{}}"#;

    match CString::new(config) {
        Ok(cstr) => {
            *out_len = config.len();
            *out_json = cstr.into_raw();
            AETHER_SUCCESS
        }
        Err(_) => AETHER_ERR_UNKNOWN,
    }
}

/// Import MCP configuration from claude_desktop_config.json format
///
/// # Arguments
/// * `json` - JSON configuration (UTF-8 encoded, null-terminated)
///
/// # Returns
/// * `0` on success
/// * Error code on failure
#[no_mangle]
pub unsafe extern "C" fn aether_import_mcp_config(json: *const c_char) -> c_int {
    if !is_initialized() {
        return AETHER_ERR_NOT_INITIALIZED;
    }

    if json.is_null() {
        return AETHER_ERR_INVALID_ARG;
    }

    let json_str = match CStr::from_ptr(json).to_str() {
        Ok(s) => s,
        Err(_) => return AETHER_ERR_INVALID_UTF8,
    };

    // TODO: Import actual MCP config
    tracing::info!("aether_import_mcp_config called with: {}", json_str);
    AETHER_SUCCESS
}
