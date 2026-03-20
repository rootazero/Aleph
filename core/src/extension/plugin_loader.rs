//! Plugin Loader - Manages runtime loading of Node.js, WASM, and MCP plugins
//!
//! Provides a unified interface to load plugins into their appropriate runtimes
//! based on PluginKind, and invoke tools/hooks on loaded plugins.
//!
//! # Architecture
//!
//! ```text
//! PluginLoader
//! ├── nodejs_runtime: Option<NodeJsRuntime>  (lazy initialized, DEPRECATED)
//! ├── wasm_runtime: Option<WasmRuntime>      (lazy initialized)
//! ├── mcp_configs: HashMap<String, HashMap<String, McpManagerConfig>>
//! └── loaded_plugins: HashMap<String, PluginKind>
//! ```
//!
//! # MCP Plugin Flow
//!
//! MCP-type plugins delegate to Aleph's MCP client system (McpManager).
//! The PluginLoader reads `.mcp.json`, stores the configs, and exposes them
//! via `get_mcp_configs()`. The caller (ExtensionManager or Gateway) is
//! responsible for registering these configs with `McpManagerHandle::add_server()`.
//!
//! Tool calls for MCP plugins are routed through the MCP system automatically —
//! MCP tools appear in the unified tool registry when the MCP server starts.
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
//! // For MCP plugins, retrieve configs for McpManager registration
//! if let Some(configs) = loader.get_mcp_configs("plugin-id") {
//!     for (server_id, config) in configs {
//!         mcp_handle.add_server(config.clone()).await?;
//!     }
//! }
//!
//! // Call a tool (Node.js/WASM only — MCP tools go through MCP system)
//! let result = loader.call_tool("plugin-id", "handler", json!({"key": "value"}))?;
//!
//! // Shutdown all runtimes
//! loader.shutdown();
//! ```

use std::collections::HashMap;
use tracing::{info, warn};

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::PluginManifest;
use crate::extension::mcp_config;
use crate::extension::registry::PluginRegistry;
use crate::extension::runtime::nodejs::{hook_def_to_registration, tool_def_to_registration};
use crate::extension::runtime::NodeJsRuntime;
use crate::extension::runtime::WasmRuntime;
use crate::extension::types::{DirectCommandResult, PluginKind};
use crate::mcp::McpManagerConfig;

/// Manages loading plugins into appropriate runtimes.
///
/// The PluginLoader provides:
/// - Lazy initialization of runtimes (Node.js, WASM)
/// - MCP config reading and storage for MCP-type plugins
/// - Unified interface for loading plugins based on their kind
/// - Tool and hook execution across different runtimes
/// - Graceful shutdown of all running plugins
///
/// # Plugin Kinds
///
/// - `NodeJs`: JavaScript/TypeScript plugins using Node.js subprocess (DEPRECATED — use MCP)
/// - `Wasm`: WebAssembly plugins using Extism
/// - `Mcp`: MCP server plugins — tools registered via Aleph's MCP client system
/// - `Static`: Static content plugins (handled by ContentLoader, not this loader)
///
/// # Thread Safety and Write Lock Requirement
///
/// Methods like [`call_tool`] and [`execute_hook`] require mutable access (`&mut self`)
/// because Node.js IPC communication requires writing to stdin/stdout streams.
/// MCP-type plugins do NOT need mutable access (tool calls go through McpManager),
/// but this method signature is kept for interface uniformity.
///
/// [`call_tool`]: #method.call_tool
/// [`execute_hook`]: #method.execute_hook
pub struct PluginLoader {
    /// Node.js runtime (lazy initialized)
    ///
    /// DEPRECATED: New plugins should use `PluginKind::Mcp` instead.
    /// Node.js IPC will be removed in a future version (P5).
    nodejs_runtime: Option<NodeJsRuntime>,

    /// WASM runtime (lazy initialized)
    wasm_runtime: Option<WasmRuntime>,

