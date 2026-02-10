//! Providers API
//!
//! High-level API for AI provider management operations.

use crate::protocol::rpc::{RpcClient, RpcError};
use serde::{Deserialize, Serialize};

/// Provider information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Provider name
    pub name: String,
    /// Whether the provider is enabled
    pub enabled: bool,
    /// Model name
    pub model: String,
    /// Provider type (e.g., "openai", "anthropic")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    /// Whether this is the default provider
    pub is_default: bool,
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Whether the provider is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Model name
    pub model: String,
    /// API key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Base URL for API endpoint
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Provider test result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// Whether the test succeeded
    pub success: bool,
    /// Error message if test failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Response latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// Providers API client
///
/// Provides high-level methods for AI provider management.
///
/// # Example
///
/// ```ignore
/// use aleph_ui_logic::api::ProvidersApi;
///
/// let api = ProvidersApi::new(rpc_client);
/// let providers = api.list().await?;
/// ```
pub struct ProvidersApi<C: crate::connection::AlephConnector> {
    rpc: RpcClient<C>,
}

impl<C: crate::connection::AlephConnector> ProvidersApi<C> {
    /// Create a new providers API client
    pub fn new(rpc: RpcClient<C>) -> Self {
        Self { rpc }
    }

    /// List all providers
    pub async fn list(&self) -> Result<Vec<ProviderInfo>, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Deserialize)]
        struct Response {
            providers: Vec<ProviderInfo>,
        }

        let response: Response = self.rpc.call("providers.list", &()).await?;
        Ok(response.providers)
    }

    /// Get a single provider by name
    pub async fn get(&self, name: &str) -> Result<ProviderInfo, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            name: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            provider: ProviderInfo,
        }

        let response: Response = self.rpc.call("providers.get", &Params { name }).await?;
        Ok(response.provider)
    }

    /// Update a provider configuration
    pub async fn update(&self, name: &str, config: ProviderConfig) -> Result<(), RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            name: &'a str,
            config: ProviderConfig,
        }

        #[derive(Deserialize)]
        struct Response {
            ok: bool,
        }

        let _response: Response = self
            .rpc
            .call("providers.update", &Params { name, config })
            .await?;
        Ok(())
    }

    /// Delete a provider
    pub async fn delete(&self, name: &str) -> Result<(), RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            name: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            ok: bool,
        }

        let _response: Response = self.rpc.call("providers.delete", &Params { name }).await?;
        Ok(())
    }

    /// Test a provider connection
    pub async fn test(&self, config: ProviderConfig) -> Result<TestResult, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params {
            config: ProviderConfig,
        }

        let result: TestResult = self.rpc.call("providers.test", &Params { config }).await?;
        Ok(result)
    }

    /// Set the default provider
    pub async fn set_default(&self, name: &str) -> Result<(), RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            name: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            ok: bool,
        }

        let _response: Response = self
            .rpc
            .call("providers.setDefault", &Params { name })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_info_serialization() {
        let info = ProviderInfo {
            name: "openai".to_string(),
            enabled: true,
            model: "gpt-4".to_string(),
            provider_type: Some("openai".to_string()),
            is_default: true,
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["name"], "openai");
        assert_eq!(json["model"], "gpt-4");
        assert_eq!(json["is_default"], true);
    }

    #[test]
    fn test_provider_config_serialization() {
        let config = ProviderConfig {
            enabled: true,
            model: "gpt-4".to_string(),
            api_key: Some("sk-xxx".to_string()),
            base_url: None,
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["model"], "gpt-4");
        assert_eq!(json["api_key"], "sk-xxx");
        assert!(json.get("base_url").is_none());
    }

    #[test]
    fn test_test_result_serialization() {
        let result = TestResult {
            success: true,
            error: None,
            latency_ms: Some(150),
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["latency_ms"], 150);
        assert!(json.get("error").is_none());
    }
}
