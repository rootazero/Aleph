//! Configuration types for the builtin tool registry

use crate::sync_primitives::Arc;

use tokio::sync::RwLock;

use crate::agents::sub_agents::SubAgentDispatcher;
use crate::dispatcher::ToolRegistry as DispatcherToolRegistry;
use crate::generation::GenerationProviderRegistry;
#[cfg(feature = "gateway")]
use crate::gateway::context::GatewayContext;

/// Configuration for builtin tools
#[derive(Clone, Default)]
pub struct BuiltinToolConfig {
    /// Tavily API key for search tool
    pub tavily_api_key: Option<String>,
    /// Generation provider registry for image/video/audio generation
    pub generation_registry: Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub dispatcher_registry: Option<Arc<RwLock<DispatcherToolRegistry>>>,
    /// Sub-agent dispatcher for delegation (smart tool discovery)
    pub sub_agent_dispatcher: Option<Arc<RwLock<SubAgentDispatcher>>>,
    /// Gateway context for sessions tools (sessions_list, sessions_send)
    /// Requires the "gateway" feature to be enabled.
    #[cfg(feature = "gateway")]
    pub gateway_context: Option<Arc<GatewayContext>>,
}
