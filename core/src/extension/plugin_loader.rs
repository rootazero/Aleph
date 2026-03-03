//! Plugin Loader - Manages runtime loading of Node.js and WASM plugins
//!
//! Provides a unified interface to load plugins into their appropriate runtimes
//! based on PluginKind, and invoke tools/hooks on loaded plugins.
//!
//! # Architecture
//!
//! ```text
//! PluginLoader
//! ├── nodejs_runtime: Option<NodeJsRuntime>  (lazy initialized)
//! ├── wasm_runtime: Option<WasmRuntime>      (lazy initialized)
//! └── loaded_plugins: HashMap<String, PluginKind>
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::extension::{PluginLoader, PluginManifest, PluginRegistry};
//!
//! let mut loader = PluginLoader::new();
//! let mut registry = PluginRegistry::new();
//!
//! // Load a plugin
//! loader.load_plugin(&manifest, &mut registry)?;
//!
//! // Call a tool
//! let result = loader.call_tool("plugin-id", "handler", json!({"key": "value"}))?;
//!
//! // Execute a hook
//! let result = loader.execute_hook("plugin-id", "onEvent", json!({"event": "data"}))?;
//!
//! // Shutdown all runtimes
//! loader.shutdown();
//! ```

use std::collections::HashMap;
use tracing::{info, warn};

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::PluginManifest;
use crate::extension::registry::PluginRegistry;
use crate::extension::runtime::nodejs::{hook_def_to_registration, tool_def_to_registration};
use crate::extension::runtime::NodeJsRuntime;
use crate::extension::runtime::WasmRuntime;
use crate::extension::types::{DirectCommandResult, PluginKind};

/// Manages loading plugins into appropriate runtimes.
///
/// The PluginLoader provides:
/// - Lazy initialization of runtimes (Node.js, WASM)
/// - Unified interface for loading plugins based on their kind
/// - Tool and hook execution across different runtimes
/// - Graceful shutdown of all running plugins
///
/// # Plugin Kinds
///
/// - `NodeJs`: JavaScript/TypeScript plugins using Node.js subprocess
/// - `Wasm`: WebAssembly plugins using Extism
/// - `Static`: Static content plugins (handled by ComponentLoader, not this loader)
///
/// # Thread Safety and Write Lock Requirement
///
/// Methods like [`call_tool`] and [`execute_hook`] require mutable access (`&mut self`)
/// because Node.js IPC communication requires writing to stdin/stdout streams.
/// These operations are inherently sequential - interleaving writes from multiple
/// concurrent callers would corrupt the message framing and cause protocol errors.
///
/// When wrapping PluginLoader in an RwLock (as done in ExtensionManager), tool and
/// hook calls must acquire a **write lock** to ensure sequential access to the IPC streams.
///
/// This is a fundamental constraint of the Node.js subprocess communication model,
/// not a limitation of this implementation.
///
/// [`call_tool`]: #method.call_tool
/// [`execute_hook`]: #method.execute_hook
pub struct PluginLoader {
    /// Node.js runtime (lazy initialized)
    nodejs_runtime: Option<NodeJsRuntime>,

    /// WASM runtime (lazy initialized)
    wasm_runtime: Option<WasmRuntime>,

    /// Map of plugin_id -> runtime kind for fast lookup
    loaded_plugins: HashMap<String, PluginKind>,
}

impl PluginLoader {
    /// Create a new plugin loader.
    ///
    /// Runtimes are lazily initialized when first needed to avoid
    /// unnecessary resource allocation.
    pub fn new() -> Self {
        Self {
            nodejs_runtime: None,
            wasm_runtime: None,
            loaded_plugins: HashMap::new(),
        }
    }

    /// Check if any runtime is currently active.
    ///
    /// Returns `true` if Node.js or WASM runtime has been initialized.
    pub fn is_any_runtime_active(&self) -> bool {
        let nodejs_active = self.nodejs_runtime.is_some();
        nodejs_active || self.wasm_runtime.is_some()
    }

