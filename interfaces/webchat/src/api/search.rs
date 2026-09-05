use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    /// SearXNG only — comma-separated upstream engines to pin (e.g. "bing").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engines: Option<String>,
    /// A key is stored in the vault (reported by get; the secret is never echoed).
    #[serde(default)]
    pub has_api_key: bool,
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

/// What a `search_config.update` / `deleteBackend` save actually did about
/// the RUNNING process — persisted is not the same as applied.
///
/// `reload_impact` is the server's verified verdict
/// (`config::live_apply::classify_verified`): `"live"` when the rebuilt
/// registry was swapped onto the running tool, `"restart"` when the change
/// only reached disk. Absent when talking to a server older than the field;
/// treated as unknown, never as `"live"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchUpdateOutcome {
    pub success: bool,
    #[serde(default)]
    pub reload_impact: Option<String>,
}

impl SearchUpdateOutcome {
    /// True only when the server explicitly said the change did NOT hot-apply.
    pub fn needs_restart(&self) -> bool {
        self.reload_impact.as_deref() == Some("restart")
    }
}

pub struct SearchConfigApi;

impl SearchConfigApi {
    pub async fn get(state: &DashboardState) -> Result<SearchConfig, String> {
        let result = state.rpc_call("search_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(
        state: &DashboardState,
        config: SearchConfig,
    ) -> Result<SearchUpdateOutcome, String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        let result = state.rpc_call("search_config.update", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn test_connection(
        state: &DashboardState,
        name: &str,
        api_key: Option<String>,
        base_url: Option<String>,
        engine_id: Option<String>,
        engines: Option<String>,
    ) -> Result<SearchTestResult, String> {
        let params = serde_json::json!({
            "name": name,
            "api_key": api_key,
            "base_url": base_url,
            "engine_id": engine_id,
            "engines": engines,
        });
        let result = state.rpc_call("search_config.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn delete_backend(state: &DashboardState, name: &str) -> Result<(), String> {
        let params = serde_json::json!({ "name": name });
        state
            .rpc_call("search_config.deleteBackend", params)
            .await?;
        Ok(())
    }
}
