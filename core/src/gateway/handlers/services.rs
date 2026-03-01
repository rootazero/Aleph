//! Services RPC Handlers
//!
//! Handlers for background service lifecycle management: start, stop, list, status.
//!
//! Services are background processes registered by plugins that can be started
//! and stopped on demand. Each service is identified by a composite key of
//! `{plugin_id}:{service_id}`.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use super::plugins::get_extension_manager;
#[cfg(test)]
use super::plugins::is_extension_manager_initialized;
use crate::extension::{ServiceInfo, ServiceState};

// ============================================================================
// Response Types
// ============================================================================

/// Service info for JSON serialization
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfoJson {
    pub id: String,
    pub plugin_id: String,
    pub name: String,
    pub state: String,
    pub started_at: Option<String>,
    pub error: Option<String>,
}

impl From<ServiceInfo> for ServiceInfoJson {
    fn from(info: ServiceInfo) -> Self {
        Self {
            id: info.id,
            plugin_id: info.plugin_id,
            name: info.name,
            state: match info.state {
                ServiceState::Stopped => "stopped".to_string(),
                ServiceState::Starting => "starting".to_string(),
                ServiceState::Running => "running".to_string(),
                ServiceState::Stopping => "stopping".to_string(),
                ServiceState::Failed => "failed".to_string(),
            },
            started_at: info.started_at.map(|t| t.to_rfc3339()),
            error: info.error,
        }
    }
}

// ============================================================================
// Start Service
// ============================================================================

/// Parameters for services.start
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartParams {
    /// ID of the plugin that registered the service
    pub plugin_id: String,
    /// ID of the service to start
    pub service_id: String,
}

