//! Service management operations for ExtensionManager

use crate::extension::error::*;
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
        let mut service_manager = self.service_manager.write().await;
        let loader = self.plugin_loader.read().await;
        service_manager.start_service(&registration, &loader)
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
        let mut service_manager = self.service_manager.write().await;
        let loader = self.plugin_loader.read().await;
        service_manager.stop_service(&registration, &loader)
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

    /// Find a service registration by plugin_id and service_id.
    /// Extracted from the duplicated lookup logic in start_service/stop_service.
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
            .ok_or_else(|| ExtensionError::ServiceNotFound(format!("{}:{}", plugin_id, service_id)))
    }
}
