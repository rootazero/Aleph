use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProviderEntry {
    pub id: String,
    pub name: String,
    pub preset: String,
    pub api_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
    pub dimensions: u32,
    #[serde(default)]
    pub batch_size: u32,
    #[serde(default)]
    pub timeout_ms: u64,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub verified: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProviderConfig {
    pub id: String,
    pub name: String,
    pub preset: String,
    pub api_base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub model: String,
    pub dimensions: u32,
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}
fn default_batch_size() -> u32 {
    32
}
fn default_timeout_ms() -> u64 {
    10000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingTestResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingPresetEntry {
    pub preset: String,
    pub id: String,
    pub name: String,
    pub api_base: String,
    pub api_key_env: Option<String>,
    pub model: String,
    pub dimensions: u32,
}

pub struct EmbeddingProvidersApi;

impl EmbeddingProvidersApi {
    /// List all configured embedding providers
    pub async fn list(state: &DashboardState) -> Result<Vec<EmbeddingProviderEntry>, String> {
        let result = state
            .rpc_call("embedding_providers.list", Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Get a single embedding provider by id
    pub async fn get(state: &DashboardState, id: &str) -> Result<EmbeddingProviderEntry, String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("embedding_providers.get", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Add a new embedding provider
    pub async fn add(
        state: &DashboardState,
        config: EmbeddingProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({ "config": config });
        state.rpc_call("embedding_providers.add", params).await?;
        Ok(())
    }

    /// Update an existing embedding provider
    pub async fn update(
        state: &DashboardState,
        id: &str,
        config: EmbeddingProviderConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "id": id,
            "config": config,
        });
        state.rpc_call("embedding_providers.update", params).await?;
        Ok(())
    }

    /// Remove an embedding provider by id
    pub async fn remove(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("embedding_providers.remove", params).await?;
        Ok(())
    }

    /// Set a provider as the active embedding provider.
    ///
    /// Multi-dimension vector columns allow seamless provider switching
    /// without clearing the vector store.
    pub async fn set_active(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state
            .rpc_call("embedding_providers.setActive", params)
            .await?;
        Ok(())
    }

    /// Test an embedding provider's connectivity
    pub async fn test(
        state: &DashboardState,
        id: Option<&str>,
        config: EmbeddingProviderConfig,
    ) -> Result<EmbeddingTestResult, String> {
        let params = serde_json::json!({ "id": id, "config": config });
        let result = state.rpc_call("embedding_providers.test", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Get preset embedding provider configurations
    pub async fn presets(state: &DashboardState) -> Result<Vec<EmbeddingPresetEntry>, String> {
        let result = state
            .rpc_call("embedding_providers.presets", Value::Null)
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Start a reembed migration
    pub async fn reembed(state: &DashboardState, target_dim: Option<u32>) -> Result<Value, String> {
        let params = match target_dim {
            Some(dim) => serde_json::json!({ "target_dim": dim }),
            None => Value::Null,
        };
        state.rpc_call("memory.reembed", params).await
    }

    /// Cancel a running reembed migration
    pub async fn reembed_cancel(state: &DashboardState) -> Result<Value, String> {
        state.rpc_call("memory.reembed.cancel", Value::Null).await
    }
}