/// Start a background service
///
/// This handler starts a service that was registered by a plugin. The service
/// must be registered in the plugin registry before it can be started.
///
/// # Params
/// - `pluginId`: ID of the plugin that registered the service
/// - `serviceId`: ID of the service to start
///
/// # Returns
/// - `service`: ServiceInfo object with current state
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized or service failed to start
/// - `INVALID_PARAMS`: Missing or invalid parameters
pub async fn handle_start(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: StartParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Start the service
    match manager.start_service(&params.plugin_id, &params.service_id).await {
        Ok(info) => {
            let info_json = ServiceInfoJson::from(info);
            JsonRpcResponse::success(request.id, json!({ "service": info_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to start service: {}", e),
        ),
    }
}

// ============================================================================
// Stop Service
// ============================================================================

/// Parameters for services.stop
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopParams {
    /// ID of the plugin that registered the service
    pub plugin_id: String,
    /// ID of the service to stop
    pub service_id: String,
}

/// Stop a running background service
///
/// This handler stops a service that is currently running. If the service
/// is already stopped, it returns the current state.
///
/// # Params
/// - `pluginId`: ID of the plugin that registered the service
/// - `serviceId`: ID of the service to stop
///
/// # Returns
/// - `service`: ServiceInfo object with current state
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized or service failed to stop
/// - `INVALID_PARAMS`: Missing or invalid parameters
pub async fn handle_stop(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: StopParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Stop the service
    match manager.stop_service(&params.plugin_id, &params.service_id).await {
        Ok(info) => {
            let info_json = ServiceInfoJson::from(info);
            JsonRpcResponse::success(request.id, json!({ "service": info_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to stop service: {}", e),
        ),
    }
}

// ============================================================================
// List Services
// ============================================================================

/// Parameters for services.list (optional filtering)
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    /// Filter by plugin ID (optional)
    #[serde(default)]
    pub plugin_id: Option<String>,
    /// Filter by state (optional)
    #[serde(default)]
    pub state: Option<String>,
}

/// List all services
///
/// This handler returns information about all services that have been tracked
/// by the service manager. Optional filtering by plugin ID or state.
///
/// # Params (optional)
/// - `pluginId`: Filter by plugin ID
/// - `state`: Filter by state (stopped, starting, running, stopping, failed)
///
/// # Returns
/// - `services`: Array of ServiceInfo objects
/// - `total`: Total count of matching services
/// - `running`: Count of running services
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized
/// - `INVALID_PARAMS`: Invalid filter parameters
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: ListParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => ListParams::default(),
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Get all services
    let services = manager.list_services().await;

    // Apply filters
    let filtered: Vec<ServiceInfoJson> = services
        .into_iter()
        .filter(|info| {
            // Filter by plugin_id if specified
            if let Some(ref plugin_id) = params.plugin_id {
                if &info.plugin_id != plugin_id {
                    return false;
                }
            }
            // Filter by state if specified
            if let Some(ref state) = params.state {
                let state_str = match info.state {
                    ServiceState::Stopped => "stopped",
                    ServiceState::Starting => "starting",
                    ServiceState::Running => "running",
                    ServiceState::Stopping => "stopping",
                    ServiceState::Failed => "failed",
                };
                if state_str != state.as_str() {
                    return false;
                }
            }
            true
        })
        .map(ServiceInfoJson::from)
        .collect();

    let running_count = filtered.iter().filter(|s| s.state == "running").count();
    let total = filtered.len();

    JsonRpcResponse::success(
        request.id,
        json!({
            "services": filtered,
            "total": total,
            "running": running_count,
        }),
    )
}

// ============================================================================
// Get Service Status
// ============================================================================

/// Parameters for services.status
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusParams {
    /// ID of the plugin that registered the service
    pub plugin_id: String,
    /// ID of the service
    pub service_id: String,
}

/// Get the status of a specific service
///
/// This handler returns the current state of a service. If the service
/// has never been started, returns null.
///
/// # Params
/// - `pluginId`: ID of the plugin that registered the service
/// - `serviceId`: ID of the service
///
/// # Returns
/// - `service`: ServiceInfo object or null if service not found
///
/// # Errors
/// - `INTERNAL_ERROR`: Extension manager not initialized
/// - `INVALID_PARAMS`: Missing or invalid parameters
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: StatusParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get the extension manager from global state
    let manager = match get_extension_manager() {
        Ok(m) => m,
        Err(e) => return e.with_id(request.id),
    };

    // Get service status
    match manager.get_service_status(&params.plugin_id, &params.service_id).await {
        Some(info) => {
            let info_json = ServiceInfoJson::from(info);
            JsonRpcResponse::success(request.id, json!({ "service": info_json }))
        }
        None => {
            // Service has never been started - return null (not an error)
            JsonRpcResponse::success(request.id, json!({ "service": null }))
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::ExtensionManager;
    use crate::gateway::handlers::plugins::init_extension_manager;
    use crate::sync_primitives::Arc;

    #[test]
    fn test_start_params() {
        let json = json!({
            "pluginId": "my-plugin",
            "serviceId": "worker"
        });
        let params: StartParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, "my-plugin");
        assert_eq!(params.service_id, "worker");
    }

    #[test]
    fn test_stop_params() {
        let json = json!({
            "pluginId": "test-plugin",
            "serviceId": "background-task"
        });
        let params: StopParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, "test-plugin");
        assert_eq!(params.service_id, "background-task");
    }

    #[test]
    fn test_list_params_default() {
        let json = json!({});
        let params: ListParams = serde_json::from_value(json).unwrap();
        assert!(params.plugin_id.is_none());
        assert!(params.state.is_none());
    }

    #[test]
    fn test_list_params_with_filters() {
        let json = json!({
            "pluginId": "my-plugin",
            "state": "running"
        });
        let params: ListParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, Some("my-plugin".to_string()));
        assert_eq!(params.state, Some("running".to_string()));
    }

    #[test]
    fn test_status_params() {
        let json = json!({
            "pluginId": "test",
            "serviceId": "monitor"
        });
        let params: StatusParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.plugin_id, "test");
        assert_eq!(params.service_id, "monitor");
    }

    #[test]
    fn test_service_info_json_from() {
        let info = ServiceInfo {
            id: "worker".to_string(),
            plugin_id: "my-plugin".to_string(),
            name: "Background Worker".to_string(),
            state: ServiceState::Running,
            started_at: Some(chrono::Utc::now()),
            error: None,
        };

        let json = ServiceInfoJson::from(info);
        assert_eq!(json.id, "worker");
        assert_eq!(json.plugin_id, "my-plugin");
        assert_eq!(json.name, "Background Worker");
        assert_eq!(json.state, "running");
        assert!(json.started_at.is_some());
        assert!(json.error.is_none());
    }

    #[test]
    fn test_service_info_json_failed() {
        let info = ServiceInfo {
            id: "broken".to_string(),
            plugin_id: "test".to_string(),
            name: "Broken Service".to_string(),
            state: ServiceState::Failed,
            started_at: None,
            error: Some("Connection refused".to_string()),
        };

        let json = ServiceInfoJson::from(info);
        assert_eq!(json.state, "failed");
        assert_eq!(json.error, Some("Connection refused".to_string()));
    }

    #[tokio::test]
    async fn test_handle_start_missing_params() {
        let request = JsonRpcRequest::with_id("services.start", None, json!(1));
        let response = handle_start(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Missing params"));
    }

    #[tokio::test]
    async fn test_handle_start_invalid_params() {
        let request = JsonRpcRequest::new(
            "services.start",
            Some(json!({"invalid": "params"})),
            Some(json!(1)),
        );
        let response = handle_start(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_stop_missing_params() {
        let request = JsonRpcRequest::with_id("services.stop", None, json!(1));
        let response = handle_stop(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Missing params"));
    }

    #[tokio::test]
    async fn test_handle_stop_invalid_params() {
        let request = JsonRpcRequest::new(
            "services.stop",
            Some(json!({"pluginId": "test"})), // Missing serviceId
            Some(json!(1)),
        );
        let response = handle_stop(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_status_missing_params() {
        let request = JsonRpcRequest::with_id("services.status", None, json!(1));
        let response = handle_status(request).await;

        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INVALID_PARAMS);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Missing params"));
    }

    #[tokio::test]
    async fn test_handle_list_no_params() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::with_id("services.list", None, json!(1));
        let response = handle_list(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("services").is_some());
        assert!(result.get("total").is_some());
        assert!(result.get("running").is_some());
    }

    #[tokio::test]
    async fn test_handle_list_with_filters() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "services.list",
            Some(json!({
                "pluginId": "nonexistent",
                "state": "running"
            })),
            Some(json!(1)),
        );
        let response = handle_list(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let services = result.get("services").unwrap().as_array().unwrap();
        assert!(services.is_empty()); // No services match the filter
    }

    #[tokio::test]
    async fn test_handle_status_not_found() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "services.status",
            Some(json!({
                "pluginId": "nonexistent",
                "serviceId": "worker"
            })),
            Some(json!(1)),
        );
        let response = handle_status(request).await;

        // Should return success with null service (not an error)
        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("service").unwrap().is_null());
    }

    #[tokio::test]
    async fn test_handle_start_service_not_registered() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "services.start",
            Some(json!({
                "pluginId": "nonexistent-plugin",
                "serviceId": "nonexistent-service"
            })),
            Some(json!(1)),
        );
        let response = handle_start(request).await;

        // Should return an error because the service is not registered
        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Failed to start service"));
    }

    #[tokio::test]
    async fn test_handle_stop_service_not_registered() {
        // Initialize manager if not already done
        if !is_extension_manager_initialized() {
            let manager = ExtensionManager::with_defaults().await.unwrap();
            let _ = init_extension_manager(Arc::new(manager));
        }

        let request = JsonRpcRequest::new(
            "services.stop",
            Some(json!({
                "pluginId": "nonexistent-plugin",
                "serviceId": "nonexistent-service"
            })),
            Some(json!(1)),
        );
        let response = handle_stop(request).await;

        // Should return an error because the service is not registered
        assert!(response.is_error());
        assert_eq!(response.error.as_ref().unwrap().code, INTERNAL_ERROR);
        assert!(response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("Failed to stop service"));
    }
}
