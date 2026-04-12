use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub nav_timeout_secs: u64,
    pub action_timeout_secs: u64,
    pub persistent_sessions: bool,
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

// ============================================================================
// Browser Runtime API
// ============================================================================

/// Status of one runtime component (fnm / Node / CLI / Chromium / skills).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ComponentStatus {
    Installed {
        version: Option<String>,
        path: Option<String>,
    },
    Missing,
    Probing,
    Error {
        message: String,
    },
}

/// Combined runtime status returned by `browser.runtime_status`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RuntimeStatusResponse {
    pub fnm: ComponentStatus,
    pub node: ComponentStatus,
    pub playwright_cli: ComponentStatus,
    pub chromium: ComponentStatus,
    pub skills: ComponentStatus,
}

pub struct BrowserRuntimeApi;

impl BrowserRuntimeApi {
    pub async fn status(state: &DashboardState) -> Result<RuntimeStatusResponse, String> {
        let v = state
            .rpc_call("browser.runtime_status", Value::Null)
            .await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn refresh(state: &DashboardState) -> Result<RuntimeStatusResponse, String> {
        let v = state
            .rpc_call("browser.refresh_runtime", Value::Null)
            .await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn install(state: &DashboardState) -> Result<(), String> {
        let _ = state
            .rpc_call("browser.install_runtime", serde_json::json!({}))
            .await?;
        Ok(())
    }
}
