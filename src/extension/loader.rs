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

#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[cfg(test)]
#[derive(Clone, Default, Debug)]
pub(crate) struct WasmLoadProbe {
    guest_byte_reads: Arc<AtomicUsize>,
    guest_bytes: Arc<AtomicUsize>,
    guest_instance_created: Arc<AtomicUsize>,
    guest_instance_dropped: Arc<AtomicUsize>,
    next_instance_id: Arc<AtomicU64>,
    dropped_instance_ids: Arc<std::sync::Mutex<Vec<u64>>>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct WasmInstanceReceipt {
    id: u64,
    dropped: Arc<AtomicBool>,
    probe: WasmLoadProbe,
}

#[cfg(test)]
impl WasmLoadProbe {
    pub(crate) fn record_guest_read(&self, bytes: usize) {
        self.guest_byte_reads.fetch_add(1, Ordering::Relaxed);
        self.guest_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn record_instance(&self) -> WasmInstanceReceipt {
        let id = self.next_instance_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.guest_instance_created.fetch_add(1, Ordering::Relaxed);
        WasmInstanceReceipt {
            id,
            dropped: Arc::new(AtomicBool::new(false)),
            probe: self.clone(),
        }
    }

    fn record_instance_drop(&self, id: u64, dropped: &AtomicBool) {
        if dropped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.guest_instance_dropped.fetch_add(1, Ordering::Relaxed);
        match self.dropped_instance_ids.lock() {
            Ok(mut ids) => ids.push(id),
            Err(error) => error.into_inner().push(id),
        }
    }

    pub(crate) fn guest_byte_reads(&self) -> usize {
        self.guest_byte_reads.load(Ordering::Acquire)
    }

    pub(crate) fn guest_bytes(&self) -> usize {
        self.guest_bytes.load(Ordering::Acquire)
    }

    pub(crate) fn guest_instance_created(&self) -> usize {
        self.guest_instance_created.load(Ordering::Acquire)
    }

    pub(crate) fn guest_instance_dropped(&self) -> usize {
        self.guest_instance_dropped.load(Ordering::Acquire)
    }

    pub(crate) fn dropped_instance_ids(&self) -> Vec<u64> {
        match self.dropped_instance_ids.lock() {
            Ok(ids) => ids.clone(),
            Err(error) => error.into_inner().clone(),
        }
    }
}

#[cfg(test)]
impl WasmInstanceReceipt {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn is_dropped(&self) -> bool {
        self.dropped.load(Ordering::Acquire)
    }

    pub(crate) fn mark_dropped(&self) {
        self.probe.record_instance_drop(self.id, &self.dropped);
    }
}

use crate::extension::effects::{async_disposer, Disposer};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::PluginManifest;
use crate::extension::runtime::WasmRuntime;
use crate::extension::types::PluginKind;
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

    #[cfg(test)]
    wasm_load_probe: WasmLoadProbe,

    /// Map of `plugin_id` -> runtime kind for fast lookup
    loaded_plugins: HashMap<String, PluginKind>,
    // Runtime exports only. Descriptors, owners and grants live in PluginRegistry.
    context_bindings: HashMap<String, crate::extension::runtime::wasm::WasmBinding>,
    context_call_slots: Arc<tokio::sync::Semaphore>,
}

impl PluginLoader {
    /// Create a new plugin loader.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugin_settings: std::collections::HashMap::new(),
            wasm_runtime: None,
            #[cfg(test)]
            wasm_load_probe: WasmLoadProbe::default(),
            loaded_plugins: HashMap::new(),
            context_bindings: HashMap::new(),
            context_call_slots: Arc::new(tokio::sync::Semaphore::new(4)),
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
            #[cfg(test)]
            {
                self.wasm_runtime = Some(WasmRuntime::with_probe(self.wasm_load_probe.clone()));
            }
            #[cfg(not(test))]
            {
                self.wasm_runtime = Some(WasmRuntime::new());
            }
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

