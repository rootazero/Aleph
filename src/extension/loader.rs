//! Plugin Loader - Manages runtime loading of WASM plugins
//!
//! Provides a unified interface to load plugins into their appropriate runtimes
//! based on `PluginKind`, and invoke tools/hooks on loaded plugins.
//!
//! # Architecture
//!
//! ```text
//! PluginLoader
//! ├── wasm_runtime: Option<WasmRuntime>      (lazy initialized)
//! └── loaded_plugins: HashMap<String, PluginKind>
//! ```
//!
//! MCP plugins have no in-process runtime here; `lifecycle.rs` hands their
//! `.mcp.json` servers to `McpManager`.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::extension::effects::{async_disposer, Disposer};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::PluginManifest;
use crate::extension::runtime::WasmRuntime;
use crate::extension::types::{DirectCommandResult, PluginKind};
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
            loaded_plugins: HashMap::new(),
        }
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

    /// Load a plugin's in-process runtime. Only `Wasm` has one; `Mcp`
    /// plugins are MCP *servers* and are mounted by `lifecycle.rs` through
    /// `McpManager` (`mcp_server` step), `Static` plugins have none.
    pub(super) fn load_plugin(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
        if self.is_loaded(&manifest.id) {
            warn!("Plugin {} is already loaded, skipping", manifest.id);
            return Ok(());
        }
        match manifest.kind {
            PluginKind::Wasm => self.load_wasm_plugin(manifest),
            PluginKind::Mcp | PluginKind::Static => {
                info!(
                    "Plugin {} has no in-process runtime, skipping loader",
                    manifest.id
                );
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
            Some(PluginKind::Static) | Some(PluginKind::Mcp) => {
                info!("Plugin '{}' removed from tracking", plugin_id);
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
    /// Only `Wasm` plugins have a callable in-process runtime; any other
    /// kind is an error (MCP plugin tools are reached through `McpManager`).
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
            PluginKind::Mcp | PluginKind::Static => Err(ExtensionError::Runtime(format!(
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
            PluginKind::Mcp | PluginKind::Static => Err(ExtensionError::Runtime(format!(
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
            PluginKind::Mcp | PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {kind:?} does not support direct commands"
            ))),
        }
    }

    /// Shutdown all runtimes and unload all plugins.
    pub fn shutdown(&mut self) {
        info!(
            "Shutting down PluginLoader with {} plugins",
            self.loaded_plugins.len()
        );

        // WASM runtime cleanup happens automatically when dropped.
        self.loaded_plugins.clear();

        info!("PluginLoader shutdown complete");
    }

    /// Check if WASM runtime is initialized.
    #[must_use]
    pub const fn is_wasm_runtime_active(&self) -> bool {
        self.wasm_runtime.is_some()
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Mount-time runtime effect for a `PluginKind::Wasm` plugin: instantiate the
/// module now and hand back the disposer that unloads it.
///
/// Loading at mount (not lazily at first tool call, as the loader used to)
/// is what lets the `service` step that follows run the plugin's
/// `start_handler`, and what puts the module inside the scope's dispose order
/// instead of beside it.
pub(crate) async fn load_wasm_effect(
    loader: Arc<tokio::sync::RwLock<PluginLoader>>,
    manifest: &PluginManifest,
) -> ExtensionResult<Disposer> {
    debug_assert_eq!(
        manifest.kind,
        PluginKind::Wasm,
        "only WASM plugins have a module to load"
    );
    loader.write().await.load_plugin(manifest)?;
    let plugin_id = manifest.id.clone();
    Ok(async_disposer(move || async move {
        loader
            .write()
            .await
            .unload_plugin(&plugin_id)
            .map_err(|e| e.to_string())
    }))
}

/// The smallest valid WebAssembly module (magic + version, no sections).
/// extism links it without complaint and fails only when an export is
/// called, which is exactly the shape a lifecycle fixture needs.
#[cfg(test)]
pub(crate) const EMPTY_WASM_MODULE: &[u8] = b"\0asm\x01\x00\x00\x00";

/// The `memory_extension` effect: register a `McpMemoryExtension` when
/// `manifest` declares a `[memory]` section, bound to the live MCP manager
/// when one is attached, and return the disposer that unregisters it.
///
/// `Ok(None)` when there is no `[memory]` section — nothing to register,
/// nothing to dispose. `Err` when the name is already taken: two plugins
/// claiming one extension name would double-fire every hook, so the second
/// mount fails at this step instead of logging and carrying on.
///
/// Free function (not a `PluginLoader` method) because it never touches the
/// loader; it lives here so the G1 census finds every plugin-facing memory
/// registration in one file.
pub(crate) fn register_memory_extension_effect(
    manifest: &PluginManifest,
    server_id: Option<String>,
    registry: &Arc<MemoryExtensionRegistry>,
    mcp_handle: Option<crate::mcp::McpManagerHandle>,
) -> Result<Option<Disposer>, String> {
    if manifest.memory_manifest.is_none() {
        return Ok(None);
    }
    let ext = Arc::new(McpMemoryExtension::new_unbound(manifest.name.clone()));
    if let (Some(handle), Some(sid)) = (mcp_handle, server_id) {
        ext.rebind(Arc::new(
            crate::memory::extensions::ManagerBackedMcpCaller::new(handle, sid),
        ));
    }
    let name = manifest.name.clone();
    registry
        .register_mcp(Arc::clone(&ext))
        .map_err(|e| format!("memory extension '{name}' not registered: {e}"))?;
    info!(plugin = %name, "registered McpMemoryExtension for plugin with [memory] section");
    let registry = Arc::clone(registry);
    Ok(Some(crate::extension::effects::sync_disposer(move || {
        if registry.unregister(&name) {
            Ok(())
        } else {
            Err(format!(
                "memory extension '{name}' was not registered at dispose time"
            ))
        }
    })))
}

impl Drop for PluginLoader {
    fn drop(&mut self) {
        if self.is_wasm_runtime_active() || !self.loaded_plugins.is_empty() {
            self.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::extensions::manifest::{MemoryHook, MemoryManifestSection};
    use crate::memory::extensions::traits::MemoryExtension;
    use std::path::PathBuf;

    #[test]
    fn test_plugin_loader_new() {
        let loader = PluginLoader::new();
        assert!(!loader.is_wasm_runtime_active());
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

    #[tokio::test]
    async fn memory_extension_effect_registers_and_its_disposer_unregisters() {
        let manifest = make_manifest_with_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let disposer = register_memory_extension_effect(
            &manifest,
            Some("plugin:test/srv".to_string()),
            &registry,
            None,
        )
        .expect("fresh name registers")
        .expect("[memory] section present → an effect");
        assert_eq!(registry.len(), 1);
        let snap = registry.mcp_bindings_snapshot();
        assert_eq!(
            snap[0].name(),
            "Test Memory Plugin",
            "keyed by manifest.name"
        );
        // No MCP handle was attached, so the step leaves the placeholder
        // caller in place: a call answers with the "not yet bound" diagnostic
        // instead of reaching a manager.
        let err = snap[0]
            .call_for_test("noop", serde_json::json!({}))
            .await
            .expect_err("placeholder caller rejects every call");
        assert!(err.to_string().contains("not yet bound"), "{err}");

        disposer().await.unwrap();
        assert_eq!(registry.len(), 0);
        assert!(registry.mcp_bindings_snapshot().is_empty());
    }

    #[test]
    fn memory_extension_effect_is_none_without_a_memory_section() {
        let manifest = make_manifest_no_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let effect = register_memory_extension_effect(&manifest, None, &registry, None).unwrap();
        assert!(effect.is_none());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn memory_extension_effect_refuses_a_duplicate_name_loudly() {
        let manifest = make_manifest_with_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let _first = register_memory_extension_effect(&manifest, None, &registry, None)
            .unwrap()
            .unwrap();
        let err = register_memory_extension_effect(&manifest, None, &registry, None)
            .err()
            .expect(
                "a second plugin claiming the same extension name is a mount failure, not a warn",
            );
        assert!(err.contains("Test Memory Plugin"), "{err}");
        assert_eq!(registry.len(), 1, "the loser registered nothing");
    }

    /// With a live MCP handle the extension is bound at registration — no
    /// separate boot-time rebind pass over the registry is needed — and it is
    /// bound to the server id the step was given: the extension records no
    /// server id of its own, so the only place to read the binding target is
    /// the caller's own error, which names the server it tried to reach.
    #[tokio::test]
    async fn memory_extension_effect_binds_the_caller_when_a_handle_is_present() {
        let dir = tempfile::tempdir().unwrap();
        let (actor, handle) =
            crate::mcp::manager::McpManagerActor::new(Some(dir.path().join("mcp.json")))
                .await
                .unwrap();
        tokio::spawn(actor.run());
        let manifest = make_manifest_with_memory();
        let registry = Arc::new(MemoryExtensionRegistry::new());
        let _d = register_memory_extension_effect(
            &manifest,
            Some("plugin:test/srv".to_string()),
            &registry,
            Some(handle),
        )
        .unwrap()
        .unwrap();
        let ext = &registry.mcp_bindings_snapshot()[0];
        // `UnboundMcpCaller` answers every call with its diagnostic error;
        // a bound caller reaches the manager and gets "server not running",
        // naming the server it was bound to.
        let err = ext
            .call_for_test("noop", serde_json::json!({}))
            .await
            .expect_err("no such server on a fresh manager");
        assert!(
            !err.to_string().contains("not yet bound"),
            "must be bound: {err}"
        );
        assert!(
            err.to_string().contains("'plugin:test/srv' not running"),
            "must be bound to the server id the step was given: {err}"
        );
    }

    /// An MCP manifest never enters `loaded_plugins`: its servers are mounted
    /// through `McpManager`, so the loader has nothing to call a tool on.
    #[test]
    fn test_plugin_loader_mcp_tool_call_is_not_loaded_into_the_loader() {
        let mut loader = PluginLoader::new();
        let manifest = make_manifest_no_memory();
        loader.load_plugin(&manifest).unwrap();
        assert!(!loader.is_loaded(&manifest.id));

        let err = loader
            .call_tool(&manifest.id, "handler", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, ExtensionError::PluginNotFound(_)), "{err}");
    }

    #[test]
    fn test_plugin_loader_mcp_hook_is_not_loaded_into_the_loader() {
        let mut loader = PluginLoader::new();
        let manifest = make_manifest_no_memory();
        loader.load_plugin(&manifest).unwrap();
        assert!(!loader.is_loaded(&manifest.id));

        let err = loader
            .execute_hook(&manifest.id, "hook", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, ExtensionError::PluginNotFound(_)), "{err}");
    }

    #[test]
    fn test_plugin_loader_mcp_command_is_not_loaded_into_the_loader() {
        let mut loader = PluginLoader::new();
        let manifest = make_manifest_no_memory();
        loader.load_plugin(&manifest).unwrap();
        assert!(!loader.is_loaded(&manifest.id));

        let err = loader
            .execute_command(&manifest.id, "cmd", serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, ExtensionError::PluginNotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn load_wasm_effect_loads_the_module_and_its_disposer_unloads_it() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("plugin.wasm"), EMPTY_WASM_MODULE).unwrap();
        let mut manifest = PluginManifest::new(
            "qa-wasm".to_string(),
            "QA WASM".to_string(),
            PluginKind::Wasm,
            PathBuf::from("plugin.wasm"),
        );
        manifest.root_dir = tmp.path().to_path_buf();

        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));
        let disposer = load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .expect("an empty module links");
        assert!(loader.read().await.is_loaded("qa-wasm"));
        assert!(loader.read().await.is_wasm_runtime_active());

        disposer()
            .await
            .expect("unload of a loaded module succeeds");
        assert!(!loader.read().await.is_loaded("qa-wasm"));
    }

    #[tokio::test]
    async fn load_wasm_effect_with_a_missing_file_registers_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut manifest = PluginManifest::new(
            "qa-missing".to_string(),
            "QA Missing".to_string(),
            PluginKind::Wasm,
            PathBuf::from("nope.wasm"),
        );
        manifest.root_dir = tmp.path().to_path_buf();
        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));
        let err = load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .err()
            .expect("missing file is an error, not a silent skip");
        assert!(err.to_string().contains("WASM file not found"), "{err}");
        assert!(!loader.read().await.is_loaded("qa-missing"));
    }
}
