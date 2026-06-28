use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchBackendEntry {
    pub name: String,
    pub provider_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Outbound only (write on update); never present on a get response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// A key is stored in the vault (reported by get; the secret is never echoed).
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default)]
    pub verified: bool,
    /// True for providers that reuse the [search] config (firecrawl).
    #[serde(default)]
    pub shares_search: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchConfig {
    pub enabled: bool,
    pub default_provider: String,
    #[serde(default)]
    pub backends: Vec<FetchBackendEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchTestResult {
    pub success: bool,
    pub message: String,
}

pub struct FetchConfigApi;

impl FetchConfigApi {
    pub async fn get(state: &DashboardState) -> Result<FetchConfig, String> {
        let result = state.rpc_call("fetch_config.get", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn update(state: &DashboardState, config: FetchConfig) -> Result<(), String> {
        let params = serde_json::to_value(&config).map_err(|e| e.to_string())?;
        state.rpc_call("fetch_config.update", params).await?;
        Ok(())
    }

    pub async fn test_connection(
        state: &DashboardState,
        backend: &FetchBackendEntry,
    ) -> Result<FetchTestResult, String> {
        let params = serde_json::to_value(backend).map_err(|e| e.to_string())?;
        let result = state.rpc_call("fetch_config.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_deserializes_get_response() {
        let json = r#"{"enabled":true,"default_provider":"crawl4ai","backends":[{"name":"crawl4ai","provider_type":"crawl4ai","base_url":"http://x:11235","timeout_seconds":60,"has_api_key":true,"verified":false,"shares_search":false}]}"#;
        let cfg: FetchConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.default_provider, "crawl4ai");
        assert!(cfg.backends[0].has_api_key);
        assert_eq!(cfg.backends[0].base_url.as_deref(), Some("http://x:11235"));
    }
}
