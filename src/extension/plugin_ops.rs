//! Plugin execution operations for `ExtensionManager`

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest;
use crate::extension::registry::PluginRegistry;
use crate::extension::types::{DirectCommandResult, PluginInfo, PluginRecord};
use crate::sync_primitives::Arc;

use super::ExtensionManager;

impl ExtensionManager {
    /// Call a tool on a runtime plugin (Node.js or WASM).
    ///
    /// Auto-loads the plugin if not already loaded (lazy initialization).
    /// Acquires a write lock on the plugin loader for IPC. The guest call is
    /// CPU-bound untrusted code, so it runs on the blocking pool (bounded by
    /// the Extism manifest timeout) rather than on an async worker thread.
    pub async fn call_plugin_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.ensure_plugin_active(plugin_id).await?;
        // Auto-load plugin if not already loaded
        {
            let loader = self.plugin_loader.read().await;
            if !loader.is_loaded(plugin_id) {
                drop(loader);
                self.ensure_plugin_loaded(plugin_id).await?;
            }
        }

        let loader = self.plugin_loader.clone().write_owned().await;
        let plugin_id = plugin_id.to_string();
        let handler = handler.to_string();
        tokio::task::spawn_blocking(move || loader.call_tool(&plugin_id, &handler, args))
            .await
            .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    async fn ensure_plugin_active(&self, plugin_id: &str) -> ExtensionResult<()> {
        let registry = self.plugin_registry.read().await;
        match registry.get_plugin(plugin_id) {
            Some(record) if record.status.is_active() => Ok(()),
            Some(_) => Err(ExtensionError::Runtime(format!(
                "Plugin '{plugin_id}' is disabled"
            ))),
            None => Err(ExtensionError::PluginNotFound(plugin_id.to_string())),
        }
    }

    /// Ensure a plugin is loaded into the runtime.
    pub(crate) async fn ensure_plugin_loaded(&self, plugin_id: &str) -> ExtensionResult<()> {
        self.ensure_plugin_active(plugin_id).await?;
        let registry = self.plugin_registry.read().await;
        let record = registry
            .get_plugin(plugin_id)
            .filter(|record| record.status.is_active())
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;
        let root_dir = record.root_dir.clone();
        drop(registry);

        let manifest = manifest::parse_manifest_from_dir_sync(&root_dir)?;

        let mut loader = self.plugin_loader.write().await;
        if loader.is_loaded(plugin_id) {
            return Ok(()); // another task loaded it while we waited
        }
        let mem_reg_snapshot = self
            .memory_registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(ref mem_reg) = mem_reg_snapshot {
            loader.load_plugin_with_memory(&manifest, mem_reg)?;
        } else {
            loader.load_plugin(&manifest)?;
        }
        tracing::info!(
            plugin_id = plugin_id,
            "Auto-loaded plugin for tool execution"
        );
        drop(loader);

        // X1: bind the just-loaded plugin's memory caller if the MCP handle is
        // already live (idempotent for already-bound extensions).
        self.bind_memory_callers().await;
        Ok(())
    }

