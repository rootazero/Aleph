//! State for the OpenAI-compatible API routes.

use std::collections::HashMap;

use crate::config::ProviderConfig;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::providers::http_provider::HttpProvider;
use crate::sync_primitives::Arc;

/// Shared state for OpenAI-compatible API handlers.
pub struct OpenAiApiState {
    pub server_id: String,
    pub api_token: Option<String>,
    pub execution_adapter: Option<Arc<dyn ExecutionAdapter>>,
    pub provider_map: Arc<HashMap<String, Arc<HttpProvider>>>,
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub provider_configs: Arc<Vec<(String, ProviderConfig)>>,
    pub created_at: u64,
    pub embedding_provider: Option<Arc<dyn crate::memory::EmbeddingProvider>>,
}
