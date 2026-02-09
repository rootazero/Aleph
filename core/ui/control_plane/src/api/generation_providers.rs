use crate::context::DashboardState;
use crate::generation::GenerationType;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderConfig {
    pub provider_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub enabled: bool,
    pub color: String,
    pub capabilities: Vec<GenerationType>,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationProviderEntry {
    pub name: String,
    pub config: GenerationProviderConfig,
    pub is_default_for: Vec<GenerationType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

pub struct GenerationProvidersApi;

impl GenerationProvidersApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<GenerationProviderEntry>, String> {
        let result = state.rpc_call("generation_providers.list", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, name: &str) -> Result<GenerationProviderEntry, String> {
        let params = serde_json::json!({ "name": name });
        let result = state.rpc_call("generation_providers.get", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn create(
        state: &DashboardState,
        name: &str,
        config: GenerationProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });
        state.rpc_call("generation_providers.create", params).await?;
        Ok(())
    }

    pub async fn update(
        state: &DashboardState,
        name: &str,
        config: GenerationProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });
        state.rpc_call("generation_providers.update", params).await?;
        Ok(())
    }

    pub async fn delete(state: &DashboardState, name: &str) -> Result<(), String> {
        let params = serde_json::json!({ "name": name });
        state.rpc_call("generation_providers.delete", params).await?;
        Ok(())
    }

    pub async fn set_default(
        state: &DashboardState,
        name: &str,
        generation_type: GenerationType,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "generation_type": generation_type,
        });
        state.rpc_call("generation_providers.setDefault", params).await?;
        Ok(())
    }

    pub async fn test_connection(
        state: &DashboardState,
        provider_type: &str,
        api_key: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Result<TestConnectionResult, String> {
        let params = serde_json::json!({
            "provider_type": provider_type,
            "api_key": api_key,
            "base_url": base_url,
            "model": model,
        });
        let result = state.rpc_call("generation_providers.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}
