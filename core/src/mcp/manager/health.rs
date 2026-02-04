//! MCP Manager Health Check Logic
//!
//! Utilities for checking server health and processing results.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use crate::mcp::McpClient;

use super::actor::HealthCheckConfig;
use super::types::{HealthStatus, McpManagerEvent, ServerHealth};

/// Result of a health check on a single server
#[derive(Debug)]
pub struct HealthCheckResult {
    /// Server ID that was checked
    pub server_id: String,
    /// Server name for events
    pub server_name: String,
    /// Whether the check passed
    pub healthy: bool,
    /// Error message if unhealthy
    pub error: Option<String>,
}

/// Run health check on a single MCP client
///
/// Attempts to list tools with a timeout. If successful, the server is healthy.
/// If it times out or fails, the server is unhealthy.
pub async fn check_client_health(
    server_id: &str,
    server_name: &str,
    client: &Arc<McpClient>,
    timeout: Duration,
) -> HealthCheckResult {
    let check = tokio::time::timeout(timeout, client.list_tools()).await;

    match check {
        Ok(tools) => {
            tracing::trace!(
                server_id = %server_id,
                tool_count = tools.len(),
                "Health check passed"
            );
            HealthCheckResult {
                server_id: server_id.to_string(),
                server_name: server_name.to_string(),
                healthy: true,
                error: None,
            }
        }
        Err(_timeout) => {
            tracing::warn!(server_id = %server_id, "Health check timed out");
            HealthCheckResult {
                server_id: server_id.to_string(),
                server_name: server_name.to_string(),
                healthy: false,
                error: Some("Health check timed out".to_string()),
            }
        }
    }
}

/// Process a health check result and update server health state
///
/// Returns an event if the health state change warrants notification
/// (e.g., server needs restart, server is dead).
pub fn process_health_result(
    health: &mut ServerHealth,
    result: &HealthCheckResult,
    config: &HealthCheckConfig,
) -> Option<McpManagerEvent> {
    // Reset restart window if expired
    health.maybe_reset_window(config.restart_window.as_secs());

    if result.healthy {
        health.record_success();
        return None;
    }

    // Record the failure
    health.record_failure(result.error.as_deref().unwrap_or("Unknown error"));

    // Check if we've hit the configured failure threshold
    // Note: record_failure uses its own hardcoded threshold (5), but we trigger actions
    // based on the config's max_failures. We need to force Unhealthy status when
    // we hit max_failures to make should_restart work correctly.
    let should_take_action = health.consecutive_failures >= config.max_failures;

    if !should_take_action {
        // Still degraded, log and continue
        tracing::debug!(
            server_id = %result.server_id,
            failures = health.consecutive_failures,
            max_failures = config.max_failures,
            "Server degraded"
        );
        return None;
    }

    // Force unhealthy status so should_restart logic works correctly
    health.status = HealthStatus::Unhealthy;

    // We've hit the failure threshold - check if we should restart or mark dead
    if health.should_restart(config.max_restarts, config.restart_window.as_secs()) {
        health.mark_restarting();
        Some(McpManagerEvent::ServerRestarting {
            server_id: result.server_id.clone(),
            server_name: result.server_name.clone(),
            attempt: health.restart_count,
        })
    } else {
        // Max restarts exceeded
        health.mark_dead();
        Some(McpManagerEvent::ServerCrashed {
            server_id: result.server_id.clone(),
            server_name: result.server_name.clone(),
            error: "Max restart attempts exceeded".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_health_result_success() {
        let config = HealthCheckConfig::default();
        let mut health = ServerHealth::default();
        health.record_failure("previous error");
        health.record_failure("another error");

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            server_name: "Test Server".to_string(),
            healthy: true,
            error: None,
        };

        let event = process_health_result(&mut health, &result, &config);

        assert!(event.is_none());
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_process_health_result_degraded() {
        let config = HealthCheckConfig::default();
        let mut health = ServerHealth::healthy();

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            server_name: "Test Server".to_string(),
            healthy: false,
            error: Some("timeout".to_string()),
        };

        let event = process_health_result(&mut health, &result, &config);

        // First failure doesn't trigger event (default max_failures is 3)
        assert!(event.is_none());
        assert_eq!(health.consecutive_failures, 1);
    }

    #[test]
    fn test_process_health_result_triggers_restart() {
        let config = HealthCheckConfig {
            max_failures: 2,
            ..Default::default()
        };
        let mut health = ServerHealth::healthy();

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            server_name: "Test Server".to_string(),
            healthy: false,
            error: Some("timeout".to_string()),
        };

        // First failure
        process_health_result(&mut health, &result, &config);

        // Second failure - should trigger restart
        let event = process_health_result(&mut health, &result, &config);

        assert!(matches!(
            event,
            Some(McpManagerEvent::ServerRestarting { attempt: 1, .. })
        ));
    }

    #[test]
    fn test_process_health_result_max_restarts_exceeded() {
        let config = HealthCheckConfig {
            max_failures: 1,
            max_restarts: 1,
            ..Default::default()
        };
        let mut health = ServerHealth::healthy();
        health.restart_count = 1; // Already used up one restart
        health.restart_window_start = Some(std::time::Instant::now());

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            server_name: "Test Server".to_string(),
            healthy: false,
            error: Some("timeout".to_string()),
        };

        let event = process_health_result(&mut health, &result, &config);

        assert!(matches!(event, Some(McpManagerEvent::ServerCrashed { .. })));
        assert_eq!(health.status, HealthStatus::Dead);
    }

    #[test]
    fn test_process_health_result_multiple_failures_before_action() {
        let config = HealthCheckConfig {
            max_failures: 3,
            ..Default::default()
        };
        let mut health = ServerHealth::healthy();

        let result = HealthCheckResult {
            server_id: "test".to_string(),
            server_name: "Test Server".to_string(),
            healthy: false,
            error: Some("timeout".to_string()),
        };

        // First two failures should not trigger action
        let event1 = process_health_result(&mut health, &result, &config);
        assert!(event1.is_none());
        assert_eq!(health.consecutive_failures, 1);

        let event2 = process_health_result(&mut health, &result, &config);
        assert!(event2.is_none());
        assert_eq!(health.consecutive_failures, 2);

        // Third failure should trigger restart
        let event3 = process_health_result(&mut health, &result, &config);
        assert!(matches!(
            event3,
            Some(McpManagerEvent::ServerRestarting { .. })
        ));
    }

    #[test]
    fn test_health_check_result_debug() {
        let result = HealthCheckResult {
            server_id: "test".to_string(),
            server_name: "Test Server".to_string(),
            healthy: true,
            error: None,
        };

        // Ensure Debug is implemented
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("healthy: true"));
    }
}
