//! Plugin execution operations for ExtensionManager

use crate::extension::error::*;
use crate::extension::manifest;
use crate::extension::registry::PluginRegistry;
use crate::extension::types::*;

use super::ExtensionManager;

impl ExtensionManager {
    /// Call a tool on a runtime plugin (Node.js or WASM).
    ///
    /// Auto-loads the plugin if not already loaded (lazy initialization).
    /// Acquires a write lock on the plugin loader for IPC.
    pub async fn call_plugin_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        // Auto-load plugin if not already loaded
        {
            let loader = self.plugin_loader.read().await;
            if !loader.is_loaded(plugin_id) {
                drop(loader);
                self.ensure_plugin_loaded(plugin_id).await?;
            }
        }

        self.plugin_loader
            .write()
            .await
            .call_tool(plugin_id, handler, args)
    }

    /// Ensure a plugin is loaded into the runtime.
    pub(crate) async fn ensure_plugin_loaded(&self, plugin_id: &str) -> ExtensionResult<()> {
        let registry = self.plugin_registry.read().await;
        let record = registry
            .get_plugin(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;
        let root_dir = record.root_dir.clone();
        drop(registry);

        let manifest = manifest::parse_manifest_from_dir_sync(&root_dir)?;

        let mut loader = self.plugin_loader.write().await;
        if loader.is_loaded(plugin_id) {
            return Ok(()); // another task loaded it while we waited
        }
        let mut registry = self.plugin_registry.write().await;
        loader.load_plugin(&manifest, &mut registry)?;
        tracing::info!(
            plugin_id = plugin_id,
            "Auto-loaded plugin for tool execution"
        );
        Ok(())
    }

    /// Execute a hook handler on a runtime plugin.
    pub async fn execute_plugin_hook(
        &self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.plugin_loader
            .write()
            .await
            .execute_hook(plugin_id, handler, event_data)
    }

    /// Execute a direct command on a runtime plugin.
    pub async fn execute_plugin_command(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<DirectCommandResult> {
        self.plugin_loader
            .write()
            .await
            .execute_command(plugin_id, handler, args)
    }

    /// Get the plugin registry (read access).
    pub async fn get_plugin_registry(&self) -> tokio::sync::RwLockReadGuard<'_, PluginRegistry> {
        self.plugin_registry.read().await
    }

    /// Get mutable access to the plugin registry.
    pub async fn get_plugin_registry_mut(
        &self,
    ) -> tokio::sync::RwLockWriteGuard<'_, PluginRegistry> {
        self.plugin_registry.write().await
    }

    /// Get the plugin loader (read access).
    pub async fn get_plugin_loader(&self) -> tokio::sync::RwLockReadGuard<'_, super::PluginLoader> {
        self.plugin_loader.read().await
    }

    /// Load a runtime plugin from a manifest.
    pub async fn load_runtime_plugin(
        &self,
        manifest: &super::PluginManifest,
    ) -> ExtensionResult<()> {
        let mut loader = self.plugin_loader.write().await;
        let mut registry = self.plugin_registry.write().await;
        let result = loader.load_plugin(manifest, &mut registry);
        drop(registry);
        drop(loader);

        if result.is_ok() {
            self.sync_runtime_snapshots().await;
        }

        result
    }

    /// Unload a runtime plugin.
    pub async fn unload_runtime_plugin(&self, plugin_id: &str) -> ExtensionResult<()> {
        self.plugin_loader.write().await.unload_plugin(plugin_id)
    }

    /// Get all plugin info from PluginRegistry.
    pub async fn get_plugin_info(&self) -> Vec<PluginInfo> {
        self.plugin_registry
            .read()
            .await
            .list_plugins()
            .into_iter()
            .map(|record| PluginInfo {
                name: record.id.clone(),
                version: record.version.clone(),
                description: record.description.clone(),
                enabled: record.status.is_active(),
                path: record.root_dir.display().to_string(),
                skills_count: 0,
                commands_count: 0,
                agents_count: 0,
                hooks_count: record.hook_count,
                mcp_servers_count: 0,
            })
            .collect()
    }

    /// Get a specific plugin record by name.
    pub async fn get_plugin_record(&self, name: &str) -> Option<PluginRecord> {
        self.plugin_registry.read().await.get_plugin(name).cloned()
    }

    /// Enable or disable a plugin and refresh runtime snapshots.
    pub async fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> bool {
        let changed = {
            let mut registry = self.plugin_registry.write().await;
            if enabled {
                registry.enable_plugin(plugin_id)
            } else {
                registry.disable_plugin(plugin_id)
            }
        };

        if changed {
            self.sync_runtime_snapshots().await;
        }

        changed
    }
}
