use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub enabled: bool,
    pub default_provider: String,
    pub max_results: u64,
    pub timeout_seconds: u64,
    pub pii_enabled: bool,
    pub pii_scrub_email: bool,
    pub pii_scrub_phone: bool,
    pub pii_scrub_ssn: bool,
    pub pii_scrub_credit_card: bool,
    #[serde(default)]
    pub backends: Vec<SearchBackendEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTestResult {
    pub success: bool,
    pub message: String,
}

pub struct SearchConfigApi;

impl SearchConfigApi {
    pub async fn get(state: &DashboardState) -> Result<SearchConfig, String> {
        let result = state.rpc_call("search_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: SearchConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("search_config.update", params).await?;
        Ok(())
    }

    pub async fn test_connection(
        state: &DashboardState,
        name: &str,
        api_key: Option<String>,
        base_url: Option<String>,
        engine_id: Option<String>,
    ) -> Result<SearchTestResult, String> {
        let params = serde_json::json!({
            "name": name,
            "api_key": api_key,
            "base_url": base_url,
            "engine_id": engine_id,
        });
        let result = state.rpc_call("search_config.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn delete_backend(state: &DashboardState, name: &str) -> Result<(), String> {
        let params = serde_json::json!({ "name": name });
        state.rpc_call("search_config.deleteBackend", params).await?;
        Ok(())
    }
}
