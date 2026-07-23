//! Service management operations for `ExtensionManager`

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::types::ServiceInfo;

use super::ExtensionManager;

impl ExtensionManager {
    /// Start a background service.
    pub async fn start_service(
        &self,
        plugin_id: &str,
        service_id: &str,
    ) -> ExtensionResult<ServiceInfo> {
        let registration = self
            .find_service_registration(plugin_id, service_id)
            .await?;
        // Manifest-declared services can exist before their plugin runtime is
        // loaded — load it on demand (no-op when already loaded). A failure
        // still flows into start_service, which records the Failed state.
        if let Err(e) = self.ensure_plugin_loaded(plugin_id).await {
            tracing::warn!(
                plugin = %plugin_id,
                error = %e,
                "start_service: failed to load plugin runtime"
            );
        }
        let service_manager = self.service_manager.clone().write_owned().await;
        let loader = self.plugin_loader.clone().read_owned().await;
        // The start handler is untrusted guest code — run it off the async
        // worker pool (bounded by the Extism manifest timeout).
        tokio::task::spawn_blocking(move || {
            let mut service_manager = service_manager;
            service_manager.start_service(&registration, &loader)
        })
        .await
        .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    /// Stop a background service.
    pub async fn stop_service(
        &self,
        plugin_id: &str,
        service_id: &str,
    ) -> ExtensionResult<ServiceInfo> {
        let registration = self
            .find_service_registration(plugin_id, service_id)
            .await?;
        let service_manager = self.service_manager.clone().write_owned().await;
        let loader = self.plugin_loader.clone().read_owned().await;
        tokio::task::spawn_blocking(move || {
            let mut service_manager = service_manager;
            service_manager.stop_service(&registration, &loader)
        })
        .await
        .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    /// Start the `auto_start` services declared by a single (already-loaded)
    /// plugin. Idempotent — services already Running are left alone.
    ///
    /// Returns the number of services that are Running afterwards.
    pub(crate) async fn start_autostart_services(&self, plugin_id: &str) -> usize {
        let pending: Vec<crate::extension::registry::ServiceRegistration> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_services()
                .into_iter()
                .filter(|s| s.plugin_id == plugin_id && s.auto_start)
                .cloned()
                .collect()
        };
        if pending.is_empty() {
            return 0;
        }

        let service_manager = self.service_manager.clone().write_owned().await;
        let loader = self.plugin_loader.clone().read_owned().await;
        let plugin_id = plugin_id.to_string();
        tokio::task::spawn_blocking(move || {
            let mut service_manager = service_manager;
            let mut running = 0;
            for registration in &pending {
                match service_manager.start_service(registration, &loader) {
                    Ok(info) if info.state == crate::extension::types::ServiceState::Running => {
                        running += 1;
                    }
                    Ok(info) => {
                        tracing::warn!(
                            plugin = %plugin_id,
                            service = %registration.id,
                            state = ?info.state,
                            error = ?info.error,
                            "autostart service did not reach Running state"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            plugin = %plugin_id,
                            service = %registration.id,
                            error = %e,
                            "failed to autostart service"
                        );
                    }
                }
            }
            running
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "autostart services task join failed");
            0
        })
    }

    /// Load active plugins that declare `auto_start` services and start those
    /// services. Idempotent; called at daemon boot and after hot-reloads —
    /// mirrors the MCP transient-server auto-registration in
    /// [`Self::sync_mcp_plugin_servers`].
    ///
    /// Returns the number of services Running afterwards.
    pub async fn sync_plugin_services(&self) -> usize {
        let plugin_ids: Vec<String> = {
            let registry = self.plugin_registry.read().await;
            let mut ids: Vec<String> = registry
                .list_services()
                .into_iter()
                .filter(|s| s.auto_start)
                .filter(|s| {
                    registry
                        .get_plugin(&s.plugin_id)
                        .is_some_and(|r| r.status.is_active())
                })
                .map(|s| s.plugin_id.clone())
                .collect();
            ids.sort();
            ids.dedup();
            ids
        };

        let mut running = 0;
        for plugin_id in &plugin_ids {
            // No-op when the plugin is already loaded; needs the loader write
            // lock, so it must run before start_autostart_services takes the
            // read lock.
            if let Err(e) = self.ensure_plugin_loaded(plugin_id).await {
                tracing::warn!(
                    plugin = %plugin_id,
                    error = %e,
                    "sync_plugin_services: failed to load plugin for autostart services"
                );
                continue;
            }
            running += self.start_autostart_services(plugin_id).await;
        }
        running
    }

    /// Stop every running plugin service (best-effort). Called at daemon
    /// shutdown so plugin background work is torn down before process exit.
    pub async fn stop_all_services(&self) -> usize {
        let service_manager = self.service_manager.clone().write_owned().await;
        let loader = self.plugin_loader.clone().read_owned().await;
        tokio::task::spawn_blocking(move || {
            let mut service_manager = service_manager;
            service_manager.stop_all(&loader).len()
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "stop_all_services task join failed");
            0
        })
    }

    /// Stop running services whose plugin is no longer active (removed or
    /// disabled on disk). Called after hot-reloads rebuild the registry.
    pub async fn stop_orphaned_services(&self) -> usize {
        let active: std::collections::HashSet<String> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_plugins()
                .into_iter()
                .filter(|r| r.status.is_active())
                .map(|r| r.id.clone())
                .collect()
        };
        let service_manager = self.service_manager.clone().write_owned().await;
        let loader = self.plugin_loader.clone().read_owned().await;
        tokio::task::spawn_blocking(move || {
            let mut service_manager = service_manager;
            service_manager
                .stop_orphaned(|id| active.contains(id), &loader)
                .len()
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "stop_orphaned_services task join failed");
            0
        })
    }

    /// Get service status.
    pub async fn get_service_status(
        &self,
        plugin_id: &str,
        service_id: &str,
    ) -> Option<ServiceInfo> {
        self.service_manager
            .read()
            .await
            .get_service(plugin_id, service_id)
            .cloned()
    }

    /// List all tracked services.
    pub async fn list_services(&self) -> Vec<ServiceInfo> {
        self.service_manager
            .read()
            .await
            .list_services()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Get the count of running services.
    pub async fn running_service_count(&self) -> usize {
        self.service_manager.read().await.running_count()
    }

    /// Get the service manager (read access).
    pub async fn get_service_manager(
        &self,
    ) -> tokio::sync::RwLockReadGuard<'_, super::ServiceManager> {
        self.service_manager.read().await
    }

    /// Find a service registration by `plugin_id` and `service_id`.
    /// Extracted from the duplicated lookup logic in `start_service/stop_service`.
    async fn find_service_registration(
        &self,
        plugin_id: &str,
        service_id: &str,
    ) -> ExtensionResult<crate::extension::registry::ServiceRegistration> {
        let registry = self.plugin_registry.read().await;
        registry
            .list_services()
            .into_iter()
            .find(|s| s.plugin_id == plugin_id && s.id == service_id)
            .cloned()
            .ok_or_else(|| ExtensionError::ServiceNotFound(format!("{plugin_id}:{service_id}")))
    }
}
