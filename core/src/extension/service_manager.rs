//! Service lifecycle manager for background plugin services
//!
//! This module provides the ServiceManager which handles starting, stopping, and
//! tracking the state of background services registered by plugins.
//!
//! # Architecture
//!
//! ```text
//! ServiceManager
//! ├── services: HashMap<String, ServiceInfo>  (running service state)
//! └── Methods:
//!     ├── start_service()      - Start a registered service
//!     ├── stop_service()       - Stop a running service
//!     ├── get_service()        - Query service status
//!     ├── list_services()      - List all services
//!     └── stop_plugin_services() - Stop all services for a plugin
//! ```
//!
//! # Service ID Format
//!
//! Services are identified by a composite key: `"{plugin_id}:{service_id}"`.
//! For example, a service "background-worker" from plugin "my-plugin" would have
//! the key `"my-plugin:background-worker"`.
//!
//! # Service Lifecycle
//!
//! ```text
//! Stopped → Starting → Running
//!     ↑                   │
//!     └── Stopping ←──────┘
//!            │
//!            └── Failed (on error)
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::extension::{ServiceManager, PluginLoader, ServiceRegistration};
//!
//! let mut manager = ServiceManager::new();
//! let mut loader = PluginLoader::new();
//!
//! // Start a service
//! let info = manager.start_service(&registration, &mut loader).await?;
//!
//! // Check service status
//! if let Some(info) = manager.get_service("my-plugin", "worker") {
//!     println!("Service state: {:?}", info.state);
//! }
//!
//! // Stop a service
//! let info = manager.stop_service(&registration, &mut loader).await?;
//! ```

use std::collections::HashMap;

use chrono::Utc;
use tracing::{debug, error, info, warn};

use super::error::{ExtensionError, ExtensionResult};
use super::plugin_loader::PluginLoader;
use super::registry::ServiceRegistration;
use super::types::{ServiceInfo, ServiceResult, ServiceState};

/// Manages background service lifecycle for plugins.
///
/// The ServiceManager tracks the state of all running background services
/// and provides methods to start, stop, and query them.
///
/// # Thread Safety
///
/// ServiceManager methods require mutable access (`&mut self`) because they
/// modify internal state. When used in a concurrent context (like ExtensionManager),
/// wrap in an appropriate synchronization primitive (RwLock, Mutex).
pub struct ServiceManager {
    /// Running services by service_id (format: "plugin_id:service_id")
    services: HashMap<String, ServiceInfo>,
}

impl ServiceManager {
    /// Create a new service manager.
    pub fn new() -> Self {
        Self {
            services: HashMap::new(),
        }
    }

    /// Build a composite service key from plugin_id and service_id.
    fn make_key(plugin_id: &str, service_id: &str) -> String {
        format!("{}:{}", plugin_id, service_id)
    }

