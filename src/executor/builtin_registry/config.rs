//! Configuration types for the builtin tool registry

use crate::sync_primitives::{Arc, RwLock};

use crate::acp::manager::AcpAdapterManager;
use crate::config::{Config, ConfigPatcher};
use crate::dispatcher::ToolRegistry as DispatcherToolRegistry;
use crate::gateway::context::GatewayContext;
use crate::generation::GenerationProviderRegistry;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;

/// Configuration for builtin tools
#[derive(Clone, Default)]
pub struct BuiltinToolConfig {
    /// Tavily API key for search tool
    pub tavily_api_key: Option<String>,
    /// Search registry for multi-provider search (SearXNG, Tavily, Brave, etc.)
    pub search_registry: Option<Arc<crate::search::SearchRegistry>>,
    /// Generation provider registry for image/video/audio generation
    pub generation_registry: Option<Arc<RwLock<GenerationProviderRegistry>>>,
    /// Dispatcher tool registry for meta tools (smart tool discovery)
    pub dispatcher_registry: Option<Arc<tokio::sync::RwLock<DispatcherToolRegistry>>>,
    /// Shared config handle for ConfigReadTool
    pub config: Option<Arc<tokio::sync::RwLock<Config>>>,
    /// ConfigPatcher for ConfigUpdateTool
    pub config_patcher: Option<Arc<ConfigPatcher>>,
    /// Memory backend for memory_search and memory_browse tools
    pub memory_db: Option<MemoryBackend>,
    /// Embedding provider for semantic memory search
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Gateway context for sessions tools (sessions_list, sessions_send)
    pub gateway_context: Option<Arc<GatewayContext>>,
    /// Session store for session tools (session_new, session_set_topic).
    /// Used when gateway_context is not available (e.g., agent loop without full gateway).
    pub session_manager: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    /// Agent registry for agent management tools
    pub agent_registry: Option<Arc<crate::gateway::agent_instance::AgentRegistry>>,
    /// Workspace manager for agent management tools
    pub workspace_manager: Option<Arc<crate::gateway::agent_env::AgentEnvStore>>,
    /// Tool policy handle for per-agent tool access control
    pub tool_policy: Option<crate::builtin_tools::agent_manage::ToolPolicyHandle>,
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
    /// ACP harness manager for delegate tools (claude_code, codex, gemini_cli, acp_switch)
    pub acp_manager: Option<Arc<AcpAdapterManager>>,
    /// Cron service for scheduled task management
    pub cron_service: Option<crate::tasks::cron::SharedCronService>,
    /// Heartbeat service for monitoring task management
    pub heartbeat_service: Option<crate::tasks::heartbeat::SharedHeartbeatService>,
    /// Tool context handle for workspace-scoped output paths
    pub tool_context: Option<crate::tools::ToolContextHandle>,
    /// Shared token manager for vault_store tool
    pub shared_token_manager: Option<Arc<crate::gateway::security::SharedTokenManager>>,
    /// Memory similarity threshold from config (overrides hardcoded default)
    pub memory_similarity_threshold: Option<f32>,
    /// Coordination task store for task/team management tools
    pub coord_task_store: Option<Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>>,
    /// Agent message bus for task update/wait event notifications
    pub agent_message_bus: Option<Arc<crate::agents::swarm::AgentMessageBus>>,
    /// Team store for team management tools (team_create, team_delegate, team_status, team_disband)
    pub team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    /// Artifact store for persisting task artifacts (delegation results, reports, etc.)
    pub artifact_store: Option<Arc<dyn crate::teams::artifacts::ArtifactStore>>,
    /// Current agent ID for team tools (leader identity)
    pub current_agent_id: Option<String>,
    /// Current session key for team tools (parent session for spawned sub-agents)
    pub current_session_key: Option<crate::routing::SessionKey>,
    /// Channel registry for channel management tools (pairing, etc.)
    pub channel_registry: Option<Arc<crate::gateway::channel_registry::ChannelRegistry>>,
    /// Event log store for team_digest tool
    pub event_store: Option<Arc<dyn crate::teams::events::EventLogStore>>,
    /// Message router for message_send tool
    pub message_router: Option<Arc<crate::teams::messages::MessageRouter>>,
    /// Inbox helper for inbox_read tool
    pub inbox: Option<Arc<crate::teams::messages::Inbox>>,
    /// Session coordinator for collaborative session tools
    pub session_coordinator: Option<Arc<crate::teams::sessions::SessionCoordinator>>,
    /// Session store for session_read tool (read-only access)
    pub session_store: Option<Arc<dyn crate::teams::sessions::SessionStore>>,
    /// Message store for disbandment cleanup (expire pending messages)
    pub message_store: Option<Arc<dyn crate::teams::messages::MessageStore>>,
    /// State database for memory_timeline tool (event sourcing store)
    pub state_db: Option<Arc<crate::resilience::database::StateDatabase>>,
    /// Controls which memory-retrieval tools are exposed to the LLM.
    /// `Context` → skip all six retrieval tools (LLM can't call them).
    /// `Tools` / `Hybrid` → register all six retrieval tools.
    /// Defaults to `Hybrid` (same behaviour as before this field existed).
    pub injection_mode: crate::config::types::memory::MemoryInjectionMode,

    /// Capture-filter registry (Spec 4 Task 11).
    /// When set, the `session_complete` tool's raw-memory writes go through
    /// `insert_with_capture_filter` so extensions can mutate or block them.
    pub capture_registry:
        Option<std::sync::Arc<crate::memory::extensions::MemoryExtensionRegistry>>,

    /// Note orientation handle (Spec 5 Task 12).
    /// When set, `note_orient` tool is registered and dispatched, and
    /// `note_schema` tool always has its memory_dir resolved.
    /// `None` → note orientation tools unavailable at runtime (schema tool still registered
    /// but dispatched stateless from memory_dir derived from paths).
    pub orientation: Option<std::sync::Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,

    /// Memory dir root for note_schema tool (Spec 5 Task 12).
    /// Points to the `note` subdirectory (e.g. `~/.aleph/memory/note`).
    pub note_memory_dir: Option<std::path::PathBuf>,

    /// Profile synthesizer for `user_profile` tool (Spec 7 Task 9).
    /// When set, the `user_profile` tool is registered and dispatched.
    pub profile_synthesizer:
        Option<std::sync::Arc<dyn crate::memory::notes::profile::synthesizer::ProfileSynthesizer>>,

    /// Sandbox for exec-class tools (code_exec, bash_exec).
    /// `None` → tools return a structured "sandbox not configured" error.
    pub sandbox: Option<Arc<dyn crate::sandbox::Sandbox>>,
}
