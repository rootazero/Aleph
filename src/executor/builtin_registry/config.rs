//! Configuration types for the builtin tool registry

use crate::sync_primitives::{Arc, RwLock};

use crate::acp::manager::AcpAdapterManager;
use crate::config::Config;
use crate::gateway::context::GatewayContext;
use crate::generation::GenerationProviderRegistry;
use crate::memory::store::MemoryBackend;
use crate::memory::EmbeddingProvider;

/// Configuration for builtin tools
#[derive(Clone, Default)]
pub struct BuiltinToolConfig {
    /// Tavily API key for search tool
    pub tavily_api_key: Option<String>,
    /// Search registry for multi-provider search (`SearXNG`, Tavily, Brave, etc.)
    pub search_registry: Option<Arc<crate::search::SearchRegistry>>,
    /// Generation provider registry for image/video/audio generation
    pub generation_registry: Option<Arc<RwLock<GenerationProviderRegistry>>>,
    /// Shared config handle for `ConfigReadTool`
    pub config: Option<Arc<tokio::sync::RwLock<Config>>>,
    /// Memory backend for `memory_search` and `memory_browse` tools
    pub memory_db: Option<MemoryBackend>,
    /// Embedding provider for semantic memory search
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Gateway context for sessions tools (`sessions_list`, `sessions_send`)
    pub gateway_context: Option<Arc<GatewayContext>>,
    /// Session store for session tools (`session_new`, `session_set_topic`).
    /// Used when `gateway_context` is not available (e.g., agent loop without full gateway).
    pub session_manager: Option<Arc<dyn crate::gateway::session_store::SessionStore>>,
    /// Agent registry for agent management tools
    pub agent_registry: Option<Arc<crate::gateway::agent_instance::AgentRegistry>>,
    /// Workspace manager for agent management tools
    pub workspace_manager: Option<Arc<crate::gateway::agent_env::AgentEnvStore>>,
    /// Whiteboard canvas store for the `canvas` tool — the SAME `Arc` the
    /// gateway's `canvas.*` handlers hold (only that instance carries the
    /// event bus, so only its writes publish `canvas.updated` to open Panels).
    pub canvas_store: Option<Arc<crate::canvas::CanvasStore>>,
    /// SSRF policy for the pre-flight URL check in tools that fetch
    /// external resources (`a2a_agents.add`, `media_send`,
    /// `google_meet`). Sourced from the operator's `[ssrf]` config
    /// block so allow/deny rules reach the tool face.
    pub ssrf_policy: Option<crate::security::ssrf::SsrfPolicy>,
    /// Event bus for lifecycle event emission (agent switch/delete)
    pub event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    /// Agent manager for persistent agent definition storage (TOML config)
    pub agent_manager: Option<Arc<crate::config::agent_manager::AgentManager>>,
    /// Browser profile manager for browser_* tools
    pub browser_profile_manager: Option<Arc<crate::browser::manager::ProfileManager>>,
    /// Media pipeline for `media_understand` tool
    pub media_pipeline: Option<Arc<crate::media::MediaPipeline>>,
    /// Extension manager for plugin tool execution
    pub extension_manager: Option<Arc<crate::extension::ExtensionManager>>,
    /// ACP harness manager for delegate tools (`claude-code`, codex, `gemini_cli`, `acp_switch`)
    pub acp_manager: Option<Arc<AcpAdapterManager>>,
    /// A2A tool handle for the `a2a_delegate` / `a2a_agents` outbound tools.
    /// Filled by A2A subsystem init *after* the registry is built (late binding).
    /// `None` → A2A outbound tools are not registered.
    pub a2a_tool_handle: Option<crate::builtin_tools::A2AToolHandle>,
    /// Cron service for scheduled task management
    pub cron_service: Option<crate::tasks::cron::SharedCronService>,
    /// Heartbeat service for monitoring task management
    pub heartbeat_service: Option<crate::tasks::heartbeat::SharedHeartbeatService>,
    /// Tool context handle for workspace-scoped output paths
    pub tool_context: Option<crate::tools::ToolContextHandle>,
    /// Shared token manager for `vault_store` tool
    pub shared_token_manager: Option<Arc<crate::gateway::security::SharedTokenManager>>,
    /// Memory similarity threshold from config (overrides hardcoded default)
    pub memory_similarity_threshold: Option<f32>,
    /// Coordination task store for task/team management tools
    pub coord_task_store: Option<Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>>,
    /// Team snapshot store (sibling to `coord_task_store`; shares its connection).
    /// Populated alongside `coord_task_store` in the boot path so the
    /// `team_snapshot` builtin tool can capture/restore state bundles.
    pub snapshot_store: Option<Arc<crate::teams::SqliteSnapshotStore>>,
    /// Wake handle for the autonomous team dispatcher loop.
    /// Shared with `task_create` so a newly created task is dispatched without
    /// polling latency.
    pub dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    /// Team store for team management tools (`team_create`, `team_delegate`, `team_status`, `team_disband`)
    pub team_store: Option<Arc<dyn crate::teams::TeamStore>>,
    /// Artifact store for persisting task artifacts (delegation results, reports, etc.)
    pub artifact_store: Option<Arc<dyn crate::teams::artifacts::ArtifactStore>>,
    /// Channel registry for channel management tools (pairing, etc.)
    pub channel_registry: Option<Arc<crate::gateway::channel_registry::ChannelRegistry>>,
    /// Event log store for `team_digest` tool
    pub event_store: Option<Arc<dyn crate::teams::events::EventLogStore>>,
    /// Message router for `message_send` tool
    pub message_router: Option<Arc<crate::teams::messages::MessageRouter>>,
    /// Inbox helper for `inbox_read` tool
    pub inbox: Option<Arc<crate::teams::messages::Inbox>>,
    /// Session coordinator for collaborative session tools
    pub session_coordinator: Option<Arc<crate::teams::sessions::SessionCoordinator>>,
    /// Session store for `session_read` tool (read-only access)
    pub session_store: Option<Arc<dyn crate::teams::sessions::SessionStore>>,
    /// Message store for disbandment cleanup (expire pending messages)
    pub message_store: Option<Arc<dyn crate::teams::messages::MessageStore>>,
    /// State database for `memory_timeline` tool (event sourcing store)
    pub state_db: Option<Arc<crate::resilience::database::StateDatabase>>,
    /// Controls which memory-retrieval tools are exposed to the LLM.
    /// `Context` → skip all six retrieval tools (LLM can't call them).
    /// `Tools` / `Hybrid` → register all six retrieval tools.
    /// Defaults to `Hybrid` (same behaviour as before this field existed).
    pub injection_mode: crate::config::types::memory::MemoryInjectionMode,