    /// Start a service.
    ///
    /// This method:
    /// 1. Creates a composite service key
    /// 2. Checks if the service is already running (returns existing info if so)
    /// 3. Sets state to Starting
    /// 4. Calls the plugin's start_handler via the loader
    /// 5. Sets state to Running or Failed based on result
    ///
    /// # Arguments
    ///
    /// * `registration` - The service registration containing handler names
    /// * `loader` - The plugin loader to invoke handlers through
    ///
    /// # Returns
    ///
    /// * `Ok(ServiceInfo)` - The service info after starting (or existing info if already running)
    /// * `Err(ExtensionError)` - If the service failed to start
    pub fn start_service(
        &mut self,
        registration: &ServiceRegistration,
        loader: &mut PluginLoader,
    ) -> ExtensionResult<ServiceInfo> {
        let key = Self::make_key(&registration.plugin_id, &registration.id);

        // Check if already running
        if let Some(existing) = self.services.get(&key) {
            if existing.state == ServiceState::Running {
                debug!(
                    "Service {} is already running, returning existing info",
                    key
                );
                return Ok(existing.clone());
            }
        }

        // Create initial service info in Starting state
        let mut info = ServiceInfo {
            id: registration.id.clone(),
            plugin_id: registration.plugin_id.clone(),
            name: registration.name.clone(),
            state: ServiceState::Starting,
            started_at: None,
            error: None,
        };

        // Store the starting state
        self.services.insert(key.clone(), info.clone());
        info!("Starting service: {} ({})", registration.name, key);

        // Call the start handler
        let result: Result<serde_json::Value, ExtensionError> = loader.call_tool(
            &registration.plugin_id,
            &registration.start_handler,
            serde_json::json!({}),
        );

        match result {
            Ok(value) => {
                // Try to parse as ServiceResult for detailed info
                let service_result: ServiceResult =
                    serde_json::from_value(value.clone()).unwrap_or_else(|_| ServiceResult::ok());

                if service_result.success {
                    info.state = ServiceState::Running;
                    info.started_at = Some(Utc::now());
                    info.error = None;
                    info!("Service started successfully: {}", key);
                } else {
                    let error_msg = service_result
                        .message
                        .unwrap_or_else(|| "Start handler returned failure".to_string());
                    info.state = ServiceState::Failed;
                    info.error = Some(error_msg.clone());
                    error!("Service failed to start: {} - {}", key, error_msg);
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                info.state = ServiceState::Failed;
                info.error = Some(error_msg.clone());
                error!("Service start handler failed: {} - {}", key, error_msg);
            }
        }

        // Update stored info
        self.services.insert(key, info.clone());

        Ok(info)
    }

    /// Stop a service.
    ///
    /// This method:
    /// 1. Gets the current service info (if any)
    /// 2. Sets state to Stopping
    /// 3. Calls the plugin's stop_handler via the loader
    /// 4. Sets state to Stopped or Failed based on result
    ///
    /// # Arguments
    ///
    /// * `registration` - The service registration containing handler names
    /// * `loader` - The plugin loader to invoke handlers through
    ///
    /// # Returns
    ///
    /// * `Ok(ServiceInfo)` - The service info after stopping
    /// * `Err(ExtensionError)` - If the service was not found or failed to stop
    pub fn stop_service(
        &mut self,
        registration: &ServiceRegistration,
        loader: &mut PluginLoader,
    ) -> ExtensionResult<ServiceInfo> {
        let key = Self::make_key(&registration.plugin_id, &registration.id);

        // Get existing service info or create one
        let mut info = self.services.get(&key).cloned().unwrap_or_else(|| {
            ServiceInfo {
                id: registration.id.clone(),
                plugin_id: registration.plugin_id.clone(),
                name: registration.name.clone(),
                state: ServiceState::Stopped,
                started_at: None,
                error: None,
            }
        });

        // If already stopped, return early
        if info.state == ServiceState::Stopped {
            debug!("Service {} is already stopped", key);
            return Ok(info);
        }

        // Set to stopping state
        info.state = ServiceState::Stopping;
        self.services.insert(key.clone(), info.clone());
        info!("Stopping service: {} ({})", registration.name, key);

        // Call the stop handler
        let result: Result<serde_json::Value, ExtensionError> = loader.call_tool(
            &registration.plugin_id,
            &registration.stop_handler,
            serde_json::json!({}),
        );

        match result {
            Ok(value) => {
                // Try to parse as ServiceResult for detailed info
                let service_result: ServiceResult =
                    serde_json::from_value(value.clone()).unwrap_or_else(|_| ServiceResult::ok());

                if service_result.success {
                    info.state = ServiceState::Stopped;
                    info.started_at = None;
                    info.error = None;
                    info!("Service stopped successfully: {}", key);
                } else {
                    let error_msg = service_result
                        .message
                        .unwrap_or_else(|| "Stop handler returned failure".to_string());
                    info.state = ServiceState::Failed;
                    info.error = Some(error_msg.clone());
                    warn!("Service stop returned failure: {} - {}", key, error_msg);
                }
            }
            Err(e) => {
                // If the plugin is not found, the service is effectively stopped
                if matches!(e, ExtensionError::PluginNotFound(_)) {
                    info.state = ServiceState::Stopped;
                    info.started_at = None;
                    info.error = None;
                    warn!(
                        "Plugin not found when stopping service {}, marking as stopped",
                        key
                    );
                } else {
                    let error_msg = e.to_string();
                    info.state = ServiceState::Failed;
                    info.error = Some(error_msg.clone());
                    error!("Service stop handler failed: {} - {}", key, error_msg);
                }
            }
        }

        // Update stored info
        self.services.insert(key, info.clone());

        Ok(info)
    }

    /// Get service status.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The plugin ID
    /// * `service_id` - The service ID within the plugin
    ///
    /// # Returns
    ///
    /// * `Some(&ServiceInfo)` - If the service exists
    /// * `None` - If the service has never been started
    pub fn get_service(&self, plugin_id: &str, service_id: &str) -> Option<&ServiceInfo> {
        let key = Self::make_key(plugin_id, service_id);
        self.services.get(&key)
    }

    /// List all services.
    ///
    /// Returns a vector of references to all tracked services,
    /// regardless of their current state.
    pub fn list_services(&self) -> Vec<&ServiceInfo> {
        self.services.values().collect()
    }

    /// List services for a specific plugin.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The plugin ID to filter by
    ///
    /// # Returns
    ///
    /// Vector of references to services belonging to the specified plugin.
    pub fn list_plugin_services(&self, plugin_id: &str) -> Vec<&ServiceInfo> {
        self.services
            .values()
            .filter(|info| info.plugin_id == plugin_id)
            .collect()
    }

    /// Stop all services for a plugin.
    ///
    /// This method is typically called when unloading a plugin to ensure
    /// all its services are properly stopped.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The plugin ID whose services should be stopped
    /// * `registrations` - The service registrations for this plugin
    /// * `loader` - The plugin loader to invoke handlers through
    ///
    /// # Returns
    ///
    /// Vector of ServiceInfo for each stopped service.
    pub fn stop_plugin_services(
        &mut self,
        plugin_id: &str,
        registrations: &[ServiceRegistration],
        loader: &mut PluginLoader,
    ) -> Vec<ServiceInfo> {
        let mut results = Vec::new();

        for registration in registrations {
            if registration.plugin_id == plugin_id {
                match self.stop_service(registration, loader) {
                    Ok(info) => {
                        results.push(info);
                    }
                    Err(e) => {
                        warn!(
                            "Failed to stop service {}:{} - {}",
                            plugin_id, registration.id, e
                        );
                        // Create a failed info entry
                        results.push(ServiceInfo {
                            id: registration.id.clone(),
                            plugin_id: plugin_id.to_string(),
                            name: registration.name.clone(),
                            state: ServiceState::Failed,
                            started_at: None,
                            error: Some(e.to_string()),
                        });
                    }
                }
            }
        }

        results
    }

    /// Get the count of running services.
    pub fn running_count(&self) -> usize {
        self.services
            .values()
            .filter(|info| info.state == ServiceState::Running)
            .count()
    }

    /// Get the total count of tracked services.
    pub fn total_count(&self) -> usize {
        self.services.len()
    }

    /// Clear all service tracking (does not stop services).
    ///
    /// This method should only be used during shutdown or testing.
    /// For normal operation, use `stop_plugin_services` to properly
    /// stop services before clearing.
    pub fn clear(&mut self) {
        self.services.clear();
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_registration(plugin_id: &str, service_id: &str) -> ServiceRegistration {
        ServiceRegistration {
            id: service_id.to_string(),
            name: format!("Test Service {}", service_id),
            start_handler: "startService".to_string(),
            stop_handler: "stopService".to_string(),
            plugin_id: plugin_id.to_string(),
        }
    }

    #[test]
    fn test_service_manager_new() {
        let manager = ServiceManager::new();
        assert_eq!(manager.total_count(), 0);
        assert_eq!(manager.running_count(), 0);
        assert!(manager.list_services().is_empty());
    }

    #[test]
    fn test_service_manager_default() {
        let manager = ServiceManager::default();
        assert_eq!(manager.total_count(), 0);
    }

    #[test]
    fn test_make_key() {
        assert_eq!(
            ServiceManager::make_key("my-plugin", "worker"),
            "my-plugin:worker"
        );
        assert_eq!(
            ServiceManager::make_key("test", "bg-service"),
            "test:bg-service"
        );
    }

    #[test]
    fn test_get_service_not_found() {
        let manager = ServiceManager::new();
        assert!(manager.get_service("nonexistent", "service").is_none());
    }

    #[test]
    fn test_list_services_empty() {
        let manager = ServiceManager::new();
        assert!(manager.list_services().is_empty());
    }

    #[test]
    fn test_list_plugin_services_empty() {
        let manager = ServiceManager::new();
        assert!(manager.list_plugin_services("my-plugin").is_empty());
    }

    #[test]
    fn test_clear() {
        let mut manager = ServiceManager::new();

        // Manually insert a service for testing
        let key = ServiceManager::make_key("test-plugin", "test-service");
        manager.services.insert(
            key,
            ServiceInfo {
                id: "test-service".to_string(),
                plugin_id: "test-plugin".to_string(),
                name: "Test Service".to_string(),
                state: ServiceState::Running,
                started_at: Some(Utc::now()),
                error: None,
            },
        );

        assert_eq!(manager.total_count(), 1);
        assert_eq!(manager.running_count(), 1);

        manager.clear();

        assert_eq!(manager.total_count(), 0);
        assert_eq!(manager.running_count(), 0);
    }

    #[test]
    fn test_running_count_with_different_states() {
        let mut manager = ServiceManager::new();

        // Add services in different states
        let states = [
            ("plugin", "s1", ServiceState::Running),
            ("plugin", "s2", ServiceState::Running),
            ("plugin", "s3", ServiceState::Stopped),
            ("plugin", "s4", ServiceState::Failed),
            ("plugin", "s5", ServiceState::Starting),
        ];

        for (plugin_id, service_id, state) in states {
            let key = ServiceManager::make_key(plugin_id, service_id);
            manager.services.insert(
                key,
                ServiceInfo {
                    id: service_id.to_string(),
                    plugin_id: plugin_id.to_string(),
                    name: format!("Service {}", service_id),
                    state,
                    started_at: if state == ServiceState::Running {
                        Some(Utc::now())
                    } else {
                        None
                    },
                    error: if state == ServiceState::Failed {
                        Some("Test error".to_string())
                    } else {
                        None
                    },
                },
            );
        }

        assert_eq!(manager.total_count(), 5);
        assert_eq!(manager.running_count(), 2);
    }

    #[test]
    fn test_list_plugin_services_filters_correctly() {
        let mut manager = ServiceManager::new();

        // Add services from different plugins
        let services = [
            ("plugin-a", "s1"),
            ("plugin-a", "s2"),
            ("plugin-b", "s1"),
            ("plugin-c", "s1"),
        ];

        for (plugin_id, service_id) in services {
            let key = ServiceManager::make_key(plugin_id, service_id);
            manager.services.insert(
                key,
                ServiceInfo {
                    id: service_id.to_string(),
                    plugin_id: plugin_id.to_string(),
                    name: format!("Service {}", service_id),
                    state: ServiceState::Running,
                    started_at: Some(Utc::now()),
                    error: None,
                },
            );
        }

        let plugin_a_services = manager.list_plugin_services("plugin-a");
        assert_eq!(plugin_a_services.len(), 2);
        for info in plugin_a_services {
            assert_eq!(info.plugin_id, "plugin-a");
        }

        let plugin_b_services = manager.list_plugin_services("plugin-b");
        assert_eq!(plugin_b_services.len(), 1);
        assert_eq!(plugin_b_services[0].plugin_id, "plugin-b");

        let plugin_d_services = manager.list_plugin_services("plugin-d");
        assert!(plugin_d_services.is_empty());
    }

    #[test]
    fn test_get_service_found() {
        let mut manager = ServiceManager::new();

        let key = ServiceManager::make_key("my-plugin", "worker");
        manager.services.insert(
            key,
            ServiceInfo {
                id: "worker".to_string(),
                plugin_id: "my-plugin".to_string(),
                name: "Worker Service".to_string(),
                state: ServiceState::Running,
                started_at: Some(Utc::now()),
                error: None,
            },
        );

        let info = manager.get_service("my-plugin", "worker");
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.id, "worker");
        assert_eq!(info.plugin_id, "my-plugin");
        assert_eq!(info.state, ServiceState::Running);
    }

    #[test]
    fn test_start_service_plugin_not_loaded() {
        let mut manager = ServiceManager::new();
        let mut loader = PluginLoader::new();

        let registration = make_test_registration("nonexistent-plugin", "worker");

        // Should fail because the plugin is not loaded
        let result = manager.start_service(&registration, &mut loader);
        assert!(result.is_ok()); // We return Ok with Failed state

        let info = result.unwrap();
        assert_eq!(info.state, ServiceState::Failed);
        assert!(info.error.is_some());
    }

    #[test]
    fn test_stop_service_not_running() {
        let mut manager = ServiceManager::new();
        let mut loader = PluginLoader::new();

        let registration = make_test_registration("test-plugin", "worker");

        // Stopping a service that was never started should return Stopped state
        let result = manager.stop_service(&registration, &mut loader);
        assert!(result.is_ok());

        let info = result.unwrap();
        assert_eq!(info.state, ServiceState::Stopped);
    }

    #[test]
    fn test_stop_plugin_services_empty() {
        let mut manager = ServiceManager::new();
        let mut loader = PluginLoader::new();

        let results = manager.stop_plugin_services("test-plugin", &[], &mut loader);

        assert!(results.is_empty());
    }

    #[test]
    fn test_stop_plugin_services_filters_by_plugin_id() {
        let mut manager = ServiceManager::new();
        let mut loader = PluginLoader::new();

        // Create registrations for different plugins
        let registrations = vec![
            make_test_registration("plugin-a", "service1"),
            make_test_registration("plugin-b", "service1"), // Different plugin
            make_test_registration("plugin-a", "service2"),
        ];

        // Stop services for plugin-a only
        let results = manager.stop_plugin_services("plugin-a", &registrations, &mut loader);

        // Should only stop the two plugin-a services
        assert_eq!(results.len(), 2);
        for info in results {
            assert_eq!(info.plugin_id, "plugin-a");
        }
    }
}
