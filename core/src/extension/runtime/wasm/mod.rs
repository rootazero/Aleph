//! WASM Plugin Runtime using Extism
//!
//! Provides sandboxed execution of WASM plugins with permission-based
//! access to host functions.

mod allowlist;
mod capabilities;
mod capability_kernel;
mod credential_injector;
mod host_functions;
mod limits;
mod permissions;

pub use allowlist::{AllowlistError, AllowlistValidator};
pub use capabilities::{
    host_matches_pattern, CredentialBinding, CredentialInject, EndpointPattern, HttpCapability,
    RateLimit, SecretsCapability, ToolInvokeCapability, WasmCapabilities, WorkspaceCapability,
};
pub use capability_kernel::{CapabilityError, WasmCapabilityKernel};
pub use credential_injector::{inject_credential, CredentialError};
pub use limits::WasmResourceLimits;
pub use permissions::PermissionChecker;

#[cfg(feature = "plugin-wasm")]
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
#[cfg(feature = "plugin-wasm")]
use tracing::{debug, info};

use crate::extension::error::ExtensionError;
use crate::extension::manifest::PluginManifest;

#[cfg(feature = "plugin-wasm")]
use crate::sync_primitives::Arc;

#[cfg(feature = "plugin-wasm")]
use extism::{Manifest as ExtismManifest, PluginBuilder, UserData, Wasm, PTR};

/// Input for WASM tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolInput {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Output from WASM tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmToolOutput {
    pub success: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// WASM plugin runtime manager
#[derive(Default)]
pub struct WasmRuntime {
    #[cfg(feature = "plugin-wasm")]
    plugins: HashMap<String, LoadedWasmPlugin>,
    #[cfg(not(feature = "plugin-wasm"))]
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(feature = "plugin-wasm")]
struct LoadedWasmPlugin {
    plugin: extism::Plugin,
    #[allow(dead_code)]
    manifest: PluginManifest,
    #[allow(dead_code)]
    kernel: Arc<WasmCapabilityKernel>,
}

impl WasmRuntime {
    /// Create a new WASM runtime
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a WASM plugin
    #[cfg(feature = "plugin-wasm")]
    pub fn load_plugin(&mut self, manifest: &PluginManifest) -> Result<(), ExtensionError> {
        let wasm_path = manifest.entry_path();

        if !wasm_path.exists() {
            return Err(ExtensionError::Runtime(format!(
                "WASM file not found: {:?}",
                wasm_path
            )));
        }

        info!("Loading WASM plugin: {} from {:?}", manifest.id, wasm_path);

        // Parse capabilities from manifest (default = zero permissions)
        let capabilities = manifest.wasm_capabilities.clone().unwrap_or_default();
        let limits = manifest.wasm_resource_limits.clone().unwrap_or_default();

        // Create per-plugin capability kernel
        let kernel = Arc::new(WasmCapabilityKernel::new(
            manifest.id.clone(),
            capabilities,
            limits,
        ));

        // Create host state for Extism UserData
        let host_state = UserData::new(host_functions::HostState {
            kernel: kernel.clone(),
            workspace_root: manifest.root_dir.clone(),
        });

        let extism_manifest = ExtismManifest::new([Wasm::file(&wasm_path)]);

        let plugin = PluginBuilder::new(extism_manifest)
            .with_wasi(true)
            .with_function(
                "log",
                [PTR, PTR],
                [],
                host_state.clone(),
                host_functions::host_log,
            )
            .with_function(
                "now_millis",
                [],
                [PTR],
                host_state.clone(),
                host_functions::host_now_millis,
            )
            .with_function(
                "workspace_read",
                [PTR],
                [PTR],
                host_state.clone(),
                host_functions::host_workspace_read,
            )
            .with_function(
                "secret_exists",
                [PTR],
                [PTR],
                host_state,
                host_functions::host_secret_exists,
            )
            .build()
            .map_err(|e| ExtensionError::Runtime(format!("Failed to load WASM: {}", e)))?;

        let loaded = LoadedWasmPlugin {
            plugin,
            manifest: manifest.clone(),
            kernel,
        };

        self.plugins.insert(manifest.id.clone(), loaded);

        Ok(())
    }