    /// Check if a specific plugin is loaded.
    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        self.loaded_plugins.contains_key(plugin_id)
    }

    /// Get list of loaded plugin IDs.
    pub fn loaded_plugin_ids(&self) -> Vec<&str> {
        self.loaded_plugins.keys().map(|s| s.as_str()).collect()
    }

    /// Get the kind of a loaded plugin.
    pub fn get_plugin_kind(&self, plugin_id: &str) -> Option<PluginKind> {
        self.loaded_plugins.get(plugin_id).copied()
    }

    /// Get number of loaded plugins.
    pub fn loaded_count(&self) -> usize {
        self.loaded_plugins.len()
    }

    /// Load a plugin based on its kind.
    ///
    /// This method:
    /// 1. Determines the plugin kind from the manifest
    /// 2. Initializes the appropriate runtime if needed
    /// 3. Loads the plugin into the runtime
    /// 4. Registers discovered tools and hooks with the registry
    ///
    /// # Arguments
    ///
    /// * `manifest` - The plugin manifest containing metadata and entry point
    /// * `registry` - The plugin registry to register tools/hooks with
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the plugin was loaded successfully
    /// * `Err(ExtensionError)` if loading failed
    ///
    /// # Static Plugins
    ///
    /// Static plugins (PluginKind::Static) are handled by the ComponentLoader,
    /// not this runtime loader. Calling this method with a static plugin
    /// returns `Ok(())` without any action.
    pub fn load_plugin(
        &mut self,
        manifest: &PluginManifest,
        registry: &mut PluginRegistry,
    ) -> ExtensionResult<()> {
        // Check if already loaded
        if self.is_loaded(&manifest.id) {
            warn!("Plugin {} is already loaded, skipping", manifest.id);
            return Ok(());
        }

        match manifest.kind {
            PluginKind::NodeJs => self.load_nodejs_plugin(manifest, registry),
            PluginKind::Wasm => self.load_wasm_plugin(manifest, registry),
            PluginKind::Static => {
                // Static plugins are handled by ComponentLoader, not runtime
                info!(
                    "Plugin {} is static, skipping runtime loading",
                    manifest.id
                );
                Ok(())
            }
        }
    }

    /// Load a Node.js plugin.
    fn load_nodejs_plugin(
        &mut self,
        manifest: &PluginManifest,
        registry: &mut PluginRegistry,
    ) -> ExtensionResult<()> {
        // Initialize runtime if needed (lazy initialization)
        if self.nodejs_runtime.is_none() {
            info!("Initializing Node.js runtime with embedded host");
            self.nodejs_runtime = Some(NodeJsRuntime::with_embedded_host("node"));
        }

        let runtime = self
            .nodejs_runtime
            .as_mut()
            .expect("nodejs runtime just initialized");

        // Load the plugin and get registrations
        let registrations = runtime.load_plugin(manifest)?;

        // Register tools
        for tool in &registrations.tools {
            let reg = tool_def_to_registration(tool, &manifest.id);
            registry.register_tool(reg);
            info!(
                "Registered tool '{}' from plugin '{}'",
                tool.name, manifest.id
            );
        }

        // Register hooks
        for hook in &registrations.hooks {
            if let Some(reg) = hook_def_to_registration(hook, &manifest.id) {
                registry.register_hook(reg);
                info!(
                    "Registered hook '{}' from plugin '{}'",
                    hook.event, manifest.id
                );
            } else {
                warn!(
                    "Unknown hook event '{}' from plugin '{}', skipping",
                    hook.event, manifest.id
                );
            }
        }

        // Track the loaded plugin
        self.loaded_plugins
            .insert(manifest.id.clone(), PluginKind::NodeJs);

        info!(
            "Loaded Node.js plugin '{}' with {} tools and {} hooks",
            manifest.id,
            registrations.tools.len(),
            registrations.hooks.len()
        );

        Ok(())
    }

    /// Load a WASM plugin.
    fn load_wasm_plugin(
        &mut self,
        manifest: &PluginManifest,
        _registry: &mut PluginRegistry,
    ) -> ExtensionResult<()> {
        // Initialize runtime if needed (lazy initialization)
        if self.wasm_runtime.is_none() {
            info!("Initializing WASM runtime");
            self.wasm_runtime = Some(WasmRuntime::new());
        }

        let runtime = self
            .wasm_runtime
            .as_mut()
            .expect("wasm runtime just initialized");

        // Load the plugin
        runtime.load_plugin(manifest)?;

        // Track the loaded plugin
        self.loaded_plugins
            .insert(manifest.id.clone(), PluginKind::Wasm);

        // TODO: WASM plugins register tools via different mechanism
        // For now, we just load the plugin without registering tools/hooks
        // This will be implemented when WasmRuntime supports tool discovery

        info!("Loaded WASM plugin '{}'", manifest.id);

        Ok(())
    }

    /// Unload a plugin from its runtime.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin to unload
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the plugin was unloaded successfully
    /// * `Err(ExtensionError::PluginNotFound)` if the plugin is not loaded
    pub fn unload_plugin(&mut self, plugin_id: &str) -> ExtensionResult<()> {
        let kind = self.loaded_plugins.remove(plugin_id);

        match kind {
            Some(PluginKind::NodeJs) => {
                if let Some(runtime) = &mut self.nodejs_runtime {
                    runtime.unload_plugin(plugin_id)?;
                    info!("Unloaded Node.js plugin '{}'", plugin_id);
                }
            }
            Some(PluginKind::Wasm) => {
                if let Some(runtime) = &mut self.wasm_runtime {
                    if !runtime.unload_plugin(plugin_id) {
                        return Err(ExtensionError::Runtime(format!(
                            "Failed to unload WASM plugin '{}'",
                            plugin_id
                        )));
                    }
                    info!("Unloaded WASM plugin '{}'", plugin_id);
                }
            }
            Some(PluginKind::Static) => {
                // Static plugins don't need runtime unloading
                info!("Static plugin '{}' removed from tracking", plugin_id);
            }
            None => {
                return Err(ExtensionError::PluginNotFound(plugin_id.to_string()));
            }
        }

        Ok(())
    }

    /// Call a tool handler on a loaded plugin.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin containing the tool
    /// * `handler` - The handler function name to call
    /// * `args` - The arguments to pass to the handler
    ///
    /// # Returns
    ///
    /// * `Ok(serde_json::Value)` - The result from the tool handler
    /// * `Err(ExtensionError)` - If the call failed
    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        let kind = self
            .loaded_plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        match kind {
            PluginKind::NodeJs => {
                let runtime = self.nodejs_runtime.as_mut().ok_or_else(|| {
                    ExtensionError::Runtime("Node.js runtime not initialized".to_string())
                })?;
                runtime.call_tool(plugin_id, handler, args)
            }
            PluginKind::Wasm => {
                let runtime = self.wasm_runtime.as_mut().ok_or_else(|| {
                    ExtensionError::Runtime("WASM runtime not initialized".to_string())
                })?;
                let input = crate::extension::runtime::WasmToolInput {
                    name: handler.to_string(),
                    arguments: args,
                };
                let output = runtime.call_tool(plugin_id, handler, input)?;
                if output.success {
                    Ok(output.result.unwrap_or(serde_json::Value::Null))
                } else {
                    Err(ExtensionError::Runtime(
                        output.error.unwrap_or_else(|| "Unknown WASM error".to_string()),
                    ))
                }
            }
            PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {:?} does not support tool calls",
                kind
            ))),
        }
    }

    /// Execute a hook handler on a loaded plugin.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin containing the hook
    /// * `handler` - The handler function name to call
    /// * `event_data` - The event data to pass to the handler
    ///
    /// # Returns
    ///
    /// * `Ok(serde_json::Value)` - The result from the hook handler
    /// * `Err(ExtensionError)` - If the execution failed
    pub fn execute_hook(
        &mut self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        let kind = self
            .loaded_plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        match kind {
            PluginKind::NodeJs => {
                let runtime = self.nodejs_runtime.as_mut().ok_or_else(|| {
                    ExtensionError::Runtime("Node.js runtime not initialized".to_string())
                })?;
                runtime.execute_hook(plugin_id, handler, event_data)
            }
            PluginKind::Wasm => {
                // WASM hooks not yet implemented
                Err(ExtensionError::Runtime(
                    "WASM hooks not yet implemented".to_string(),
                ))
            }
            PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {:?} does not support hooks",
                kind
            ))),
        }
    }

    /// Execute a direct command handler on a loaded plugin.
    ///
    /// Direct commands are user-triggered commands that execute immediately
    /// without LLM involvement (e.g., `/status`, `/clear`, `/version`).
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin containing the command handler
    /// * `handler` - The handler function name to call
    /// * `args` - The arguments to pass to the handler
    ///
    /// # Returns
    ///
    /// * `Ok(DirectCommandResult)` - The result from the command handler
    /// * `Err(ExtensionError)` - If the execution failed
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = loader.execute_command("my-plugin", "statusHandler", json!({}))?;
    /// println!("Output: {}", result.content);
    /// ```
    pub fn execute_command(
        &mut self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<DirectCommandResult> {
        let kind = self
            .loaded_plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        match kind {
            PluginKind::NodeJs => {
                let runtime = self.nodejs_runtime.as_mut().ok_or_else(|| {
                    ExtensionError::Runtime("Node.js runtime not initialized".to_string())
                })?;

                // Call the handler and convert result to DirectCommandResult
                let result = runtime.call_tool(plugin_id, handler, args)?;
                Ok(serde_json::from_value(result).unwrap_or_else(|_| {
                    DirectCommandResult::success("Command executed")
                }))
            }
            PluginKind::Wasm => {
                let runtime = self.wasm_runtime.as_mut().ok_or_else(|| {
                    ExtensionError::Runtime("WASM runtime not initialized".to_string())
                })?;

                let input = crate::extension::runtime::WasmToolInput {
                    name: handler.to_string(),
                    arguments: args,
                };
                let output = runtime.call_tool(plugin_id, handler, input)?;

                if output.success {
                    // Try to parse result as DirectCommandResult, or create success response
                    let result = output.result.unwrap_or(serde_json::Value::Null);
                    Ok(serde_json::from_value(result).unwrap_or_else(|_| {
                        DirectCommandResult::success("Command executed")
                    }))
                } else {
                    Ok(DirectCommandResult::error(
                        output.error.unwrap_or_else(|| "Unknown WASM error".to_string()),
                    ))
                }
            }
            PluginKind::Static => Err(ExtensionError::Runtime(
                "Static plugins cannot have direct commands".to_string(),
            )),
        }
    }

    /// Shutdown all runtimes and unload all plugins.
    ///
    /// This method should be called when the application is shutting down
    /// to ensure all plugin processes are properly terminated.
    pub fn shutdown(&mut self) {
        info!("Shutting down PluginLoader with {} plugins", self.loaded_plugins.len());

        // Shutdown Node.js runtime
        if let Some(runtime) = &mut self.nodejs_runtime {
            runtime.shutdown_all();
        }

        // WASM runtime cleanup: WasmRuntime uses Extism under the hood, which cleans up
        // automatically when the runtime is dropped. No explicit shutdown needed.
        // The runtime will be dropped when PluginLoader is dropped.

        // Clear loaded plugins
        self.loaded_plugins.clear();

        info!("PluginLoader shutdown complete");
    }

    /// Check if Node.js runtime is initialized.
    pub fn is_nodejs_runtime_active(&self) -> bool {
        self.nodejs_runtime.is_some()
    }

    /// Check if WASM runtime is initialized.
    pub fn is_wasm_runtime_active(&self) -> bool {
        self.wasm_runtime.is_some()
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PluginLoader {
    fn drop(&mut self) {
        // Shutdown is called automatically when the loader is dropped
        // to ensure all plugin processes are properly terminated
        if self.is_any_runtime_active() || !self.loaded_plugins.is_empty() {
            self.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_loader_new() {
        let loader = PluginLoader::new();
        assert!(!loader.is_any_runtime_active());
        assert!(loader.loaded_plugin_ids().is_empty());
        assert_eq!(loader.loaded_count(), 0);
    }

    #[test]
    fn test_plugin_loader_is_loaded() {
        let loader = PluginLoader::new();
        assert!(!loader.is_loaded("nonexistent"));
    }

    #[test]
    fn test_plugin_loader_default() {
        let loader = PluginLoader::default();
        assert!(!loader.is_any_runtime_active());
        assert!(!loader.is_nodejs_runtime_active());
        assert!(!loader.is_wasm_runtime_active());
    }

    #[test]
    fn test_plugin_loader_get_plugin_kind() {
        let loader = PluginLoader::new();
        assert!(loader.get_plugin_kind("nonexistent").is_none());
    }

    #[test]
    fn test_plugin_loader_unload_nonexistent() {
        let mut loader = PluginLoader::new();
        let result = loader.unload_plugin("nonexistent");
        assert!(result.is_err());
        match result {
            Err(ExtensionError::PluginNotFound(id)) => assert_eq!(id, "nonexistent"),
            _ => panic!("Expected PluginNotFound error"),
        }
    }

    #[test]
    fn test_plugin_loader_call_tool_nonexistent() {
        let mut loader = PluginLoader::new();
        let result = loader.call_tool("nonexistent", "handler", serde_json::json!({}));
        assert!(result.is_err());
        match result {
            Err(ExtensionError::PluginNotFound(id)) => assert_eq!(id, "nonexistent"),
            _ => panic!("Expected PluginNotFound error"),
        }
    }

    #[test]
    fn test_plugin_loader_execute_hook_nonexistent() {
        let mut loader = PluginLoader::new();
        let result = loader.execute_hook("nonexistent", "handler", serde_json::json!({}));
        assert!(result.is_err());
        match result {
            Err(ExtensionError::PluginNotFound(id)) => assert_eq!(id, "nonexistent"),
            _ => panic!("Expected PluginNotFound error"),
        }
    }

    #[test]
    fn test_plugin_loader_execute_command_nonexistent() {
        let mut loader = PluginLoader::new();
        let result = loader.execute_command("nonexistent", "handler", serde_json::json!({}));
        assert!(result.is_err());
        match result {
            Err(ExtensionError::PluginNotFound(id)) => assert_eq!(id, "nonexistent"),
            _ => panic!("Expected PluginNotFound error"),
        }
    }

    #[test]
    fn test_plugin_loader_shutdown_empty() {
        let mut loader = PluginLoader::new();
        // Should not panic on empty loader
        loader.shutdown();
        assert!(loader.loaded_plugin_ids().is_empty());
    }

    #[test]
    fn test_plugin_loader_loaded_plugin_ids() {
        let loader = PluginLoader::new();
        let ids = loader.loaded_plugin_ids();
        assert!(ids.is_empty());
    }
}
