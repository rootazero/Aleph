//! Plugin Loader - Manages runtime loading of WASM and MCP plugins
//!
//! Provides a unified interface to load plugins into their appropriate runtimes
//! based on `PluginKind`, and invoke tools/hooks on loaded plugins.
//!
//! # Architecture
//!
//! ```text
//! PluginLoader
//! ├── wasm_runtime: Option<WasmRuntime>      (lazy initialized)
//! ├── mcp_configs: HashMap<String, HashMap<String, McpManagerConfig>>
//! └── loaded_plugins: HashMap<String, PluginKind>
//! ```
//!
//! # MCP Plugin Flow
//!
//! MCP-type plugins delegate to Aleph's MCP client system (`McpManager`).
//! The `PluginLoader` reads `.mcp.json`, stores the configs, and exposes them
//! via `get_mcp_configs()`. The caller (`ExtensionManager` or Gateway) is
//! responsible for registering these configs with `McpManager`.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::PluginManifest;
use crate::extension::mcp_config;
use crate::extension::runtime::WasmRuntime;
use crate::extension::types::{DirectCommandResult, PluginKind};
use crate::mcp::McpManagerConfig;
use crate::memory::extensions::{McpMemoryExtension, MemoryExtensionRegistry};

/// Manages loading plugins into appropriate runtimes.
pub struct PluginLoader {
    /// The operator's stored configuration per plugin, mirrored from
    /// `plugins.toml`.
    ///
    /// A field rather than a parameter on the four `load_*` signatures: a new
    /// load path then inherits configuration instead of having to be told
    /// about it, which is the shape that let `${CLAUDE_PLUGIN_ROOT}` go
    /// unexpanded across five manifest adapters. Empty means "nothing
    /// configured", which is what a plugin with no `config_schema` should see.
    plugin_settings: std::collections::HashMap<String, serde_json::Value>,
    /// WASM runtime (lazy initialized)
    wasm_runtime: Option<WasmRuntime>,

    /// MCP server configs per plugin: `plugin_id` -> (`server_id` -> `McpManagerConfig`)
    mcp_configs: HashMap<String, HashMap<String, McpManagerConfig>>,

    /// Map of `plugin_id` -> runtime kind for fast lookup
    loaded_plugins: HashMap<String, PluginKind>,
}

