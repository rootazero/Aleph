//! Configuration types for the builtin tool registry

use crate::sync_primitives::Arc;

use tokio::sync::RwLock;

use crate::agents::sub_agents::{SubAgentDispatcher, SubAgentRegistry};
use crate::config::{Config, ConfigPatcher};
use crate::dispatcher::ToolRegistry as DispatcherToolRegistry;
use crate::generation::GenerationProviderRegistry;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;
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
    /// Shared config handle for ConfigReadTool
    pub config: Option<Arc<RwLock<Config>>>,
    /// ConfigPatcher for ConfigUpdateTool
    pub config_patcher: Option<Arc<ConfigPatcher>>,
    /// Memory backend for memory_search and memory_browse tools
    pub memory_db: Option<MemoryBackend>,
    /// Embedding provider for semantic memory search
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Gateway context for sessions tools (sessions_list, sessions_send)
    pub gateway_context: Option<Arc<GatewayContext>>,
    /// Agent registry for agent management tools
    pub agent_registry: Option<Arc<crate::gateway::agent_instance::AgentRegistry>>,
    /// Workspace manager for agent management tools
    pub workspace_manager: Option<Arc<crate::gateway::workspace::WorkspaceManager>>,
    /// Tool policy handle for per-agent tool access control
    pub tool_policy: Option<crate::builtin_tools::agent_manage::ToolPolicyHandle>,
    /// Sub-agent registry for subagent_steer and subagent_kill tools
    pub sub_agent_registry: Option<Arc<SubAgentRegistry>>,
    /// Event bus for lifecycle event emission (agent switch/delete)
    pub event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    /// Agent manager for persistent agent definition storage (TOML config)
    pub agent_manager: Option<Arc<crate::config::agent_manager::AgentManager>>,
    /// Browser profile manager for browser_* tools
    pub browser_profile_manager: Option<Arc<crate::browser::manager::ProfileManager>>,
    /// Media pipeline for media_understand tool
    pub media_pipeline: Option<Arc<crate::media::MediaPipeline>>,
    /// Extension manager for plugin tool execution
    pub extension_manager: Option<Arc<crate::extension::ExtensionManager>>,
}
