//! Extism host function registrations for WASM plugins.
//!
//! Registers host functions that are injected into the WASM sandbox:
//! - log(level, message) — controlled logging
//! - now_millis() -> u64 — current timestamp
//! - workspace_read(path) -> JSON string — read workspace files
//! - secret_exists(name) -> "true"/"false" — check secret availability

#[cfg(feature = "plugin-wasm")]
use crate::sync_primitives::Arc;

#[cfg(feature = "plugin-wasm")]
use extism::host_fn;

#[cfg(feature = "plugin-wasm")]
use super::capability_kernel::WasmCapabilityKernel;

/// Shared state passed to all host functions via Extism UserData
#[cfg(feature = "plugin-wasm")]
pub struct HostState {
    pub kernel: Arc<WasmCapabilityKernel>,
    pub workspace_root: std::path::PathBuf,
}

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_log(state: HostState; level: String, message: String) {
    let state = state.get()?;
    let state = state.lock().unwrap();
    let _ = state.kernel.log(&level, &message);
    Ok(())
});

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_now_millis(state: HostState;) -> u64 {
    let state = state.get()?;
    let state = state.lock().unwrap();
    Ok(state.kernel.now_millis())
});

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_workspace_read(state: HostState; path: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap();

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

#[cfg(feature = "plugin-wasm")]
host_fn!(pub host_secret_exists(state: HostState; name: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap();
    let exists = state.kernel.check_secret_pattern(&name);
    Ok(exists.to_string())
});