impl PluginLoader {
    /// Create a new plugin loader.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugin_settings: std::collections::HashMap::new(),
            wasm_runtime: None,
            mcp_configs: HashMap::new(),
            loaded_plugins: HashMap::new(),
        }
    }

    /// Check if any runtime is currently active.
    #[must_use]
    pub fn is_any_runtime_active(&self) -> bool {
        self.wasm_runtime.is_some() || !self.mcp_configs.is_empty()
    }

    /// Check if a specific plugin is loaded.
    #[must_use]
    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        self.loaded_plugins.contains_key(plugin_id)
    }

    /// Get list of loaded plugin IDs.
    #[must_use]
    pub fn loaded_plugin_ids(&self) -> Vec<&str> {
        self.loaded_plugins.keys().map(|s| s.as_str()).collect()
    }

    /// Get the kind of a loaded plugin.
    #[must_use]
    pub fn get_plugin_kind(&self, plugin_id: &str) -> Option<PluginKind> {
        self.loaded_plugins.get(plugin_id).copied()
    }

    /// Get number of loaded plugins.
    #[must_use]
    pub fn loaded_count(&self) -> usize {
        self.loaded_plugins.len()
    }

    // ===== MCP Config Access =====

    /// Get MCP server configs for a specific plugin.
    #[must_use]
    pub fn get_mcp_configs(&self, plugin_id: &str) -> Option<&HashMap<String, McpManagerConfig>> {
        self.mcp_configs.get(plugin_id)
    }

    /// Get all MCP server configs as a flat `HashMap` (`server_id` -> config).
    ///
    /// If two plugins register MCP servers with the same ID, the later one wins
    /// and a warning is logged.
    pub fn all_mcp_configs_map(&self) -> HashMap<String, McpManagerConfig> {
        let mut result = HashMap::new();
        for (plugin_id, servers) in &self.mcp_configs {
            for (id, config) in servers {
                if let Some(_existing) = result.insert(id.clone(), config.clone()) {
                    warn!(
                        "MCP server ID '{}' from plugin '{}' overwrites a server with the same ID from another plugin",
                        id, plugin_id
                    );
                }
            }
        }
        result
    }

    // ===== Loading =====

    /// Mirror the operator's stored plugin configuration into the loader.
    ///
    /// Called once when the manager reads `plugins.toml` and again on every
    /// config write, so a plugin loaded (or reloaded) afterwards sees the
    /// current values. A plugin already running keeps the configuration it was
    /// started with until it is reloaded — reloading tears down MCP servers
    /// and background services, which is not a side effect a config write may
    /// smuggle in.
    pub fn set_all_plugin_settings(
        &mut self,
        settings: std::collections::HashMap<String, serde_json::Value>,
    ) {
        self.plugin_settings = settings;
    }

    /// Load a plugin based on its kind.
    pub fn load_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
        if self.is_loaded(&manifest.id) {
            warn!("Plugin {} is already loaded, skipping", manifest.id);
            return Ok(());
        }

        match manifest.kind {
            PluginKind::Wasm => self.load_wasm_plugin(manifest),
            PluginKind::Mcp => self.load_mcp_plugin(manifest),
            PluginKind::Static => {
                info!("Plugin {} is static, skipping runtime loading", manifest.id);
                Ok(())
            }
        }
    }

    /// Load a WASM plugin.
    fn load_wasm_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
        if self.wasm_runtime.is_none() {
            info!("Initializing WASM runtime");
            self.wasm_runtime = Some(WasmRuntime::new());
        }

        let runtime = self
            .wasm_runtime
            .as_mut()
            .ok_or_else(|| ExtensionError::Runtime("WASM runtime not initialized".to_string()))?;

        let settings = self
            .plugin_settings
            .get(&manifest.id)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        // The wire that made host-side credential injection reachable. This
        // argument was `None` from the day the injector was written, so every
        // `[capabilities.http.credentials]` binding resolved to `secret not
        // found` — a fully implemented security feature with no way to reach it.
        //
        // Reads the process-global vault rather than a handle threaded through
        // `PluginLoader::new`: see `VaultBackedSecretResolver::for_current_process`
        // for why a constructor parameter is the wrong shape for this.
        let resolver = super::runtime::wasm::VaultBackedSecretResolver::for_current_process();
        runtime.load_plugin_with(manifest, Some(resolver), &settings)?;

        self.loaded_plugins
            .insert(manifest.id.clone(), PluginKind::Wasm);

        info!("Loaded WASM plugin '{}'", manifest.id);
        Ok(())
    }

    /// Load an MCP-type plugin.
    fn load_mcp_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
        let settings = self
            .plugin_settings
            .get(&manifest.id)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let configs = mcp_config::read_mcp_json(&manifest.root_dir, &manifest.id, &settings)?;

        if configs.is_empty() {
            warn!(
                "MCP plugin '{}' has no servers defined in .mcp.json",
                manifest.id
            );
        }

        let server_count = configs.len();
        let mut server_names: Vec<String> = configs.keys().cloned().collect();
        server_names.sort();

        self.mcp_configs.insert(manifest.id.clone(), configs);
        self.loaded_plugins
            .insert(manifest.id.clone(), PluginKind::Mcp);

        info!(
            "Loaded MCP plugin '{}' with {} server(s): {:?}",
            manifest.id, server_count, server_names
        );

        Ok(())
    }

    /// Load a plugin and, if the manifest declares a `[memory]` section,
    /// register a `McpMemoryExtension` backed by an `UnboundMcpCaller` into
    /// `memory_registry`.
    ///
    /// This is the preferred entry point when the `MemoryExtensionRegistry` is
    /// available (threaded through the server context at startup). Existing
    /// callers that don't yet have the registry can keep using `load_plugin`
    /// unchanged.
    pub fn load_plugin_with_memory(
        &mut self,
        manifest: &PluginManifest,
        memory_registry: &Arc<MemoryExtensionRegistry>,
    ) -> ExtensionResult<()> {
        self.load_plugin(manifest)?;
        // Resolve the plugin's MCP server id (memory hooks route there). A
        // memory plugin is expected to declare exactly one server; if it
        // declares several, use the first and warn.
        let server_id = self.mcp_configs.get(&manifest.id).and_then(|servers| {
            let mut keys: Vec<String> = servers.keys().cloned().collect();
            keys.sort();
            if keys.len() > 1 {
                warn!(
                    plugin = %manifest.id,
                    "plugin declares >1 MCP server; routing [memory] hooks to the first"
                );
            }
            keys.into_iter().next()
        });
        register_memory_extension_if_declared(manifest, server_id, memory_registry);
        Ok(())
    }

    // ===== Unloading =====

    /// Unload a plugin from its runtime.
    pub fn unload_plugin(&mut self, plugin_id: &str) -> ExtensionResult<()> {
        let kind = self.loaded_plugins.remove(plugin_id);

        match kind {
            Some(PluginKind::Wasm) => {
                if let Some(runtime) = &mut self.wasm_runtime {
                    if !runtime.unload_plugin(plugin_id) {
                        self.loaded_plugins
                            .insert(plugin_id.to_string(), PluginKind::Wasm);
                        return Err(ExtensionError::Runtime(format!(
                            "Failed to unload WASM plugin '{plugin_id}'"
                        )));
                    }
                    info!("Unloaded WASM plugin '{}'", plugin_id);
                } else {
                    self.loaded_plugins
                        .insert(plugin_id.to_string(), PluginKind::Wasm);
                    return Err(ExtensionError::Runtime(
                        "WASM runtime not initialized".to_string(),
                    ));
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
    /// For MCP plugins, tool calls should go through the MCP system (`McpManager`).
    pub fn call_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        let kind = self
            .loaded_plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        match kind {
            PluginKind::Wasm => {
                let runtime = self.wasm_runtime.as_ref().ok_or_else(|| {
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
                        output
                            .error
                            .unwrap_or_else(|| "Unknown WASM error".to_string()),
                    ))
                }
            }
            PluginKind::Mcp => Err(ExtensionError::Runtime(
                "MCP plugin tool calls must go through McpManager, not PluginLoader".to_string(),
            )),
            PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {kind:?} does not support tool calls"
            ))),
        }
    }

    /// Execute a hook handler on a loaded plugin.
    ///
    /// A WASM hook is an exported function invoked with the event payload, so
    /// it routes through the same runtime path as tool/command calls — no
    /// separate hook ABI is needed.
    pub fn execute_hook(
        &self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        let kind = self
            .loaded_plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        match kind {
            PluginKind::Wasm => {
                let runtime = self.wasm_runtime.as_ref().ok_or_else(|| {
                    ExtensionError::Runtime("WASM runtime not initialized".to_string())
                })?;
                let input = crate::extension::runtime::WasmToolInput {
                    name: handler.to_string(),
                    arguments: event_data,
                };
                let output = runtime.call_tool(plugin_id, handler, input)?;
                if output.success {
                    Ok(output.result.unwrap_or(serde_json::Value::Null))
                } else {
                    Err(ExtensionError::Runtime(
                        output
                            .error
                            .unwrap_or_else(|| "Unknown WASM error".to_string()),
                    ))
                }
            }
            PluginKind::Mcp => Err(ExtensionError::Runtime(
                "MCP plugins do not support hooks via PluginLoader".to_string(),
            )),
            PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {kind:?} does not support hooks"
            ))),
        }
    }

    /// Execute a direct command handler on a loaded plugin.
    pub fn execute_command(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<DirectCommandResult> {
        let kind = self
            .loaded_plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        match kind {
            PluginKind::Wasm => {
                let runtime = self.wasm_runtime.as_ref().ok_or_else(|| {
                    ExtensionError::Runtime("WASM runtime not initialized".to_string())
                })?;

                let input = crate::extension::runtime::WasmToolInput {
                    name: handler.to_string(),
                    arguments: args,
                };
                let output = runtime.call_tool(plugin_id, handler, input)?;

                if output.success {
                    let result = output.result.unwrap_or(serde_json::Value::Null);
                    Ok(serde_json::from_value(result)
                        .unwrap_or_else(|_| DirectCommandResult::success("Command executed")))
                } else {
                    Ok(DirectCommandResult::error(
                        output
                            .error
                            .unwrap_or_else(|| "Unknown WASM error".to_string()),
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
    pub fn shutdown(&mut self) {
        info!(
            "Shutting down PluginLoader with {} plugins",
            self.loaded_plugins.len()
        );

        // WASM runtime cleanup happens automatically when dropped.
        self.mcp_configs.clear();
        self.loaded_plugins.clear();

        info!("PluginLoader shutdown complete");
    }

    /// Check if WASM runtime is initialized.
    #[must_use]
    pub const fn is_wasm_runtime_active(&self) -> bool {
        self.wasm_runtime.is_some()
    }

    /// Check if any MCP plugins are loaded.
    #[must_use]
    pub fn has_mcp_plugins(&self) -> bool {
        !self.mcp_configs.is_empty()
    }

    /// Get the number of MCP servers across all loaded MCP plugins.
    #[must_use]
    pub fn mcp_server_count(&self) -> usize {
        self.mcp_configs.values().map(|m| m.len()).sum()
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Register a `McpMemoryExtension` into `registry` when `manifest` declares a
/// `[memory]` section.
///
/// The extension is initially backed by `UnboundMcpCaller`, which returns a
/// clear diagnostic error on every call. `ExtensionManager::bind_memory_callers`
/// replaces the caller with a real `McpManager` binding at server startup.
///
/// This is extracted as a free function so it can be unit-tested directly
/// without constructing a full `PluginLoader`.
pub(crate) fn register_memory_extension_if_declared(
    manifest: &PluginManifest,
    server_id: Option<String>,
    registry: &Arc<MemoryExtensionRegistry>,
) {
    if manifest.memory_manifest.is_some() {
        let ext = McpMemoryExtension::new_unbound(manifest.name.clone(), server_id);
        if let Err(e) = registry.register_mcp(Arc::new(ext)) {
            warn!(
                plugin = %manifest.name,
                error = %e,
                "failed to register McpMemoryExtension (duplicate or invalid plugin name)"
            );
            return;
        }
        info!(
            plugin = %manifest.name,
            "registered McpMemoryExtension (unbound) for plugin with [memory] section"
        );
    }
}

impl Drop for PluginLoader {
    fn drop(&mut self) {
        if self.is_any_runtime_active() || !self.loaded_plugins.is_empty() {
            self.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::extensions::manifest::{MemoryHook, MemoryManifestSection};
    use std::path::PathBuf;

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
        let loader = PluginLoader::new();
        let result = loader.call_tool("nonexistent", "handler", serde_json::json!({}));
        assert!(result.is_err());
        match result {
            Err(ExtensionError::PluginNotFound(id)) => assert_eq!(id, "nonexistent"),
            _ => panic!("Expected PluginNotFound error"),
        }
    }

    #[test]
    fn test_plugin_loader_execute_hook_nonexistent() {
        let loader = PluginLoader::new();
        let result = loader.execute_hook("nonexistent", "handler", serde_json::json!({}));
        assert!(result.is_err());
        match result {
            Err(ExtensionError::PluginNotFound(id)) => assert_eq!(id, "nonexistent"),
            _ => panic!("Expected PluginNotFound error"),
        }
    }

    #[test]
    fn test_plugin_loader_execute_command_nonexistent() {
        let loader = PluginLoader::new();
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

    // ===== Memory extension registration helpers =====

    fn make_manifest_with_memory() -> PluginManifest {
        let mut m = PluginManifest::new(
            "test-mem-plugin".to_string(),
            "Test Memory Plugin".to_string(),
            PluginKind::Mcp,
            PathBuf::from("index.js"),
        );
        m.memory_manifest = Some(MemoryManifestSection {
            hooks: vec![MemoryHook::OnRetrieve],
            priority: 100,
            produce_interval_seconds: None,
        });
        m
    }

    fn make_manifest_no_memory() -> PluginManifest {
        PluginManifest::new(
            "test-plain-plugin".to_string(),
            "Test Plain Plugin".to_string(),
            PluginKind::Mcp,
            PathBuf::from("index.js"),
        )
    }

    #[test]
    fn register_memory_extension_registers_when_memory_section_present() {
        let manifest = make_manifest_with_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());

        assert_eq!(registry.len(), 0);
        register_memory_extension_if_declared(
            &manifest,
            Some("plugin:test/srv".to_string()),
            &registry,
        );
        assert_eq!(registry.len(), 1, "should have registered one extension");
    }

    #[test]
    fn register_memory_extension_skips_when_no_memory_section() {
        let manifest = make_manifest_no_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());

        register_memory_extension_if_declared(
            &manifest,
            Some("plugin:test/srv".to_string()),
            &registry,
        );
        assert_eq!(
            registry.len(),
            0,
            "no extension should be registered without [memory] section"
        );
    }

    #[test]
    fn test_plugin_loader_mcp_tool_call_rejected() {
        let mut loader = PluginLoader::new();
        loader
            .loaded_plugins
            .insert("mcp-test".to_string(), PluginKind::Mcp);

        let result = loader.call_tool("mcp-test", "handler", serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("McpManager"));
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
        loader.mcp_configs.insert("mcp-test".to_string(), {
            let mut m = HashMap::new();
            m.insert(
                "plugin:mcp-test/srv".to_string(),
                McpManagerConfig::stdio("plugin:mcp-test/srv", "srv", "echo"),
            );
            m
        });

        assert!(loader.has_mcp_plugins());
        assert_eq!(loader.mcp_server_count(), 1);

        loader.unload_plugin("mcp-test").unwrap();

        assert!(!loader.is_loaded("mcp-test"));
        assert!(loader.get_mcp_configs("mcp-test").is_none());
        assert!(!loader.has_mcp_plugins());
    }
}
