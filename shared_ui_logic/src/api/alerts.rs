//! Alerts API
//!
//! Provides methods for monitoring system health, memory status, and real-time alerts.

use crate::connection::AlephConnector;
use crate::protocol::{RpcClient, RpcError};
use serde::{Deserialize, Serialize};

/// System health status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// System is operating normally
    Healthy,
    /// System is degraded but operational
    Degraded,
    /// System is experiencing issues
    Unhealthy,
}

/// System health data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealthData {
    /// Current health status
    pub status: HealthStatus,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Optional error message if unhealthy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Memory usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatusData {
    /// Total number of memories stored
    pub total_memories: i64,
    /// Database size in megabytes
    pub database_size_mb: f64,
    /// Optional warning threshold (percentage)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_threshold: Option<f64>,
}

/// Alert severity level
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    /// Informational alert
    Info,
    /// Warning alert
    Warning,
    /// Error alert
    Error,
    /// Critical alert
    Critical,
}

/// Real-time alert data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertData {
    /// Alert ID
    pub id: String,
    /// Alert severity
    pub severity: AlertSeverity,
    /// Alert title
    pub title: String,
    /// Alert message
    pub message: String,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Alerts API client
///
/// Provides high-level methods for monitoring system health, memory status,
/// and subscribing to real-time alerts.
///
/// ## Example
///
/// ```rust,ignore
/// use aleph_ui_logic::api::AlertsApi;
/// use aleph_ui_logic::connection::create_connector;
///
/// let connector = create_connector();
/// let alerts = AlertsApi::new(connector);
///
/// // Get system health
/// let health = alerts.get_system_health().await?;
/// println!("System status: {:?}", health.status);
///
/// // Get memory status
/// let memory = alerts.get_memory_status().await?;
/// println!("Total memories: {}", memory.total_memories);
/// ```
pub struct AlertsApi<C: AlephConnector> {
    client: RpcClient<C>,
}

impl<C: AlephConnector> AlertsApi<C> {
    /// Create a new Alerts API client
    pub fn new(connector: C) -> Self {
        Self {
            client: RpcClient::new(connector),
        }
    }

    /// Get current system health status
    ///
    /// # Returns
    ///
    /// [`SystemHealthData`] containing the current health status and timestamp
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let health = alerts.get_system_health().await?;
    /// match health.status {
    ///     HealthStatus::Healthy => println!("System is healthy"),
    ///     HealthStatus::Degraded => println!("System is degraded"),
    ///     HealthStatus::Unhealthy => println!("System is unhealthy"),
    /// }
    /// ```
    pub async fn get_system_health(&self) -> Result<SystemHealthData, RpcError> {
        #[derive(Deserialize)]
        struct HealthResponse {
            status: String,
            timestamp: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            message: Option<String>,
        }

        let response: HealthResponse = self.client.call("health", ()).await?;

        // Parse status string to enum
        let status = match response.status.as_str() {
            "healthy" => HealthStatus::Healthy,
            "degraded" => HealthStatus::Degraded,
            "unhealthy" => HealthStatus::Unhealthy,
            _ => HealthStatus::Healthy, // Default to healthy for unknown status
        };

        Ok(SystemHealthData {
            status,
            timestamp: response.timestamp,
            message: response.message,
        })
    }

    /// Get memory usage statistics
    ///
    /// # Returns
    ///
    /// [`MemoryStatusData`] containing memory usage information
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the request fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let memory = alerts.get_memory_status().await?;
    /// println!("Total memories: {}", memory.total_memories);
    /// println!("Database size: {:.2} MB", memory.database_size_mb);
    /// ```
    pub async fn get_memory_status(&self) -> Result<MemoryStatusData, RpcError> {
        #[derive(Deserialize)]
        struct MemoryStatsResponse {
            #[serde(rename = "totalMemories")]
            total_memories: i64,
            #[serde(rename = "databaseSizeMb")]
            database_size_mb: f64,
        }

        let response: MemoryStatsResponse = self.client.call("memory.stats", ()).await?;

        Ok(MemoryStatusData {
            total_memories: response.total_memories,
            database_size_mb: response.database_size_mb,
            warning_threshold: None, // Can be configured later
        })
    }

