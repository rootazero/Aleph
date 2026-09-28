//! Service management operations for `ExtensionManager`

use crate::extension::effects::{async_disposer, Disposer};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::registry::ServiceRegistration;
use crate::extension::types::{ServiceInfo, ServiceState};

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
        // The service's start handler is guest code in the plugin's module,
        // which loads at mount (`wasm_module` step) — never on demand here.
        if !self.plugin_loader.read().await.is_loaded(plugin_id) {
            return Err(ExtensionError::Runtime(format!(
                "Plugin '{plugin_id}' has no runtime mounted; enable it first"
            )));
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

    /// The `service` effect for one mounted plugin.
    ///
    /// Starts every `auto_start` service the registry holds for `plugin_id`
    /// (the plugin's runtime must already be loaded — `wasm_module` is the
    /// step before this one). The returned disposer stops EVERY registered
    /// service of the plugin, autostarted or started later through
    /// `services.start`, then forgets their rows — so a manual start never
    /// outlives the mount (G1 exempts `start_service` on exactly this ground).
    ///
    /// Start failures are recorded on the `ServiceInfo` row (`Failed`) and do
    /// not fail the mount; the disposer reports a stop failure by service id.
    pub(crate) async fn start_services_effect(&self, plugin_id: &str) -> Disposer {
        let registrations: Vec<ServiceRegistration> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_services()
                .into_iter()
                .filter(|s| s.plugin_id == plugin_id)
                .cloned()
                .collect()
        };
        let pending: Vec<ServiceRegistration> = registrations
            .iter()
            .filter(|s| s.auto_start)
            .cloned()
            .collect();
        if !pending.is_empty() {
            let service_manager = self.service_manager.clone().write_owned().await;
            let loader = self.plugin_loader.clone().read_owned().await;
            let id = plugin_id.to_string();
            // The start handler is untrusted guest code — off the async pool.
            let join = tokio::task::spawn_blocking(move || {
                let mut service_manager = service_manager;
                for registration in &pending {
                    match service_manager.start_service(registration, &loader) {
                        Ok(info) if info.state == ServiceState::Running => {}
                        Ok(info) => tracing::warn!(
                            plugin = %id, service = %registration.id, state = ?info.state,
                            error = ?info.error, "autostart service did not reach Running state"
                        ),
                        Err(e) => tracing::warn!(
                            plugin = %id, service = %registration.id, error = %e,
                            "failed to autostart service"
                        ),
                    }
                }
            })
            .await;
            if let Err(e) = join {
                tracing::warn!(error = %e, "autostart services task join failed");
            }
        }

        let service_manager = self.service_manager.clone();
        let loader = self.plugin_loader.clone();
        let id = plugin_id.to_string();
        async_disposer(move || async move {
            let sm = service_manager.write_owned().await;
            let ld = loader.read_owned().await;
            let plugin = id.clone();
            let results = tokio::task::spawn_blocking(move || {
                let mut sm = sm;
                let results = sm.stop_plugin_services(&plugin, &registrations, &ld);
                sm.forget_plugin(&plugin);
                results
            })
            .await
            .map_err(|e| format!("stop_plugin_services task join failed: {e}"))?;
            let failed: Vec<String> = results
                .iter()
                .filter(|i| i.state == ServiceState::Failed)
                .map(|i| crate::extension::namespaced_component_key(&i.plugin_id, &i.id))
                .collect();
            if failed.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "services failed to stop cleanly: {}",
                    failed.join(", ")
                ))
            }
        })
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
        self.service_manager
            .read()
            .await
            .list_services()
            .into_iter()
            .filter(|info| info.state == crate::extension::types::ServiceState::Running)
            .count()
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
            .ok_or_else(|| {
                ExtensionError::ServiceNotFound(crate::extension::namespaced_component_key(
                    plugin_id, service_id,
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::discovery::DiscoveryConfig;
    use crate::extension::types::ServiceState;
    use crate::extension::{ExtensionConfig, ExtensionManager, ServiceRegistration};

    /// A manager whose registry holds one WASM plugin with one autostart
    /// service, backed by the empty module fixture (`loader.rs::EMPTY_WASM_MODULE`):
    /// the module links, the `start_ticker` export does not exist, so the
    /// start is recorded as `Failed` — which is the interesting row for a
    /// disposer to have to clean up.
    async fn manager_with_one_service(
        tmp: &std::path::Path,
    ) -> (ExtensionManager, crate::utils::paths::IsolatedAlephHome) {
        // Held by the caller for the whole test: `Config::load()` writes a
        // default config under the Aleph home when none exists (see the
        // `IsolatedAlephHome` doc) and the manager may touch it after `new`.
        let home = crate::utils::paths::IsolatedAlephHome::new();
        let manager = ExtensionManager::new(ExtensionConfig {
            discovery: DiscoveryConfig {
                working_dir: tmp.to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 0,
                claude_home_override: None,
            },
            plugins_config_path: Some(tmp.join("plugins.toml")),
            extra_plugin_parents: vec![],
        })
        .await
        .unwrap();
        std::fs::write(
            tmp.join("plugin.wasm"),
            crate::extension::loader::EMPTY_WASM_MODULE,
        )
        .unwrap();
        let mut manifest = crate::extension::PluginManifest::new(
            "qa-wasm".into(),
            "QA WASM".into(),
            crate::extension::PluginKind::Wasm,
            std::path::PathBuf::from("plugin.wasm"),
        );
        manifest.root_dir = tmp.to_path_buf();
        manager
            .get_plugin_loader_for_test()
            .write()
            .await
            .load_plugin(&manifest)
            .unwrap();
        {
            let mut reg = manager.get_plugin_registry_mut().await;
            reg.register_plugin(crate::extension::PluginRecord::new(
                "qa-wasm".into(),
                "QA WASM".into(),
                crate::extension::PluginKind::Wasm,
                crate::extension::PluginOrigin::Global,
            ));
            reg.register_service(ServiceRegistration {
                id: "ticker".into(),
                name: "ticker".into(),
                start_handler: "start_ticker".into(),
                stop_handler: "stop_ticker".into(),
                plugin_id: "qa-wasm".into(),
                auto_start: true,
            });
        }
        (manager, home)
    }

    #[tokio::test]
    async fn start_services_effect_starts_autostart_rows_and_its_disposer_forgets_them() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, _home) = manager_with_one_service(tmp.path()).await;

        let disposer = manager.start_services_effect("qa-wasm").await;
        let rows = manager.list_services().await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].id, "ticker");
        assert_eq!(
            rows[0].state,
            ServiceState::Failed,
            "empty module has no start_ticker export"
        );

        let outcome = disposer().await;
        // The stop handler does not exist either, so the stop is reported —
        // honestly — as a failure with the service named...
        assert!(
            outcome.as_ref().is_err_and(|e| e.contains("ticker")),
            "{outcome:?}"
        );
        // ...and the rows are gone regardless: a disposer that leaves state
        // behind is not an inverse.
        assert!(manager.list_services().await.is_empty());
    }

    #[tokio::test]
    async fn start_services_effect_with_no_registrations_is_an_empty_disposer() {
        let tmp = tempfile::tempdir().unwrap();
        let (manager, _home) = manager_with_one_service(tmp.path()).await;
        let disposer = manager.start_services_effect("someone-else").await;
        assert!(manager.list_services().await.is_empty());
        disposer().await.unwrap();
    }
}