        self.context_bindings
            .retain(|_, binding| binding.plugin_id() != plugin_id);
        Ok(())
    }

    /// Capture the opaque identity of the WASM instance this loader
    /// currently has mounted under `plugin_id`, or `None` when it has none.
    ///
    /// The identity names the actual `Arc` in the runtime's map — never a
    /// name, an export, or a permission string — so an exportless module
    /// still has one and a caller cannot fabricate one.
    pub(crate) fn wasm_mount_identity(
        &self,
        plugin_id: &str,
    ) -> Option<crate::extension::runtime::wasm::WasmMountIdentity> {
        match (
            self.loaded_plugins.get(plugin_id),
            self.wasm_runtime.as_ref(),
        ) {
            (Some(PluginKind::Wasm), Some(runtime)) => runtime.mounted_instance(plugin_id),
            _ => None,
        }
    }

    /// Remove the WASM mount under `plugin_id` only while both the runtime
    /// and the kind index still belong to the exact instance `identity` was
    /// captured from.
    ///
    /// The kind index is touched only after the runtime confirms the same
    /// instance is still mounted, so there is no interval where the index
    /// entry is removed before the match is known, and a stale identity
    /// (already unloaded, replaced, or dropped by shutdown) is a successful
    /// no-op that preserves whatever is mounted now.
    pub(crate) fn unload_wasm_mount(
        &mut self,
        plugin_id: &str,
        identity: &crate::extension::runtime::wasm::WasmMountIdentity,
    ) {
        let Some(runtime) = self.wasm_runtime.as_mut() else {
            return;
        };
        if !runtime.remove_if_current(plugin_id, identity) {
            return;
        }
        if self.loaded_plugins.get(plugin_id) == Some(&PluginKind::Wasm) {
            self.loaded_plugins.remove(plugin_id);
        }
        self.context_bindings
            .retain(|_, binding| binding.plugin_id() != plugin_id);
    }

