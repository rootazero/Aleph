use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpHarnessInfo {
    pub id: String,
    pub display_name: String,
    pub executable: String,
    pub mode: String,
    pub enabled: bool,
    pub available: bool,
    pub preset: Option<String>,
    pub config: AcpHarnessConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpHarnessConfig {
    #[serde(default)]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, alias = "mode")]
    pub default_mode: String,
    #[serde(default)]
    pub output_format: serde_json::Value,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpTestResult {
    pub success: bool,
    pub message: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpPresetMeta {
    pub id: String,
    pub display_name: String,
    pub executable: String,
    pub default_mode: String,
    pub trust_level: String,
}

pub struct AcpApi;

impl AcpApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<AcpHarnessInfo>, String> {
        let result = state.rpc_call("acp.list", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse ACP harness list: {}", e))
    }

    pub async fn get(state: &DashboardState, id: &str) -> Result<AcpHarnessInfo, String> {
        let result = state
            .rpc_call("acp.get", serde_json::json!({ "id": id }))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse ACP harness: {}", e))
    }

    pub async fn create(
        state: &DashboardState,
        id: &str,
        config: &AcpHarnessConfig,
    ) -> Result<AcpHarnessInfo, String> {
        let result = state
            .rpc_call(
                "acp.create",
                serde_json::json!({ "id": id, "config": config }),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to create ACP harness: {}", e))
    }

    pub async fn update(
        state: &DashboardState,
        id: &str,
        config: &AcpHarnessConfig,
    ) -> Result<AcpHarnessInfo, String> {
        let result = state
            .rpc_call(
                "acp.update",
                serde_json::json!({ "id": id, "config": config }),
            )
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to update ACP harness: {}", e))
    }

    pub async fn delete(state: &DashboardState, id: &str) -> Result<(), String> {
        state
            .rpc_call("acp.delete", serde_json::json!({ "id": id }))
            .await?;
        Ok(())
    }

    pub async fn test(state: &DashboardState, id: &str) -> Result<AcpTestResult, String> {
        let result = state
            .rpc_call("acp.test", serde_json::json!({ "id": id }))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse test result: {}", e))
    }

    pub async fn set_enabled(
        state: &DashboardState,
        id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        state
            .rpc_call(
                "acp.set_enabled",
                serde_json::json!({ "id": id, "enabled": enabled }),
            )
            .await?;
        Ok(())
    }

    pub async fn presets(state: &DashboardState) -> Result<serde_json::Value, String> {
        let result = state.rpc_call("acp.presets", Value::Null).await?;
        Ok(result)
    }

    pub async fn presets_meta(state: &DashboardState) -> Result<Vec<AcpPresetMeta>, String> {
        let result = state.rpc_call("acp.presets_meta", Value::Null).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse preset metadata: {}", e))
    }

    /// Get the top-level ACP enabled state from config.
    /// Uses config.get since acp.* handlers may not be registered when ACP is disabled.
    pub async fn get_acp_enabled(state: &DashboardState) -> Result<bool, String> {
        let result = state
            .rpc_call("config.get", serde_json::json!({ "key": "acp.enabled" }))
            .await;
        match result {
            Ok(val) => Ok(val.as_bool().unwrap_or(true)),
            Err(_) => Ok(true), // default to enabled
        }
    }

    /// Set the top-level ACP enabled state via config.patch.
    /// Uses config.patch since acp.* handlers may not be registered when ACP is disabled.
    pub async fn set_acp_enabled(state: &DashboardState, enabled: bool) -> Result<(), String> {
        state
            .rpc_call(
                "config.patch",
                serde_json::json!({
                    "path": "acp",
                    "patch": { "enabled": enabled }
                }),
            )
            .await?;
        Ok(())
    }
}