    /// Capture-filter registry (Spec 4 Task 11).
    /// When set, the `session_complete` tool's raw-memory writes go through
    /// `insert_with_capture_filter` so extensions can mutate or block them.
    pub capture_registry: Option<Arc<crate::memory::extensions::MemoryExtensionRegistry>>,

    /// Note orientation handle (Spec 5 Task 12).
    /// When set, `note_orient` tool is registered and dispatched, and
    /// `note_schema` tool always has its `memory_dir` resolved.
    /// `None` → note orientation tools unavailable at runtime (schema tool still registered
    /// but dispatched stateless from `memory_dir` derived from paths).
    pub orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,

    /// Memory dir root for `note_schema` tool (Spec 5 Task 12).
    /// Points to the `note` subdirectory (e.g. `~/.aleph/memory/note`).
    pub note_memory_dir: Option<std::path::PathBuf>,

    /// Profile synthesizer for `user_profile` tool (Spec 7 Task 9).
    /// When set, the `user_profile` tool is registered and dispatched.
    pub profile_synthesizer:
        Option<Arc<dyn crate::memory::notes::profile::synthesizer::ProfileSynthesizer>>,

    /// Sandbox for exec-class tools (`code_exec`, `bash_exec`).
    /// `None` → tools return a structured "sandbox not configured" error.
    pub sandbox: Option<Arc<dyn crate::sandbox::Sandbox>>,

    /// External Google Meet transport bridge for the `google_meet` tool.
    /// `None` → the tool reports "bridge not configured" (Chrome/Twilio/audio
    /// automation lives out-of-core per R1/R3; core only relays JSON-RPC).
    pub google_meet_bridge: Option<Arc<crate::builtin_tools::google_meet::GoogleMeetBridge>>,

    /// Mirror of `MemoryConfig.project_scoped`: when true, the `note_manage`
    /// tool partitions notes by the active project directory. Default `false`
    /// (single-namespace, pre-feature behaviour).
    pub memory_project_scoped: bool,

    /// Optional provider for the strategic-planner node, resolved ONCE at
    /// startup (above the Think→Act loop, R10). `Some` ⇒ planner enabled (a
    /// dedicated `[strategy] planner_model` provider, or the executor's main
    /// provider as fallback); `None` ⇒ planner disabled. Read by the goal/loop/
    /// workflow tools in the registry constructor.
    pub planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,

    /// Catalog cache for store tools (`hub_catalog_sync` and T6–T8 tools).
    /// When `Some`, `hub_catalog_sync` is registered and dispatched.
    /// Shares the same SQLite file as the gateway extensions handlers.
    pub catalog_cache: Option<Arc<crate::hub::cache::CatalogCache>>,

    /// Marketplace configs for store tools (mirrors the gateway's conversion
    /// of `plugin_marketplaces` → `MarketplaceConfig`).
    /// Only meaningful when `catalog_cache` is `Some`.
    pub hub_marketplace_configs: Option<
        std::collections::HashMap<String, crate::extension::marketplace::types::MarketplaceConfig>,
    >,

    /// Live MCP manager handle for `hub_install_run` (T7). The SAME shared
    /// handle the gateway `extensions.*` handlers use — it cannot be
    /// reconstructed. `None` → agent-driven MCP installs report "MCP manager
    /// unavailable"; plugin installs and secret storage still work.
    pub hub_mcp_handle: Option<crate::mcp::manager::McpManagerHandle>,
}
