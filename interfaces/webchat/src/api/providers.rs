//! `providers.*` RPC client.
//!
//! # No DTOs live here
//!
//! Every request/response shape is re-exported from
//! [`aleph_protocol::providers`], which `alephcore` *builds* its responses
//! from. The Panel used to declare its own copies, and they had drifted in
//! both directions:
//!
//! * the write DTO said `model: String` where the server says
//!   `models: Vec<String>`, and the persistence path replaces `models`
//!   wholesale — so a provider carrying a model ladder was truncated to its
//!   first rung the moment anyone toggled its "enabled" switch here;
//! * `capabilities` and `cost` were not declared at all, so the server
//!   serialised both on every `providers.catalog` call and serde dropped them
//!   on arrival — a context window and a price we were already paying to
//!   transmit, and could not show.
//!
//! Sharing the types makes a rename a compile error on both sides at once,
//! which is the only guard that holds for a contract whose halves live in
//! different crates.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use aleph_protocol::providers::{
    AuthKind, CatalogEntry, CatalogParams, CatalogResult, CatalogView, DiscoveryFailureKind,
    ModelCapabilities, ModelLifecycle, ModelSource, ModelStatus, ModelsRefreshParams,
    ModelsRefreshResult, ModelsRefreshRow, OAuthStatus, ProviderConfigJson, ProviderGetResult,
    ProviderInfo, ProviderListResult, RateBasis, RateCard, RosterModel, TestResult,
};

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
        serde_json::from_value::<ProviderListResult>(result)
            .map(|r| r.providers)
            .map_err(|e| format!("Failed to parse providers: {e}"))
    }

    /// Fetch the credential-aware preset catalogue.
    ///
    /// `view` decides how much of it comes back: `Configured` for a picker,
    /// `All` for the settings page (which must offer presets nobody has set up
    /// yet). There is deliberately no query parameter — filtering happens
    /// client-side through `aleph_protocol::providers::search`.
    pub async fn catalog(
        state: &DashboardState,
        view: CatalogView,
    ) -> Result<Vec<CatalogEntry>, String> {
        let params = serde_json::to_value(CatalogParams::for_view(view))
            .map_err(|e| format!("Failed to serialize params: {e}"))?;
        let result = state.rpc_call("providers.catalog", params).await?;
        serde_json::from_value::<CatalogResult>(result)
            .map(|r| r.items)
            .map_err(|e| format!("Failed to parse catalog: {e}"))
    }

    /// Get a specific provider
    pub async fn get(state: &DashboardState, name: String) -> Result<ProviderInfo, String> {
        let params = serde_json::json!({
            "name": name,
        });

        let result = state.rpc_call("providers.get", params).await?;
        serde_json::from_value::<ProviderGetResult>(result)
            .map(|r| r.provider)
            .map_err(|e| format!("Failed to parse provider: {e}"))
    }

    /// Create a new provider
    pub async fn create(
        state: &DashboardState,
        name: String,
        config: ProviderConfigJson,
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
        config: ProviderConfigJson,
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
        config: ProviderConfigJson,
    ) -> Result<TestResult, String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });

        let result = state.rpc_call("providers.test", params).await?;

        serde_json::from_value(result).map_err(|e| format!("Failed to parse test result: {e}"))
    }

    /// Ask providers what models they serve now.
    ///
    /// `provider` narrows the sweep to one id (what a per-row refresh button
    /// wants); `None` sweeps every provider that is enabled **and** has a
    /// resolvable credential. Per-provider failures come back as rows carrying
    /// a [`DiscoveryFailureKind`], never as an RPC error, so one unreachable
    /// vendor does not blank the result — and a provider that is not
    /// configured at all yields no row at all, which is a third state a caller
    /// has to render rather than read as success.
    pub async fn models_refresh(
        state: &DashboardState,
        provider: Option<String>,
    ) -> Result<ModelsRefreshResult, String> {
        let params = serde_json::to_value(ModelsRefreshParams { provider })
            .map_err(|e| format!("Failed to serialize params: {e}"))?;
        let result = state.rpc_call("providers.modelsRefresh", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse models refresh result: {e}"))
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
