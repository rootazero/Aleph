use serde::Deserialize;
use serde_json::Value;
use crate::context::DashboardState;

/// Result from config.reload RPC
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigReloadResult {
    pub ok: bool,
    #[serde(default)]
    pub reloaded: Vec<String>,
    #[serde(default)]
    pub failed: Vec<Value>,
}

pub struct ConfigApi;

impl ConfigApi {
    /// Get configuration value
    pub async fn get(
        state: &DashboardState,
        key: String,
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "key": key,
        });

        state.rpc_call("config.get", params).await
    }

    /// Set configuration value
    pub async fn set(
        state: &DashboardState,
        key: String,
        value: Value,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "key": key,
            "value": value,
        });

        state.rpc_call("config.set", params).await?;
        Ok(())
    }

    /// List all configuration keys
    pub async fn list(state: &DashboardState) -> Result<Vec<String>, String> {
        let result = state.rpc_call("config.list", Value::Null).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse config list: {}", e))
    }

    /// Reload configuration from disk and refresh subsystems
    pub async fn reload(state: &DashboardState) -> Result<ConfigReloadResult, String> {
        let result = state.rpc_call("config.reload", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse reload result: {}", e))
    }
}