    /// Subscribe to real-time alert updates
    ///
    /// This method sets up a subscription to receive real-time alerts from the Gateway.
    /// The actual subscription mechanism will be implemented using WebSocket events.
    ///
    /// # Returns
    ///
    /// A subscription ID that can be used to unsubscribe later
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the subscription fails
    ///
    /// # Note
    ///
    /// This is a placeholder for the subscription mechanism. The actual implementation
    /// will be completed in Task 14 (WebSocket subscription mechanism).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let subscription_id = alerts.subscribe_alerts().await?;
    /// println!("Subscribed to alerts: {}", subscription_id);
    /// ```
    pub async fn subscribe_alerts(&self) -> Result<String, RpcError> {
        #[derive(Serialize)]
        struct SubscribeParams {
            event_type: String,
        }

        #[derive(Deserialize)]
        struct SubscribeResponse {
            subscription_id: String,
        }

        let response: SubscribeResponse = self
            .client
            .call(
                "events.subscribe",
                SubscribeParams {
                    event_type: "alerts".to_string(),
                },
            )
            .await?;

        Ok(response.subscription_id)
    }

    /// Unsubscribe from alert updates
    ///
    /// # Arguments
    ///
    /// - `subscription_id`: The subscription ID returned by `subscribe_alerts`
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] if the unsubscribe request fails
    pub async fn unsubscribe_alerts(&self, subscription_id: &str) -> Result<(), RpcError> {
        #[derive(Serialize)]
        struct UnsubscribeParams<'a> {
            subscription_id: &'a str,
        }

        self.client
            .call::<_, ()>(
                "events.unsubscribe",
                UnsubscribeParams { subscription_id },
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_serialization() {
        let status = HealthStatus::Healthy;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"healthy\"");

        let deserialized: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, HealthStatus::Healthy);
    }

    #[test]
    fn test_alert_severity_serialization() {
        let severity = AlertSeverity::Warning;
        let json = serde_json::to_string(&severity).unwrap();
        assert_eq!(json, "\"warning\"");

        let deserialized: AlertSeverity = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, AlertSeverity::Warning);
    }

    #[test]
    fn test_system_health_data_serialization() {
        let health = SystemHealthData {
            status: HealthStatus::Healthy,
            timestamp: "2024-01-15T10:30:00Z".to_string(),
            message: None,
        };

        let json = serde_json::to_string(&health).unwrap();
        let deserialized: SystemHealthData = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.status, HealthStatus::Healthy);
        assert_eq!(deserialized.timestamp, "2024-01-15T10:30:00Z");
    }

    #[test]
    fn test_memory_status_data_serialization() {
        let memory = MemoryStatusData {
            total_memories: 1000,
            database_size_mb: 25.5,
            warning_threshold: Some(80.0),
        };

        let json = serde_json::to_string(&memory).unwrap();
        let deserialized: MemoryStatusData = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.total_memories, 1000);
        assert_eq!(deserialized.database_size_mb, 25.5);
        assert_eq!(deserialized.warning_threshold, Some(80.0));
    }

    #[test]
    fn test_alert_data_serialization() {
        let alert = AlertData {
            id: "alert-123".to_string(),
            severity: AlertSeverity::Error,
            title: "Test Alert".to_string(),
            message: "This is a test alert".to_string(),
            timestamp: "2024-01-15T10:30:00Z".to_string(),
            metadata: Some(serde_json::json!({"key": "value"})),
        };

        let json = serde_json::to_string(&alert).unwrap();
        let deserialized: AlertData = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "alert-123");
        assert_eq!(deserialized.severity, AlertSeverity::Error);
        assert_eq!(deserialized.title, "Test Alert");
    }
}
