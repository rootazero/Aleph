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
    /// Whether the provider has been verified via connection test
    #[serde(default)]
    pub verified: bool,
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

/// Discovered model from provider API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// Model identifier
    pub id: String,
    /// Display name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Who owns this model
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
    /// Model capabilities
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Provider probe result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    /// Whether probe succeeded (models discovered from API)
    pub success: bool,
    /// Connection latency in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Discovered models
    pub models: Vec<DiscoveredModel>,
    /// Source of models: "api" or "preset"
    pub model_source: String,
    /// Error message if probe failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Needs-setup check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsSetupResult {
    /// Whether first-run setup is needed
    pub needs_setup: bool,
    /// Number of configured providers
    pub provider_count: usize,
    /// Whether any provider is verified
    pub has_verified: bool,
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

    /// Probe a provider: test connection + discover models
    pub async fn probe(
        &self,
        protocol: &str,
        api_key: Option<&str>,
        base_url: Option<&str>,
    ) -> Result<ProbeResult, RpcError> {
        #[derive(Serialize)]
        struct Params<'a> {
            protocol: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            api_key: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            base_url: Option<&'a str>,
        }

        let result: ProbeResult = self
            .rpc
            .call("providers.probe", &Params { protocol, api_key, base_url })
            .await?;
        Ok(result)
    }

    /// Check if first-run setup wizard is needed
    pub async fn needs_setup(&self) -> Result<NeedsSetupResult, RpcError> {
        let result: NeedsSetupResult = self.rpc.call("providers.needsSetup", &()).await?;
        Ok(result)
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
            verified: true,
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["name"], "openai");
        assert_eq!(json["model"], "gpt-4");
        assert_eq!(json["is_default"], true);
        assert_eq!(json["verified"], true);
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

    #[test]
    fn test_probe_result_deserialization() {
        let json = serde_json::json!({
            "success": true,
            "latency_ms": 234,
            "models": [
                {"id": "gpt-4o", "name": "GPT-4o", "capabilities": ["chat", "vision"]}
            ],
            "model_source": "api"
        });
        let result: ProbeResult = serde_json::from_value(json).unwrap();
        assert!(result.success);
        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].id, "gpt-4o");
    }

    #[test]
    fn test_needs_setup_result_deserialization() {
        let json = serde_json::json!({
            "needs_setup": true,
            "provider_count": 0,
            "has_verified": false
        });
        let result: NeedsSetupResult = serde_json::from_value(json).unwrap();
        assert!(result.needs_setup);
        assert_eq!(result.provider_count, 0);
    }

    #[test]
    fn test_discovered_model_name_optional() {
        let json = serde_json::json!({"id": "gpt-4o", "capabilities": ["chat"]});
        let model: DiscoveredModel = serde_json::from_value(json).unwrap();
        assert_eq!(model.id, "gpt-4o");
        assert!(model.name.is_none());
    }
}
