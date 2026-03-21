use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

// ============================================================================
// Security Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub require_auth: bool,
    pub enable_pairing: bool,
    pub allow_guest: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
}

pub struct SecurityConfigApi;

impl SecurityConfigApi {
    /// Get current security configuration
    pub async fn get(state: &DashboardState) -> Result<SecurityConfig, String> {
        let result = state.rpc_call("security_config.get", serde_json::Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse security config: {}", e))
    }

    /// Update security configuration
    pub async fn update(
        state: &DashboardState,
        config: SecurityConfig,
    ) -> Result<(), String> {
        let params = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        state.rpc_call("security_config.update", params).await?;
        Ok(())
    }

    /// List all paired devices
    pub async fn list_devices(state: &DashboardState) -> Result<Vec<DeviceInfo>, String> {
        let result = state.rpc_call("security_config.list_devices", serde_json::Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse devices: {}", e))
    }

    /// Revoke a device's access
    pub async fn revoke_device(state: &DashboardState, device_id: String) -> Result<(), String> {
        let params = serde_json::json!({
            "device_id": device_id,
        });

        state.rpc_call("security_config.revoke_device", params).await?;
        Ok(())
    }
}

// ============================================================================
// Auth Token API (UI Token Authentication)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokenInfo {
    pub token: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_used_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionInfo>,
    pub count: u64,
}

pub struct AuthTokenApi;

impl AuthTokenApi {
    /// Show current shared token
    pub async fn show_token(state: &DashboardState) -> Result<AuthTokenInfo, String> {
        let result = state.rpc_call("auth.show_token", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse token info: {}", e))
    }

    /// Regenerate shared token
    pub async fn reset_token(state: &DashboardState) -> Result<AuthTokenInfo, String> {
        let result = state.rpc_call("auth.reset_token", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse token info: {}", e))
    }

    /// List active HTTP sessions
    pub async fn list_sessions(state: &DashboardState) -> Result<SessionListResponse, String> {
        let result = state.rpc_call("auth.list_sessions", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse sessions: {}", e))
    }

    /// Revoke a specific HTTP session
    pub async fn revoke_session(state: &DashboardState, session_id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "session_id": session_id });
        state.rpc_call("auth.revoke_session", params).await?;
        Ok(())
    }
}
