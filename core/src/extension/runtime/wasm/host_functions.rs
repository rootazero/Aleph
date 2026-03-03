//! Extism host function registrations for WASM plugins.
//!
//! Registers host functions that are injected into the WASM sandbox:
//! - log(level, message) — controlled logging
//! - now_millis() -> u64 — current timestamp
//! - workspace_read(path) -> JSON string — read workspace files
//! - secret_exists(name) -> "true"/"false" — check secret availability

use crate::sync_primitives::Arc;

use extism::host_fn;

use super::capability_kernel::WasmCapabilityKernel;

/// Shared state passed to all host functions via Extism UserData
pub struct HostState {
    pub kernel: Arc<WasmCapabilityKernel>,
    pub workspace_root: std::path::PathBuf,
}

host_fn!(pub host_log(state: HostState; level: String, message: String) {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    let _ = state.kernel.log(&level, &message);
    Ok(())
});

host_fn!(pub host_now_millis(state: HostState;) -> u64 {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    Ok(state.kernel.now_millis())
});

host_fn!(pub host_workspace_read(state: HostState; path: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());

    // Check capability
    if let Err(e) = state.kernel.check_workspace_read(&path) {
        return Ok(serde_json::json!({"error": e.to_string()}).to_string());
    }

    // Read file from workspace
    let full_path = state.workspace_root.join(&path);
    match std::fs::read_to_string(&full_path) {
        Ok(content) => Ok(serde_json::json!({"content": content}).to_string()),
        Err(e) => Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    }
});

host_fn!(pub host_secret_exists(state: HostState; name: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    let exists = state.kernel.check_secret_pattern(&name);
    Ok(exists.to_string())
});