    /// Execute a hook handler on a runtime plugin.
    ///
    /// Auto-loads the plugin if not already loaded (mirrors
    /// [`Self::call_plugin_tool`]), so a hook can fire on the first event
    /// without a prior explicit load.
    pub async fn execute_plugin_hook(
        &self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.ensure_plugin_active(plugin_id).await?;
        // Auto-load plugin if not already loaded
        {
            let loader = self.plugin_loader.read().await;
            if !loader.is_loaded(plugin_id) {
                drop(loader);
                self.ensure_plugin_loaded(plugin_id).await?;
            }
        }

        let loader = self.plugin_loader.clone().read_owned().await;
        let plugin_id = plugin_id.to_string();
        let handler = handler.to_string();
        tokio::task::spawn_blocking(move || loader.execute_hook(&plugin_id, &handler, event_data))
            .await
            .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    /// Execute a direct command on a runtime plugin.
    pub async fn execute_plugin_command(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<DirectCommandResult> {
        self.ensure_plugin_active(plugin_id).await?;
        {
            let loader = self.plugin_loader.read().await;
            if !loader.is_loaded(plugin_id) {
                drop(loader);
                self.ensure_plugin_loaded(plugin_id).await?;
            }
        }

        let loader = self.plugin_loader.clone().read_owned().await;
        let plugin_id = plugin_id.to_string();
        let handler = handler.to_string();
        tokio::task::spawn_blocking(move || loader.execute_command(&plugin_id, &handler, args))
            .await
            .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    /// Get the plugin registry (read access).
    pub async fn get_plugin_registry(&self) -> tokio::sync::RwLockReadGuard<'_, PluginRegistry> {
        self.plugin_registry.read().await
    }

    /// P3 Stage I — clone the shared registry handle for per-subagent MCP scope
    /// provisioning. Unlike `get_plugin_registry` (a borrowed read guard tied to
    /// `&self`), this hands out the `Arc<RwLock<..>>` itself so a subagent
    /// spawner can carry it across layers and snapshot the referenced tools
    /// under its own read guard at provision time.
    #[must_use]
    pub fn plugin_registry_handle(&self) -> Arc<tokio::sync::RwLock<PluginRegistry>> {
        self.plugin_registry.clone()
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
        let mem_reg_snapshot = self
            .memory_registry
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let result = if let Some(ref mem_reg) = mem_reg_snapshot {
            loader.load_plugin_with_memory(manifest, mem_reg)
        } else {
            loader.load_plugin(manifest)
        };
        drop(loader);

        if result.is_ok() {
            self.sync_runtime_snapshots().await;
        }

        // X1: bind memory caller for the hot-loaded plugin (idempotent).
        self.bind_memory_callers().await;

        // Autostart any background services this plugin declared, now that
        // its runtime is loaded (mirrors MCP servers' auto_start behavior).
        if result.is_ok() {
            self.start_autostart_services(&manifest.id).await;
        }

        result
    }

    /// Unload a runtime plugin.
    ///
    /// For MCP plugins, the loader drops the in-memory `.mcp.json` configs on
    /// unload, so we capture the server IDs *first* and then ask the MCP manager
    /// to tear down those transient servers — otherwise they would keep running
    /// (and their tools stay registered) after the plugin is gone.
    pub async fn unload_runtime_plugin(&self, plugin_id: &str) -> ExtensionResult<()> {
        let server_ids: Vec<String> = {
            let loader = self.plugin_loader.read().await;
            loader
                .get_mcp_configs(plugin_id)
                .map(|servers| servers.keys().cloned().collect())
                .unwrap_or_default()
        };

        // Stop any background services this plugin registered *before* unloading
        // it — once the plugin is gone the stop handlers can no longer run, which
        // would leave their background tasks orphaned (and their tooling still
        // active) for the lifetime of the process.
        let service_registrations: Vec<crate::extension::registry::ServiceRegistration> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_services()
                .into_iter()
                .filter(|s| s.plugin_id == plugin_id)
                .cloned()
                .collect()
        };
        if !service_registrations.is_empty() {
            let service_manager = self.service_manager.clone().write_owned().await;
            let loader = self.plugin_loader.clone().read_owned().await;
            let plugin_id_owned = plugin_id.to_string();
            if let Err(e) = tokio::task::spawn_blocking(move || {
                let mut service_manager = service_manager;
                service_manager.stop_plugin_services(
                    &plugin_id_owned,
                    &service_registrations,
                    &loader,
                );
            })
            .await
            {
                tracing::warn!(error = %e, "stop_plugin_services task join failed");
            }
        }

        let result = self.plugin_loader.write().await.unload_plugin(plugin_id);

        if result.is_ok() && !server_ids.is_empty() {
            let handle = self
                .mcp_handle
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(handle) = handle {
                for server_id in server_ids {
                    if let Err(e) = handle.remove_transient_server(&server_id).await {
                        tracing::warn!(
                            plugin_id = plugin_id,
                            server_id = %server_id,
                            error = %e,
                            "failed to remove transient MCP server on plugin unload"
                        );
                    }
                }
            }
        }

        result
    }

    /// Get all plugin info from `PluginRegistry`.
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
                skills_count: record.skill_count,
                commands_count: record.command_count,
                agents_count: record.agent_count,
                hooks_count: record.hook_count,
                mcp_servers_count: record.mcp_server_count,
                tools_count: record.tool_names.len(),
                status: record.status.label().to_string(),
                error: record.error.clone(),
            })
            .collect()
    }

    /// Get a specific plugin record by name.
    pub async fn get_plugin_record(&self, name: &str) -> Option<PluginRecord> {
        self.plugin_registry.read().await.get_plugin(name).cloned()
    }

    /// List all plugin records (cloned) for the store's installed reconciliation.
    pub async fn list_plugin_records(&self) -> Vec<PluginRecord> {
        self.plugin_registry
            .read()
            .await
            .list_plugins()
            .into_iter()
            .cloned()
            .collect()
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
            if !enabled {
                let _ = self.unload_runtime_plugin(plugin_id).await;
            }
            self.sync_runtime_snapshots().await;
            *self.hook_executor.write().await = crate::extension::hooks::HookExecutor::empty()
                .with_consent(crate::extension::hooks::ShellHookConsent::shared());
            self.sync_hooks_from_registry().await;
            self.sync_user_hooks().await;

            let mut skill_dirs: Vec<std::path::PathBuf> = self
                .discovery
                .discover_skill_dirs()
                .unwrap_or_default()
                .into_iter()
                .map(|dir| dir.path)
                .collect();
            let (plugin_skill_dirs, plugin_subagents) = {
                let registry = self.plugin_registry.read().await;
                let active = |plugin_id: &str| {
                    registry
                        .get_plugin(plugin_id)
                        .is_some_and(|plugin| plugin.status.is_active())
                };
                let skill_dirs: Vec<std::path::PathBuf> = registry
                    .list_active_plugins()
                    .into_iter()
                    .map(|plugin| plugin.root_dir.join("skills"))
                    .filter(|dir| dir.is_dir())
                    .collect();
                let subagents = registry
                    .list_agents()
                    .into_iter()
                    .filter(|agent| active(&agent.plugin_id))
                    .filter_map(super::plugin_agent_to_def)
                    .collect();
                (skill_dirs, subagents)
            };
            skill_dirs.extend(plugin_skill_dirs.iter().cloned());
            crate::utils::paths::publish_plugin_skill_dirs(plugin_skill_dirs);
            crate::agents::publish_plugin_subagents(plugin_subagents);
            if let Err(e) = self.skill_system.init(skill_dirs).await {
                tracing::warn!(error = %e, "Failed to refresh skill system after plugin toggle");
            }
        }

        changed
    }
}
