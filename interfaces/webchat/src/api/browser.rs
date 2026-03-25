use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

// ============================================================================
// Browser Config API
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub default_driver: String,
    pub browser_engine: String,
    pub headless: bool,
    pub devtools_profile: String,
    pub block_private: bool,
    pub blocked_domains: Vec<String>,
    pub allowed_domains: Vec<String>,
}

pub struct BrowserConfigApi;

impl BrowserConfigApi {
    pub async fn get(state: &DashboardState) -> Result<BrowserConfig, String> {
        let result = state.rpc_call("browser_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: BrowserConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("browser_config.update", params).await?;
        Ok(())
    }
}
