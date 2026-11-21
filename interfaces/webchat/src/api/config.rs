use crate::context::DashboardState;
use serde::Deserialize;
use serde_json::Value;

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
    /// Reload configuration from disk and refresh subsystems
    pub async fn reload(state: &DashboardState) -> Result<ConfigReloadResult, String> {
        let result = state.rpc_call("config.reload", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse reload result: {e}"))
    }
}
