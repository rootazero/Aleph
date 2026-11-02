//! State for the OpenAI-compatible API routes.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::ProviderConfig;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::providers::http_provider::HttpProvider;

/// Shared state for OpenAI-compatible API handlers.
pub struct OpenAiApiState {
    pub server_id: String,
    /// Closure returning the *current* bearer token accepted by the
    /// OpenAI-compatible `/v1/*` routes. `None` means no shared token is
    /// resident — the token manager was never wired, or the token could not be
    /// provisioned at boot — and the `/v1/*` routes then reject every request
    /// with 401 rather than accepting any bearer (see `super::auth::authorize`).
    ///
    /// This is a closure rather than a snapshot `Option<String>` so that
    /// `SharedTokenManager::rotate` immediately revokes the previously
    /// issued token — previously the snapshot was taken at boot and
    /// `/v1/*` would accept the rotated-out token indefinitely.
    pub api_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub execution_adapter: Option<Arc<dyn ExecutionAdapter>>,
    pub provider_map: Arc<HashMap<String, Arc<HttpProvider>>>,
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub provider_configs: Arc<Vec<(String, ProviderConfig)>>,
    pub created_at: u64,
    pub embedding_provider: Option<Arc<dyn crate::memory::EmbeddingProvider>>,
}