    #[cfg(not(feature = "plugin-wasm"))]
    pub fn load_plugin(&mut self, _manifest: &PluginManifest) -> Result<(), ExtensionError> {
        Err(ExtensionError::Runtime(
            "WASM runtime not enabled. Compile with --features plugin-wasm".to_string(),
        ))
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, plugin_id: &str) -> bool {
        #[cfg(feature = "plugin-wasm")]
        {
            self.plugins.remove(plugin_id).is_some()
        }
        #[cfg(not(feature = "plugin-wasm"))]
        {
            let _ = plugin_id;
            false
        }
    }

    /// Check if a plugin is loaded
    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        #[cfg(feature = "plugin-wasm")]
        {
            self.plugins.contains_key(plugin_id)
        }
        #[cfg(not(feature = "plugin-wasm"))]
        {
            let _ = plugin_id;
            false
        }
    }

    /// Call a tool handler in a WASM plugin
    #[cfg(feature = "plugin-wasm")]
    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        handler: &str,
        input: WasmToolInput,
    ) -> Result<WasmToolOutput, ExtensionError> {
        let loaded = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        let input_json = serde_json::to_string(&input)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to serialize input: {}", e)))?;

        debug!(
            "Calling WASM handler '{}' with input: {}",
            handler, input_json
        );

        let result = loaded
            .plugin
            .call::<&str, &str>(handler, &input_json)
            .map_err(|e| ExtensionError::Runtime(format!("WASM call failed: {}", e)))?;

        let output: WasmToolOutput = serde_json::from_str(result)
            .map_err(|e| ExtensionError::Runtime(format!("Failed to parse output: {}", e)))?;

        Ok(output)
    }

    #[cfg(not(feature = "plugin-wasm"))]
    pub fn call_tool(
        &mut self,
        _plugin_id: &str,
        _handler: &str,
        _input: WasmToolInput,
    ) -> Result<WasmToolOutput, ExtensionError> {
        Err(ExtensionError::Runtime(
            "WASM runtime not enabled".to_string(),
        ))
    }

    /// Get list of loaded plugin IDs
    pub fn loaded_plugins(&self) -> Vec<String> {
        #[cfg(feature = "plugin-wasm")]
        {
            self.plugins.keys().cloned().collect()
        }
        #[cfg(not(feature = "plugin-wasm"))]
        {
            Vec::new()
        }
    }
}

#[cfg(all(test, feature = "plugin-wasm"))]
mod tests {
    use super::*;
    use crate::extension::types::PluginKind;
    use std::path::PathBuf;

    #[test]
    fn test_wasm_runtime_not_found() {
        let mut runtime = WasmRuntime::new();
        let manifest = PluginManifest::new(
            "test".to_string(),
            "Test".to_string(),
            PluginKind::Wasm,
            PathBuf::from("nonexistent.wasm"),
        );

        let result = runtime.load_plugin(&manifest);
        assert!(result.is_err());
    }

    #[test]
    fn test_wasm_runtime_call_not_loaded() {
        let mut runtime = WasmRuntime::new();
        let input = WasmToolInput {
            name: "test".to_string(),
            arguments: serde_json::json!({}),
        };

        let result = runtime.call_tool("nonexistent", "handler", input);
        assert!(result.is_err());
    }
}

#[cfg(all(test, not(feature = "plugin-wasm")))]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_runtime_disabled() {
        let runtime = WasmRuntime::new();
        assert!(runtime.loaded_plugins().is_empty());
    }

    #[test]
    fn test_wasm_runtime_is_loaded_returns_false() {
        let runtime = WasmRuntime::new();
        assert!(!runtime.is_loaded("any-plugin"));
    }

    #[test]
    fn test_wasm_runtime_unload_returns_false() {
        let mut runtime = WasmRuntime::new();
        assert!(!runtime.unload_plugin("any-plugin"));
    }
}
