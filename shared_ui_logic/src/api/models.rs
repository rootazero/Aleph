//! Models API
//!
//! High-level API for model discovery and selection operations.

use crate::protocol::rpc::{RpcClient, RpcError};
use serde::{Deserialize, Serialize};

/// Refresh result from models.refresh (single provider)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResult {
    /// Provider name
    pub provider: String,
    /// Number of models found
    pub count: usize,
    /// Source: "api", "preset", "config"
    pub source: String,
    /// Discovered models (simpler shape than ModelInfo)
    pub models: Vec<RefreshModelEntry>,
}

/// Model entry in refresh response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshModelEntry {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Model information from discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "gpt-4o")
    pub id: String,
    /// Provider name
    pub provider: String,
    /// Provider type (e.g., "openai", "anthropic")
    pub provider_type: String,
    /// Whether the provider is enabled
    pub enabled: bool,
    /// Whether this is the default model
    pub is_default: bool,
    /// Whether this is the provider's currently configured model
    pub is_current: bool,
    /// Model capabilities: "chat", "vision", "tools", "thinking"
    pub capabilities: Vec<String>,
    /// Source: "api", "preset", "config"
    pub source: String,
}

/// Models API client
///
/// Provides model discovery and selection operations.
pub struct ModelsApi<C: crate::connection::AlephConnector> {
    rpc: RpcClient<C>,
}

impl<C: crate::connection::AlephConnector> ModelsApi<C> {
    /// Create a new models API client
    pub fn new(rpc: RpcClient<C>) -> Self {
        Self { rpc }
    }

    /// List available models
    pub async fn list(
        &self,
        provider: Option<&str>,
        refresh: bool,
    ) -> Result<Vec<ModelInfo>, RpcError> {
        #[derive(Serialize)]
        struct Params<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            provider: Option<&'a str>,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            refresh: bool,
        }

        #[derive(Deserialize)]
        struct Response {
            models: Vec<ModelInfo>,
        }

        let response: Response = self
            .rpc
            .call("models.list", &Params { provider, refresh })
            .await?;
        Ok(response.models)
    }

    /// Force refresh model list for a specific provider
    ///
    /// Note: models.refresh returns a different shape than models.list.
    /// Single provider: `{ provider, count, source, models: [{id, name, capabilities}] }`
    pub async fn refresh(&self, provider: &str) -> Result<RefreshResult, RpcError> {
        #[derive(Serialize)]
        struct Params<'a> {
            provider: &'a str,
        }

        let result: RefreshResult = self
            .rpc
            .call("models.refresh", &Params { provider })
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_info_deserialization() {
        let json = serde_json::json!({
            "id": "gpt-4o",
            "provider": "openai",
            "provider_type": "openai",
            "enabled": true,
            "is_default": true,
            "is_current": true,
            "capabilities": ["chat", "vision", "tools"],
            "source": "api"
        });
        let info: ModelInfo = serde_json::from_value(json).unwrap();
        assert_eq!(info.id, "gpt-4o");
        assert_eq!(info.capabilities.len(), 3);
    }

    #[test]
    fn test_refresh_result_deserialization() {
        let json = serde_json::json!({
            "provider": "openai",
            "count": 2,
            "source": "api",
            "models": [
                {"id": "gpt-4o", "name": "GPT-4o", "capabilities": ["chat"]},
                {"id": "gpt-4o-mini", "capabilities": ["chat"]}
            ]
        });
        let result: RefreshResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.models[0].name, Some("GPT-4o".to_string()));
        assert!(result.models[1].name.is_none());
    }
}
