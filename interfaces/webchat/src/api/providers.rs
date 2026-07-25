use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(default)]
    pub has_api_key: bool,
}

fn default_provider_color() -> String {
    "#808080".to_string()
}
const fn default_timeout() -> u64 {
    300
}

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

/// One catalog row returned by `providers.catalog`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: String,
    pub display_name: String,
    pub default_model: String,
    pub base_url: String,
    pub protocol: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub modalities: Vec<String>,
    #[serde(default)]
    pub models: Vec<String>,
    /// Curated alternatives the preset ships for this provider. The backend has
    /// sent these since the field was introduced, documented as "used by the
    /// picker" — but nothing here read them, so a provider with no
    /// operator-configured `models` rendered exactly one row. [`roster`] is now
    /// the single place that decides what the picker shows.
    ///
    /// [`roster`]: crate::components::model_picker::roster
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub is_default: bool,
    /// Lifecycle of `default_model` — lets the picker mark an id the vendor has
    /// retired instead of offering it as a live choice.
    #[serde(default)]
    pub lifecycle: ModelLifecycle,
    /// This provider ships no default; the operator must name a model.
    #[serde(default)]
    pub requires_explicit_model: bool,
}

/// Wire form of the backend's `model_catalog::ModelLifecycle`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelLifecycle {
    /// `"active"` / `"preview"` / `"deprecated"`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Default for ModelLifecycle {
    fn default() -> Self {
        Self {
            status: "active".to_string(),
            successor: None,
            note: None,
        }
    }
}

impl ModelLifecycle {
    #[must_use]
    pub fn is_deprecated(&self) -> bool {
        self.status == "deprecated"
    }
}

/// Filter applied by the chat-window picker when querying the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogView {
    /// Verified, enabled providers (default — what the picker shows).
    Configured,
    /// API key present (verified or not) — useful for "add a key" hints.
    Available,
    /// Every chat-capable preset, regardless of credential state.
    All,
}

impl CatalogView {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Available => "available",
            Self::All => "all",
        }
    }
}

/// Wire form of [`crate::api::chat::ChatApi::send`]'s `model_override` —
/// mirrors `src/gateway/model_override::ModelOverride` byte-for-byte.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ModelOverride {
    /// Pin both provider and model — skips fallback chain on the server.
    Qualified { provider: String, model: String },
    /// Send only the model id; server resolves the provider.
    Raw { model: String },
}

impl ModelOverride {
    #[must_use]
    pub fn model(&self) -> &str {
        match self {
            Self::Qualified { model, .. } => model,
            Self::Raw { model } => model,
        }
    }

    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Qualified { provider, .. } => Some(provider),
            Self::Raw { .. } => None,
        }
    }
}

pub struct ProvidersApi;

impl ProvidersApi {
    /// List all providers
    pub async fn list(state: &DashboardState) -> Result<Vec<ProviderInfo>, String> {
        let result = state.rpc_call("providers.list", Value::Null).await?;

        // Extract providers array from result
        result
            .get("providers")
            .ok_or_else(|| "Invalid response: missing providers".to_string())
            .and_then(|providers| {
                serde_json::from_value(providers.clone())
                    .map_err(|e| format!("Failed to parse providers: {e}"))
            })
    }

    /// Chat-window model picker — fetch the credential-aware catalog.
    pub async fn catalog(
        state: &DashboardState,
        view: CatalogView,
    ) -> Result<Vec<CatalogEntry>, String> {
        let params = serde_json::json!({ "view": view.as_str() });
        let result = state.rpc_call("providers.catalog", params).await?;
        result
            .get("items")
            .ok_or_else(|| "Invalid response: missing items".to_string())
            .and_then(|items| {
                serde_json::from_value(items.clone())
                    .map_err(|e| format!("Failed to parse catalog: {e}"))
            })
    }

    /// Get a specific provider
    pub async fn get(state: &DashboardState, name: String) -> Result<ProviderInfo, String> {
        let params = serde_json::json!({
            "name": name,
        });

        let result = state.rpc_call("providers.get", params).await?;

        // Extract provider from result
        result
            .get("provider")
            .ok_or_else(|| "Invalid response: missing provider".to_string())
            .and_then(|provider| {
                serde_json::from_value(provider.clone())
                    .map_err(|e| format!("Failed to parse provider: {e}"))
            })
    }

    /// Create a new provider
    pub async fn create(
        state: &DashboardState,
        name: String,
        config: ProviderConfig,
    ) -> Result<(), String> {
        let config_value = serde_json::to_value(&config)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;

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
            .map_err(|e| format!("Failed to serialize config: {e}"))?;

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

        serde_json::from_value(result).map_err(|e| format!("Failed to parse test result: {e}"))
    }

    /// Trigger OAuth browser login for a subscription provider
    pub async fn oauth_login(
        state: &DashboardState,
        provider: String,
    ) -> Result<OAuthStatus, String> {
        let params = serde_json::json!({ "provider": provider });
        let result = state.rpc_call("providers.oauthLogin", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse OAuth status: {e}"))
    }

    /// Clear OAuth token for a subscription provider
    pub async fn oauth_logout(state: &DashboardState, provider: String) -> Result<(), String> {
        let params = serde_json::json!({ "provider": provider });
        state.rpc_call("providers.oauthLogout", params).await?;
        Ok(())
    }

    /// Get OAuth connection status
    pub async fn oauth_status(
        state: &DashboardState,
        provider: String,
    ) -> Result<OAuthStatus, String> {
        let params = serde_json::json!({ "provider": provider });
        let result = state.rpc_call("providers.oauthStatus", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse OAuth status: {e}"))
    }
}