    pub(crate) fn context_call_slots(&self) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(&self.context_call_slots)
    }

    pub(crate) fn context_binding(
        &self,
        key: &str,
    ) -> Option<crate::extension::runtime::wasm::WasmBinding> {
        self.context_bindings
            .get(key)
            .filter(|binding| self.wasm_binding_is_current(binding))
            .cloned()
    }

    /// Validated declarations on the actual mounted instance; never re-read disk.
    pub(crate) fn context_declarations(
        &self,
        plugin_id: &str,
    ) -> ExtensionResult<Vec<crate::extension::capability::ContextDeclaration>> {
        if !self.is_loaded(plugin_id) {
            return Err(ExtensionError::PluginNotFound(plugin_id.into()));
        }
        let mut declarations = Vec::new();
        for (key, binding) in &self.context_bindings {
            if binding.plugin_id() != plugin_id {
                continue;
            }
            if !binding.context_effect_free() || !self.wasm_binding_is_current(binding) {
                return Err(ExtensionError::Runtime(
                    "context binding is unavailable".into(),
                ));
            }
            let declaration =
                binding
                    .context_declaration(key.strip_prefix(&format!("{plugin_id}:")).ok_or_else(
                        || ExtensionError::Runtime("invalid context binding key".into()),
                    )?)
                    .ok_or_else(|| {
                        ExtensionError::Runtime("mounted context declaration missing".into())
                    })?;
            declaration
                .descriptor
                .validate()
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            declarations.push(declaration.clone());
        }
        declarations.sort_by(|left, right| left.descriptor.id.cmp(&right.descriptor.id));
        Ok(declarations)
    }

    pub(crate) fn wasm_binding_is_current(
        &self,
        binding: &crate::extension::runtime::wasm::WasmBinding,
    ) -> bool {
        self.loaded_plugins.get(binding.plugin_id()) == Some(&PluginKind::Wasm)
            && self
                .wasm_runtime
                .as_ref()
                .is_some_and(|runtime| runtime.binding_is_current(binding))
    }

    /// Called only by the normal mount effect. Does not register any grant.
    fn bind_context_exports(&mut self, manifest: &PluginManifest) -> ExtensionResult<()> {
        let mut bindings = Vec::new();
        for declaration in manifest
            .aleph_extensions
            .iter()
            .flat_map(|ext| &ext.contexts)
        {
            let adapter =
                super::context_adapter::WasmContextAdapter::bind(self, manifest, declaration)?;
            let key =
                super::registry::namespaced_component_key(&manifest.id, &declaration.descriptor.id);
            if bindings.iter().any(|(existing, _)| existing == &key) {
                return Err(ExtensionError::Runtime(format!(
                    "duplicate context binding: {key}"
                )));
            }
            bindings.push((key, adapter.binding().clone()));
        }
        self.context_bindings.extend(bindings);
        Ok(())
    }

    // ===== Tool / Hook / Command Execution =====

    /// Bind an export on the currently mounted WASM plugin instance.
    pub(crate) fn bind_wasm(
        &self,
        plugin_id: &str,
        export: &str,
    ) -> ExtensionResult<crate::extension::runtime::wasm::WasmBinding> {
        let kind = self
            .loaded_plugins
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;
        match kind {
            PluginKind::Wasm => {
                let runtime = self.wasm_runtime.as_ref().ok_or_else(|| {
                    ExtensionError::Runtime("WASM runtime not initialized".to_string())
                })?;
                runtime.bind(plugin_id, export)
            }
            PluginKind::Mcp | PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {kind:?} does not support WASM bindings"
            ))),
        }
    }

    /// Call a previously bound export without accepting a second identity.
    pub(crate) fn call_bound(
        &self,
        binding: &crate::extension::runtime::wasm::WasmBinding,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        let plugin_id = binding.plugin_id();
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
                    name: binding.export().to_string(),
                    arguments: args,
                };
                let output = runtime.call_bound(binding, input)?;
                Self::map_wasm_output(output)
            }
            PluginKind::Mcp | PluginKind::Static => Err(ExtensionError::Runtime(format!(
                "Plugin kind {kind:?} does not support tool calls"
            ))),
        }
    }

    fn map_wasm_output(
        output: crate::extension::runtime::WasmToolOutput,
    ) -> ExtensionResult<serde_json::Value> {
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
                Self::map_wasm_output(output)
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
        let binding = self.bind_wasm_hook(plugin_id, handler)?;
        self.call_bound(&binding, event_data)
    }

    /// Bind only a hook declared on the exact mounted instance. This carries
    /// no canonical/owner authority; the manager adds its owner permit/fences.
    pub(crate) fn bind_wasm_hook(
        &self,
        plugin_id: &str,
        handler: &str,
    ) -> ExtensionResult<crate::extension::runtime::wasm::WasmBinding> {
        match self.loaded_plugins.get(plugin_id) {
            Some(PluginKind::Wasm) => self
                .wasm_runtime
                .as_ref()
                .ok_or_else(|| ExtensionError::Runtime("WASM runtime not initialized".into()))?
                .bind_hook(plugin_id, handler),
            Some(kind) => Err(ExtensionError::Runtime(format!(
                "Plugin kind {kind:?} does not support hooks"
            ))),
            None => Err(ExtensionError::PluginNotFound(plugin_id.into())),
        }
    }

    /// Shutdown all runtimes and unload all plugins.
    pub fn shutdown(&mut self) {
        info!(
            "Shutting down PluginLoader with {} plugins",
            self.loaded_plugins.len()
        );

        // Drop the mounted runtime so retained bindings cannot execute through it.
        self.wasm_runtime = None;
        self.context_bindings.clear();
        self.loaded_plugins.clear();

        info!("PluginLoader shutdown complete");
    }

    #[cfg(test)]
    pub(crate) fn wasm_load_probe(&self) -> WasmLoadProbe {
        self.wasm_load_probe.clone()
    }

    #[cfg(test)]
    pub(crate) fn wasm_instance_receipt(
        &self,
        plugin_id: &str,
    ) -> Option<crate::extension::loader::WasmInstanceReceipt> {
        self.wasm_runtime
            .as_ref()
            .and_then(|runtime| runtime.instance_receipt(plugin_id))
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
///
/// The effect owns only the instance this call actually mounted: the load
/// and the identity capture happen under one loader write guard with no
/// await in between, and the disposer compares that exact instance under the
/// same guard before removing anything. A duplicate mount keeps the loader's
/// existing skip semantics and captures no identity, so its disposer is a
/// successful no-op that can never evict the incumbent; a stale identity
/// (already unloaded, replaced under the same id, or dropped by shutdown)
/// disposes as a no-op that preserves whatever is mounted now.
pub(crate) async fn load_wasm_effect(
    loader: Arc<tokio::sync::RwLock<PluginLoader>>,
    manifest: &PluginManifest,
) -> ExtensionResult<Disposer> {
    debug_assert_eq!(
        manifest.kind,
        PluginKind::Wasm,
        "only WASM plugins have a module to load"
    );
    let plugin_id = manifest.id.clone();
    let identity = {
        let mut guard = loader.write().await;
        // The same predicate `load_plugin` uses to skip a duplicate, checked
        // under the same guard so the two can never disagree.
        let skipped_duplicate = guard.is_loaded(&plugin_id);
        guard.load_plugin(manifest)?;
        if skipped_duplicate {
            None
        } else {
            if let Err(error) = guard.bind_context_exports(manifest) {
                guard.unload_plugin(&plugin_id)?;
                return Err(error);
            }
            let identity = guard.wasm_mount_identity(&plugin_id);
            debug_assert!(
                identity.is_some(),
                "a successful fresh load leaves an instance to capture"
            );
            identity
        }
    };
    Ok(async_disposer(move || async move {
        let mut guard = loader.write().await;
        if let Some(identity) = identity {
            guard.unload_wasm_mount(&plugin_id, &identity);
        }
        Ok(())
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
    use crate::extension::runtime::wasm::tests::guest_fixture::{
        manifest_with_guest, HANDLER, PAYLOAD_A, PAYLOAD_B,
    };
    use crate::memory::extensions::manifest::{MemoryHook, MemoryManifestSection};
    use crate::memory::extensions::traits::MemoryExtension;
    use std::path::PathBuf;
    use tempfile::TempDir;

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
    fn loader_binds_real_guest_and_preserves_legacy_tool_hook_calls() {
        let directory = TempDir::new().unwrap();
        let mut manifest = manifest_with_guest(directory.path(), "loader-a", PAYLOAD_A);
        manifest.hooks_v2 = Some(vec![crate::extension::manifest::HookSection {
            event: "before_tool_call".into(),
            kind: None,
            handler: Some(HANDLER.into()),
            priority: "normal".into(),
            filter: None,
        }]);
        let mut loader = PluginLoader::new();
        loader.load_plugin(&manifest).unwrap();

        let binding = loader.bind_wasm("loader-a", HANDLER).unwrap();
        assert_eq!(binding.plugin_id(), "loader-a");
        assert_eq!(binding.export(), HANDLER);
        assert_eq!(
            loader.call_bound(&binding, serde_json::json!({})).unwrap(),
            serde_json::json!("A")
        );
        assert_eq!(
            loader
                .call_tool("loader-a", HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );
        assert_eq!(
            loader
                .execute_hook("loader-a", HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );
    }

    #[test]
    fn loader_bind_refuses_unknown_export_id_and_wrong_kind() {
        let directory = TempDir::new().unwrap();
        let manifest = manifest_with_guest(directory.path(), "loader-export", PAYLOAD_A);
        let mut loader = PluginLoader::new();
        loader.load_plugin(&manifest).unwrap();

        assert!(matches!(
            loader.bind_wasm("loader-export", "missing"),
            Err(ExtensionError::Runtime(message))
                if message == "WASM export not found: missing"
        ));
        assert!(matches!(
            loader.bind_wasm("missing", HANDLER),
            Err(ExtensionError::PluginNotFound(id)) if id == "missing"
        ));
        loader
            .loaded_plugins
            .insert("static-id".to_string(), PluginKind::Static);
        assert!(matches!(
            loader.bind_wasm("static-id", HANDLER),
            Err(ExtensionError::Runtime(message))
                if message == "Plugin kind Static does not support WASM bindings"
        ));
    }

    #[test]
    fn loader_unload_reload_refuses_stale_clones_and_returns_b() {
        let directory_a = TempDir::new().unwrap();
        let directory_b = TempDir::new().unwrap();
        let manifest_a = manifest_with_guest(directory_a.path(), "loader-reload", PAYLOAD_A);
        let manifest_b = manifest_with_guest(directory_b.path(), "loader-reload", PAYLOAD_B);
        let mut loader = PluginLoader::new();
        loader.load_plugin(&manifest_a).unwrap();
        let old = loader.bind_wasm("loader-reload", HANDLER).unwrap();
        let old_clone = old.clone();
        loader.unload_plugin("loader-reload").unwrap();
        assert!(matches!(
            loader.call_bound(&old, serde_json::json!({})),
            Err(ExtensionError::PluginNotFound(id)) if id == "loader-reload"
        ));
        loader.load_plugin(&manifest_b).unwrap();
        for binding in [&old, &old_clone] {
            assert!(matches!(
                loader.call_bound(binding, serde_json::json!({})),
                Err(ExtensionError::Runtime(message))
                    if message == "WASM binding no longer mounted: loader-reload"
            ));
        }
        let fresh = loader.bind_wasm("loader-reload", HANDLER).unwrap();
        assert_eq!(
            loader.call_bound(&fresh, serde_json::json!({})).unwrap(),
            serde_json::json!("B")
        );
    }

    #[test]
    fn loader_shutdown_clears_runtime_and_allows_lazy_restart() {
        let directory_a = TempDir::new().unwrap();
        let directory_b = TempDir::new().unwrap();
        let manifest_a = manifest_with_guest(directory_a.path(), "loader-shutdown", PAYLOAD_A);
        let manifest_b = manifest_with_guest(directory_b.path(), "loader-shutdown", PAYLOAD_B);
        let mut loader = PluginLoader::new();
        loader.set_all_plugin_settings(std::collections::HashMap::from([(
            "loader-shutdown".to_string(),
            serde_json::json!({"key": "value"}),
        )]));
        loader.load_plugin(&manifest_a).unwrap();
        let old = loader.bind_wasm("loader-shutdown", HANDLER).unwrap();
        assert_eq!(
            loader
                .call_tool("loader-shutdown", HANDLER, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );
        loader.shutdown();
        assert_eq!(loader.loaded_count(), 0);
        if let Some(runtime) = loader.wasm_runtime.as_ref() {
            assert!(
                runtime
                    .call_bound(
                        &old,
                        crate::extension::runtime::WasmToolInput {
                            name: HANDLER.to_string(),
                            arguments: serde_json::json!({}),
                        },
                    )
                    .is_err(),
                "shutdown must not retain an executable old instance"
            );
        }
        assert!(!loader.is_wasm_runtime_active());
        assert!(matches!(
            loader.call_bound(&old, serde_json::json!({})),
            Err(ExtensionError::PluginNotFound(id)) if id == "loader-shutdown"
        ));
        assert!(matches!(
            loader.call_tool("loader-shutdown", HANDLER, serde_json::json!({})),
            Err(ExtensionError::PluginNotFound(id)) if id == "loader-shutdown"
        ));
        assert!(matches!(
            loader.execute_hook("loader-shutdown", HANDLER, serde_json::json!({})),
            Err(ExtensionError::PluginNotFound(id)) if id == "loader-shutdown"
        ));
        loader.shutdown();
        assert_eq!(
            loader.plugin_settings["loader-shutdown"],
            serde_json::json!({"key": "value"})
        );
        loader.load_plugin(&manifest_b).unwrap();
        assert!(matches!(
            loader.call_bound(&old, serde_json::json!({})),
            Err(ExtensionError::Runtime(message))
                if message == "WASM binding no longer mounted: loader-shutdown"
        ));
        let fresh = loader.bind_wasm("loader-shutdown", HANDLER).unwrap();
        assert_eq!(
            loader.call_bound(&fresh, serde_json::json!({})).unwrap(),
            serde_json::json!("B")
        );
    }

    #[test]
    fn loader_mapping_matches_bound_tool_and_hook_outputs() {
        let cases = [
            (
                "loader-null",
                r#"{"success":true,"result":null,"error":null}"#,
                Ok(serde_json::Value::Null),
            ),
            (
                "loader-error",
                r#"{"success":false,"result":null,"error":"guest failure"}"#,
                Err("guest failure"),
            ),
            (
                "loader-unknown",
                r#"{"success":false,"result":null,"error":null}"#,
                Err("Unknown WASM error"),
            ),
        ];
        for (id, payload, expected) in cases {
            let directory = TempDir::new().unwrap();
            let mut manifest = manifest_with_guest(directory.path(), id, payload);
            manifest.hooks_v2 = Some(vec![crate::extension::manifest::HookSection {
                event: "before_tool_call".into(),
                kind: None,
                handler: Some(HANDLER.into()),
                priority: "normal".into(),
                filter: None,
            }]);
            let mut loader = PluginLoader::new();
            loader.load_plugin(&manifest).unwrap();
            let binding = loader.bind_wasm(id, HANDLER).unwrap();
            for actual in [
                loader.call_bound(&binding, serde_json::json!({})),
                loader.call_tool(id, HANDLER, serde_json::json!({})),
                loader.execute_hook(id, HANDLER, serde_json::json!({})),
            ] {
                match expected.as_ref() {
                    Ok(value) => assert_eq!(actual.unwrap(), *value),
                    Err(message) => assert!(matches!(
                        actual,
                        Err(ExtensionError::Runtime(actual)) if actual == *message
                    )),
                }
            }
        }
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

    // ===== C2-D: a disposer owns only the instance its effect mounted =====

    #[tokio::test]
    async fn c2d_owned_disposer_unloads_its_real_guest() {
        let directory = TempDir::new().unwrap();
        let manifest = manifest_with_guest(directory.path(), "c2d-owned", PAYLOAD_A);
        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));

        let disposer = load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .expect("real guest loads");
        let binding = loader.read().await.bind_wasm("c2d-owned", HANDLER).unwrap();
        assert_eq!(
            loader
                .read()
                .await
                .call_bound(&binding, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );

        disposer().await.expect("owned disposer succeeds");
        let guard = loader.read().await;
        assert!(!guard.is_loaded("c2d-owned"));
        assert_eq!(guard.loaded_count(), 0);
        assert_eq!(guard.get_plugin_kind("c2d-owned"), None);
        assert!(matches!(
            guard.call_bound(&binding, serde_json::json!({})),
            Err(ExtensionError::PluginNotFound(id)) if id == "c2d-owned"
        ));
    }

    #[tokio::test]
    async fn c2d_late_disposer_preserves_same_id_replacement() {
        let directory_a = TempDir::new().unwrap();
        let directory_b = TempDir::new().unwrap();
        let manifest_a = manifest_with_guest(directory_a.path(), "c2d-late", PAYLOAD_A);
        let manifest_b = manifest_with_guest(directory_b.path(), "c2d-late", PAYLOAD_B);
        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));

        let disposer_a = load_wasm_effect(Arc::clone(&loader), &manifest_a)
            .await
            .expect("mount A succeeds");
        let binding_a = loader.read().await.bind_wasm("c2d-late", HANDLER).unwrap();
        assert_eq!(
            loader
                .read()
                .await
                .call_bound(&binding_a, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );

        // The legitimate existing unload path removes A, then a fresh same-id
        // mount installs B.
        loader.write().await.unload_plugin("c2d-late").unwrap();
        let disposer_b = load_wasm_effect(Arc::clone(&loader), &manifest_b)
            .await
            .expect("mount B succeeds");
        let binding_b = loader.read().await.bind_wasm("c2d-late", HANDLER).unwrap();
        assert_eq!(
            loader
                .read()
                .await
                .call_bound(&binding_b, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("B")
        );

        // A's late disposer succeeds without touching B or its index entry.
        disposer_a()
            .await
            .expect("stale disposer is a successful no-op");
        {
            let guard = loader.read().await;
            assert!(guard.is_loaded("c2d-late"));
            assert_eq!(guard.get_plugin_kind("c2d-late"), Some(PluginKind::Wasm));
            assert_eq!(guard.loaded_count(), 1);
            assert_eq!(
                guard.call_bound(&binding_b, serde_json::json!({})).unwrap(),
                serde_json::json!("B")
            );
            assert!(matches!(
                guard.call_bound(&binding_a, serde_json::json!({})),
                Err(ExtensionError::Runtime(message))
                    if message == "WASM binding no longer mounted: c2d-late"
            ));
        }

        // B's own disposer removes B.
        disposer_b().await.expect("owned disposer succeeds");
        let guard = loader.read().await;
        assert!(!guard.is_loaded("c2d-late"));
        assert_eq!(guard.loaded_count(), 0);
        assert!(matches!(
            guard.call_bound(&binding_b, serde_json::json!({})),
            Err(ExtensionError::PluginNotFound(id)) if id == "c2d-late"
        ));
    }

    #[tokio::test]
    async fn c2d_duplicate_skipped_effect_cannot_unload_incumbent() {
        let directory_a = TempDir::new().unwrap();
        let directory_b = TempDir::new().unwrap();
        let manifest_a = manifest_with_guest(directory_a.path(), "c2d-dup", PAYLOAD_A);
        let manifest_b = manifest_with_guest(directory_b.path(), "c2d-dup", PAYLOAD_B);
        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));

        let disposer_a = load_wasm_effect(Arc::clone(&loader), &manifest_a)
            .await
            .expect("mount A succeeds");
        // Same id and even a different manifest payload: the loader keeps its
        // skip semantics, the guest stays A, and this effect owns nothing.
        let disposer_skipped = load_wasm_effect(Arc::clone(&loader), &manifest_b)
            .await
            .expect("a skipped duplicate mount is not an error");
        let binding = loader.read().await.bind_wasm("c2d-dup", HANDLER).unwrap();
        assert_eq!(
            loader
                .read()
                .await
                .call_bound(&binding, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A"),
            "the skipped load retains the incumbent guest"
        );

        disposer_skipped()
            .await
            .expect("a disposer that owns nothing succeeds");
        {
            let guard = loader.read().await;
            assert!(guard.is_loaded("c2d-dup"));
            assert_eq!(guard.get_plugin_kind("c2d-dup"), Some(PluginKind::Wasm));
            assert_eq!(guard.loaded_count(), 1);
            assert_eq!(
                guard.call_bound(&binding, serde_json::json!({})).unwrap(),
                serde_json::json!("A")
            );
        }

        disposer_a()
            .await
            .expect("the real owner's disposer unloads");
        let guard = loader.read().await;
        assert!(!guard.is_loaded("c2d-dup"));
        assert_eq!(guard.loaded_count(), 0);
        assert!(matches!(
            guard.call_bound(&binding, serde_json::json!({})),
            Err(ExtensionError::PluginNotFound(id)) if id == "c2d-dup"
        ));
    }

    #[tokio::test]
    async fn c2d_old_disposer_preserves_post_shutdown_replacement() {
        let directory_a = TempDir::new().unwrap();
        let directory_b = TempDir::new().unwrap();
        let manifest_a = manifest_with_guest(directory_a.path(), "c2d-shutdown", PAYLOAD_A);
        let manifest_b = manifest_with_guest(directory_b.path(), "c2d-shutdown", PAYLOAD_B);
        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));

        let disposer_a = load_wasm_effect(Arc::clone(&loader), &manifest_a)
            .await
            .expect("mount A succeeds");
        let binding_a = loader
            .read()
            .await
            .bind_wasm("c2d-shutdown", HANDLER)
            .unwrap();
        assert_eq!(
            loader
                .read()
                .await
                .call_bound(&binding_a, serde_json::json!({}))
                .unwrap(),
            serde_json::json!("A")
        );

        // Shutdown drops the whole runtime, then the same id mounts again as
        // B on a fresh runtime.
        loader.write().await.shutdown();
        let disposer_b = load_wasm_effect(Arc::clone(&loader), &manifest_b)
            .await
            .expect("mount B after shutdown");

        // A's disposer predates the shutdown and must not remove B.
        disposer_a()
            .await
            .expect("stale disposer is a successful no-op");
        let binding_b = loader
            .read()
            .await
            .bind_wasm("c2d-shutdown", HANDLER)
            .unwrap();
        {
            let guard = loader.read().await;
            assert!(guard.is_loaded("c2d-shutdown"));
            assert_eq!(
                guard.get_plugin_kind("c2d-shutdown"),
                Some(PluginKind::Wasm)
            );
            assert_eq!(guard.loaded_count(), 1);
            assert_eq!(
                guard.call_bound(&binding_b, serde_json::json!({})).unwrap(),
                serde_json::json!("B")
            );
            assert!(matches!(
                guard.call_bound(&binding_a, serde_json::json!({})),
                Err(ExtensionError::Runtime(message))
                    if message == "WASM binding no longer mounted: c2d-shutdown"
            ));
        }

        disposer_b().await.expect("owned disposer succeeds");
        let guard = loader.read().await;
        assert!(!guard.is_loaded("c2d-shutdown"));
        assert_eq!(guard.loaded_count(), 0);
    }

    #[tokio::test]
    async fn c2d_exportless_module_has_owned_cleanup() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("plugin.wasm"), EMPTY_WASM_MODULE).unwrap();
        let mut manifest = PluginManifest::new(
            "c2d-exportless".to_string(),
            "C2D exportless".to_string(),
            PluginKind::Wasm,
            PathBuf::from("plugin.wasm"),
        );
        manifest.root_dir = tmp.path().to_path_buf();
        let loader = Arc::new(tokio::sync::RwLock::new(PluginLoader::new()));

        // No handler export exists and identity capture must not invent one:
        // the effect still owns the mount and its disposer cleans it up.
        let disposer = load_wasm_effect(Arc::clone(&loader), &manifest)
            .await
            .expect("an empty module links");
        {
            let guard = loader.read().await;
            assert!(guard.is_loaded("c2d-exportless"));
            assert_eq!(
                guard.get_plugin_kind("c2d-exportless"),
                Some(PluginKind::Wasm)
            );
            assert!(matches!(
                guard.bind_wasm("c2d-exportless", HANDLER),
                Err(ExtensionError::Runtime(message))
                    if message == format!("WASM export not found: {HANDLER}")
            ));
        }

        disposer().await.expect("owned disposer succeeds");
        let guard = loader.read().await;
        assert!(!guard.is_loaded("c2d-exportless"));
        assert_eq!(guard.loaded_count(), 0);
    }
}
