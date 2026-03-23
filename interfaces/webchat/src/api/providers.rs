use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default = "default_provider_color")]
    pub color: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub is_default: bool,
    #[serde(default)]
    pub verified: bool,
}

fn default_provider_color() -> String { "#808080".to_string() }
fn default_timeout() -> u64 { 300 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    pub enabled: bool,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStatus {
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct ProvidersApi;

impl ProvidersApi {
    /// List all providers
    pub async fn list(state: &DashboardState) -> Result<Vec<ProviderInfo>, String> {
        let result = state.rpc_call("providers.list", Value::Null).await?;

        // Extract providers array from result
        result.get("providers")
            .ok_or_else(|| "Invalid response: missing providers".to_string())
            .and_then(|providers| {
                serde_json::from_value(providers.clone())
                    .map_err(|e| format!("Failed to parse providers: {}", e))
            })
    }

    /// Get a specific provider
    pub async fn get(state: &DashboardState, name: String) -> Result<ProviderInfo, String> {
        let params = serde_json::json!({
            "name": name,
        });

        let result = state.rpc_call("providers.get", params).await?;

        // Extract provider from result
        result.get("provider")
            .ok_or_else(|| "Invalid response: missing provider".to_string())
            .and_then(|provider| {
                serde_json::from_value(provider.clone())
                    .map_err(|e| format!("Failed to parse provider: {}", e))
            })
    }

    /// Create a new provider
    pub async fn create(
        state: &DashboardState,
        name: String,
        config: ProviderConfig,
    ) -> Result<(), String> {
        let config_value = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        let params = serde_json::json!({
            "name": name,
            "config": config_value,
        });

        state.rpc_call("providers.create", params).await?;
        Ok(())
    }

    /// Update an existing provider
    pub async fn update(
        state: &DashboardState,
        name: String,
        config: ProviderConfig,
    ) -> Result<(), String> {
        let config_value = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        let params = serde_json::json!({
            "name": name,
            "config": config_value,
        });

        state.rpc_call("providers.update", params).await?;
        Ok(())
    }

    /// Delete a provider
    pub async fn delete(state: &DashboardState, name: String) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
        });

        state.rpc_call("providers.delete", params).await?;
        Ok(())
    }

    /// Set default provider
    pub async fn set_default(state: &DashboardState, name: String) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
        });

        state.rpc_call("providers.setDefault", params).await?;
        Ok(())
    }

    /// Test provider connection
    pub async fn test_connection(
        state: &DashboardState,
        name: Option<&str>,
        config: ProviderConfig,
    ) -> Result<TestResult, String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });

        let result = state.rpc_call("providers.test", params).await?;

        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse test result: {}", e))
    }

    /// Trigger OAuth browser login for a subscription provider
    pub async fn oauth_login(state: &DashboardState, provider: String) -> Result<OAuthStatus, String> {
        let params = serde_json::json!({ "provider": provider });
        let result = state.rpc_call("providers.oauthLogin", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse OAuth status: {}", e))
    }

    /// Clear OAuth token for a subscription provider
    pub async fn oauth_logout(state: &DashboardState, provider: String) -> Result<(), String> {
        let params = serde_json::json!({ "provider": provider });
        state.rpc_call("providers.oauthLogout", params).await?;
        Ok(())
    }

    /// Get OAuth connection status
    pub async fn oauth_status(state: &DashboardState, provider: String) -> Result<OAuthStatus, String> {
        let params = serde_json::json!({ "provider": provider });
        let result = state.rpc_call("providers.oauthStatus", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse OAuth status: {}", e))
    }
}