    /// MCP server configs per plugin: plugin_id → (server_id → McpManagerConfig)
    ///
    /// These configs are read from `.mcp.json` during `load_plugin()` and
    /// should be registered with McpManagerHandle by the caller.
    mcp_configs: HashMap<String, HashMap<String, McpManagerConfig>>,

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
            mcp_configs: HashMap::new(),
            loaded_plugins: HashMap::new(),
        }
    }

    /// Check if any runtime is currently active.
    ///
    /// Returns `true` if Node.js or WASM runtime has been initialized,
    /// or if any MCP plugins are loaded.
    pub fn is_any_runtime_active(&self) -> bool {
        self.nodejs_runtime.is_some()
            || self.wasm_runtime.is_some()
            || !self.mcp_configs.is_empty()
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

    // ===== MCP Config Access =====

    /// Get MCP server configs for a specific plugin.
    ///
    /// Returns `None` if the plugin is not an MCP plugin or has no servers.
    /// The caller is responsible for registering these with `McpManagerHandle::add_server()`.
    pub fn get_mcp_configs(&self, plugin_id: &str) -> Option<&HashMap<String, McpManagerConfig>> {
        self.mcp_configs.get(plugin_id)
    }

    /// Get all MCP server configs as a flat HashMap (server_id → config).
    ///
    /// Useful for bulk registration with McpManager.
    pub fn all_mcp_configs_map(&self) -> HashMap<String, McpManagerConfig> {
        let mut result = HashMap::new();
        for servers in self.mcp_configs.values() {
            for (id, config) in servers {
                result.insert(id.clone(), config.clone());
            }
        }
        result
    }

    // ===== Loading =====

    /// Load a plugin based on its kind.
    ///
    /// This method:
    /// 1. Determines the plugin kind from the manifest
    /// 2. Initializes the appropriate runtime if needed
    /// 3. Loads the plugin into the runtime
    /// 4. Registers discovered tools and hooks with the registry
    ///
    /// For MCP-type plugins, this reads `.mcp.json` and stores the configs.
    /// The caller must subsequently register them with `McpManagerHandle`.
    ///
    /// # Arguments
    ///
    /// * `manifest` - The plugin manifest containing metadata and entry point
    /// * `registry` - The plugin registry to register tools/hooks with
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
            PluginKind::Mcp => self.load_mcp_plugin(manifest),
            PluginKind::Static => {
                // Static plugins are handled by ContentLoader, not runtime
                info!(
                    "Plugin {} is static, skipping runtime loading",
                    manifest.id
                );
                Ok(())
            }
        }
    }

    /// Load a Node.js plugin.
    ///
    /// DEPRECATED: New plugins should use `PluginKind::Mcp` with `.mcp.json`.
    /// Node.js IPC runtime will be removed in P5.
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

    /// Load an MCP-type plugin.
    ///
    /// Reads `.mcp.json` from the plugin directory, substitutes environment
    /// variables, and stores the resulting `McpManagerConfig`s. The configs
    /// are NOT automatically registered with the MCP system — the caller
    /// must do so via `McpManagerHandle::add_server()`.
    ///
    /// MCP tools are automatically available in the unified tool registry
    /// once the MCP server is started by the McpManager.
    fn load_mcp_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
        let configs = mcp_config::read_mcp_json(&manifest.root_dir, &manifest.id)?;

        if configs.is_empty() {
            warn!(
                "MCP plugin '{}' has no servers defined in .mcp.json",
                manifest.id
            );
        }

        let server_count = configs.len();
        let server_names: Vec<String> = configs.keys().cloned().collect();

        // Store configs for later retrieval by caller
        self.mcp_configs.insert(manifest.id.clone(), configs);

        // Track the loaded plugin
        self.loaded_plugins
            .insert(manifest.id.clone(), PluginKind::Mcp);

        info!(
            "Loaded MCP plugin '{}' with {} server(s): {:?}",
            manifest.id, server_count, server_names
        );

        Ok(())
    }

    // ===== Unloading =====

    /// Unload a plugin from its runtime.
    ///
    /// For MCP plugins, this removes the stored configs. The caller is
    /// responsible for calling `McpManagerHandle::remove_server()` for
    /// each server that was registered.
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
            Some(PluginKind::Mcp) => {
                let removed = self.mcp_configs.remove(plugin_id);
                let server_count = removed.as_ref().map_or(0, |m| m.len());
                info!(
                    "Unloaded MCP plugin '{}' ({} server configs removed). \
                     Caller must remove servers from McpManager.",
                    plugin_id, server_count
                );
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

    // ===== Tool / Hook / Command Execution =====

    /// Call a tool handler on a loaded plugin.
    ///
    /// For MCP plugins, tool calls should go through the MCP system (McpManager),
    /// not this method. This method returns an error for MCP plugins.
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
            PluginKind::Mcp => Err(ExtensionError::Runtime(
                "MCP plugin tool calls must go through McpManager, not PluginLoader".to_string(),
            )),
            PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {:?} does not support tool calls",
                kind
            ))),
        }
    }

    /// Execute a hook handler on a loaded plugin.
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
            PluginKind::Mcp => Err(ExtensionError::Runtime(
                "MCP plugins do not support hooks via PluginLoader".to_string(),
            )),
            PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {:?} does not support hooks",
                kind
            ))),
        }
    }

    /// Execute a direct command handler on a loaded plugin.
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
            PluginKind::Mcp => Err(ExtensionError::Runtime(
                "MCP plugins do not support direct commands via PluginLoader".to_string(),
            )),
            PluginKind::Static => Err(ExtensionError::Runtime(
                "Static plugins cannot have direct commands".to_string(),
            )),
        }
    }

    /// Shutdown all runtimes and unload all plugins.
    ///
    /// This method should be called when the application is shutting down
    /// to ensure all plugin processes are properly terminated.
    ///
    /// Note: MCP servers are managed by McpManager and must be shut down
    /// separately via `McpManagerHandle::shutdown()`.
    pub fn shutdown(&mut self) {
        info!("Shutting down PluginLoader with {} plugins", self.loaded_plugins.len());

        // Shutdown Node.js runtime
        if let Some(runtime) = &mut self.nodejs_runtime {
            runtime.shutdown_all();
        }

        // WASM runtime cleanup: WasmRuntime uses Extism under the hood, which cleans up
        // automatically when the runtime is dropped. No explicit shutdown needed.
        // The runtime will be dropped when PluginLoader is dropped.

        // Clear MCP configs (servers are managed by McpManager)
        self.mcp_configs.clear();

        // Clear loaded plugins
        self.loaded_plugins.clear();

        info!("PluginLoader shutdown complete");
    }

    /// Check if Node.js runtime is initialized.
    ///
    /// DEPRECATED: Node.js IPC runtime will be removed in P5.
    pub fn is_nodejs_runtime_active(&self) -> bool {
        self.nodejs_runtime.is_some()
    }

    /// Check if WASM runtime is initialized.
    pub fn is_wasm_runtime_active(&self) -> bool {
        self.wasm_runtime.is_some()
    }

    /// Check if any MCP plugins are loaded.
    pub fn has_mcp_plugins(&self) -> bool {
        !self.mcp_configs.is_empty()
    }

    /// Get the number of MCP servers across all loaded MCP plugins.
    pub fn mcp_server_count(&self) -> usize {
        self.mcp_configs.values().map(|m| m.len()).sum()
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
        assert!(!loader.has_mcp_plugins());
        assert_eq!(loader.mcp_server_count(), 0);
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
        assert!(!loader.has_mcp_plugins());
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

    #[test]
    fn test_plugin_loader_mcp_configs_empty() {
        let loader = PluginLoader::new();
        assert!(loader.get_mcp_configs("nonexistent").is_none());
        assert!(loader.all_mcp_configs_map().is_empty());
    }

    #[test]
    fn test_plugin_loader_mcp_tool_call_rejected() {
        let mut loader = PluginLoader::new();
        // Manually insert an MCP plugin to test routing
        loader
            .loaded_plugins
            .insert("mcp-test".to_string(), PluginKind::Mcp);

        let result = loader.call_tool("mcp-test", "handler", serde_json::json!({}));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("McpManager")
        );
    }

    #[test]
    fn test_plugin_loader_mcp_hook_rejected() {
        let mut loader = PluginLoader::new();
        loader
            .loaded_plugins
            .insert("mcp-test".to_string(), PluginKind::Mcp);

        let result = loader.execute_hook("mcp-test", "hook", serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_plugin_loader_mcp_command_rejected() {
        let mut loader = PluginLoader::new();
        loader
            .loaded_plugins
            .insert("mcp-test".to_string(), PluginKind::Mcp);

        let result = loader.execute_command("mcp-test", "cmd", serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_plugin_loader_unload_mcp_plugin() {
        let mut loader = PluginLoader::new();
        loader
            .loaded_plugins
            .insert("mcp-test".to_string(), PluginKind::Mcp);
        loader.mcp_configs.insert(
            "mcp-test".to_string(),
            {
                let mut m = HashMap::new();
                m.insert(
                    "plugin:mcp-test/srv".to_string(),
                    McpManagerConfig::stdio("plugin:mcp-test/srv", "srv", "echo"),
                );
                m
            },
        );

        assert!(loader.has_mcp_plugins());
        assert_eq!(loader.mcp_server_count(), 1);

        loader.unload_plugin("mcp-test").unwrap();

        assert!(!loader.is_loaded("mcp-test"));
        assert!(loader.get_mcp_configs("mcp-test").is_none());
        assert!(!loader.has_mcp_plugins());
    }
}
