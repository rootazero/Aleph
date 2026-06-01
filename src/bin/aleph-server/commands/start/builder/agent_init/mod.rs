//! Spec C policy: covered by parent `start` command's main-level
//! lock (`with_policy_owned` semantically; acquired in `main()`
//! before `fork()`).
//!
//! Agent handler registration and provider initialization.
//!
//! Extracted from `start/mod.rs` to keep the orchestrator under 500 lines.
//! Contains the complex agent engine setup: provider registry creation,
//! tool registry, task routing, and handler wiring.
//!
//! ## Layout
//! - [`coord_stores`] — `coord.db` + `teams.db` SQLite store initialization
//! - [`generation_init`] — generation provider registry + hot-reload
//! - this module — orchestration + signature

mod coord_stores;
mod generation_init;

use coord_stores::{
    build_team_components, init_coord_and_snapshot, init_team_store, init_teams_evolution_stores,
};
use generation_init::init_generation_registry;

use alephcore::sync_primitives::{Arc, RwLock};

use alephcore::executor::BuiltinToolRegistry;
use alephcore::gateway::handlers::agent::{
    self as agent_handlers, handle_cancel as handle_agent_cancel, handle_respond_to_input,
    handle_run, handle_status as handle_agent_status, AgentRunManager,
};
use alephcore::gateway::handlers::chat as chat_handlers;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{
    available_provider_from_env, can_create_provider_from_env, create_provider_registry_from_env,
    AgentRegistry, ExecutionEngine, ExecutionEngineConfig, GatewayConfig as FullGatewayConfig,
};
use alephcore::ProviderRegistry;

use crate::server_init::{handle_chat_send_with_engine, handle_run_with_engine};
use alephcore::providers::adapter::RequestPayload;
use alephcore::providers::message::UnifiedMessage;

/// Result from registering agent handlers — includes optional execution support
/// for use by InboundMessageRouter.
pub(in crate::commands::start) struct AgentHandlersResult {
    pub _run_manager: Arc<AgentRunManager>,
    pub execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    pub tool_catalog: Option<Arc<alephcore::tool_metadata::ToolCatalog>>,
    pub embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    pub compression_service:
        Option<std::sync::Arc<alephcore::memory::compression::CompressionService>>,
    /// Multi-provider registry for routing and hot-switching
    pub multi_registry: Option<Arc<alephcore::MultiProviderRegistry>>,
    /// Deferred injection cell for ChannelRegistry (created after agent handlers)
    pub channel_registry_cell: Option<
        Arc<tokio::sync::OnceCell<Arc<alephcore::gateway::channel_registry::ChannelRegistry>>>,
    >,
    /// Deferred injection cell for ClarificationManager (enables the `ask_user` tool)
    pub clarification_manager_cell:
        Option<Arc<tokio::sync::OnceCell<Arc<alephcore::clarification::ClarificationManager>>>>,
    /// Generation provider registry for TTS voice output
    pub generation_registry: Option<Arc<RwLock<alephcore::generation::GenerationProviderRegistry>>>,
    /// Builtin tool registry (for heartbeat probe executor)
    pub tool_registry: Option<Arc<BuiltinToolRegistry>>,
    /// Team store for panel RPC handlers
    pub team_store: Option<Arc<dyn alephcore::teams::TeamStore>>,
    /// Coord task store for team RPC handlers (unified task system)
    pub coord_task_store: Option<Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>>,
    /// Snapshot store for `teams.snapshot.*` RPC handlers. Shares the coord
    /// connection — None if coord init failed.
    pub snapshot_store: Option<Arc<alephcore::teams::SqliteSnapshotStore>>,
    /// State database (~/.aleph/data/state.db). Holds `task_traces` rows that
    /// the `teams.usage` RPC + `team_usage` tool aggregate by team-member
    /// agent_id. `None` in simulated mode.
    pub state_db: Option<Arc<alephcore::resilience::StateDatabase>>,
    /// Event log store for team event logging
    pub event_store: Option<Arc<dyn alephcore::teams::events::EventLogStore>>,
    /// Artifact store for teams.task.trace and team artifact tools (R3).
    /// Trait-object handle is what gateway handlers consume; the concrete
    /// `SqliteArtifactStore` is still used internally by tool wiring.
    pub artifact_store: Option<Arc<dyn alephcore::teams::artifacts::ArtifactStore>>,
    /// Message router for the TeamNotifier event handler
    pub message_router: Option<Arc<alephcore::teams::messages::MessageRouter>>,
    /// OnceLock handle shared with the real ExecutionEngine. Boot code calls
    /// `.set(orchestrator)` on this after `initialize_orchestrator` returns so
    /// that `dispatch_via_orchestrator` can resolve the orchestrator from the
    /// engine's own field rather than an external argument.
    pub orchestrator_cell: Option<
        std::sync::Arc<std::sync::OnceLock<std::sync::Arc<alephcore::orchestrator::Orchestrator>>>,
    >,
    /// Phase 6 follow-up: MemoryContextProvider exposed for the orchestrator
    /// path so `AgentHarnessRunner` can build the per-turn system prompt with
    /// curated memory + hybrid retrieval. ExecutionEngine receives a clone via
    /// `with_memory_context_provider`; the orchestrator path needs its own
    /// reference because it does not hold an `ExecutionEngine`.
    pub memory_context_provider: Option<Arc<alephcore::thinker::MemoryContextProvider>>,
    /// SQLite memory backend exposed for the orchestrator path so the per-run
    /// `ContextCompactor` can reuse hierarchical session summaries for
    /// zero-API-cost compaction.
    pub memory_backend: Option<alephcore::memory::store::MemoryBackend>,
    /// Arena manager for collaborative multi-agent arenas (R3).
    pub arena_manager: Option<Arc<RwLock<alephcore::arena::ArenaManager>>>,
}

/// Register agent.run / agent.status / agent.cancel / chat.* handlers.
/// Selects real ExecutionEngine when an API key is available (env or config),
/// otherwise uses the simulated AgentRunManager.
/// Returns execution support components for inbound routing.
#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) async fn register_agent_handlers(
    server: &mut GatewayServer,
    session_store: Arc<dyn alephcore::gateway::session_store::SessionStore>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    router: Arc<AgentRouter>,
    full_config: &FullGatewayConfig,
    app_config: &alephcore::Config,
    app_config_arc: Arc<tokio::sync::RwLock<alephcore::Config>>,
    memory_db: &alephcore::memory::store::MemoryBackend,
    workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
    agent_manager: Arc<alephcore::AgentManager>,
    acp_manager: Option<Arc<alephcore::acp::manager::AcpAdapterManager>>,
    // A2A outbound tool handle — filled by A2A subsystem init *after* the
    // builtin tool registry is built (see commands/start/mod.rs). `None` when
    // A2A is disabled, in which case the a2a_* tools are not registered.
    a2a_tool_handle: Option<alephcore::builtin_tools::A2AToolHandle>,
    cron_service: Option<alephcore::tasks::cron::SharedCronService>,
    heartbeat_service: Option<alephcore::tasks::heartbeat::SharedHeartbeatService>,
    daemon: bool,
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
    // Spec 5 Task 12: wiki orientation handle for DreamDaemon + MemoryContextProvider + tools.
    orientation: Option<std::sync::Arc<dyn alephcore::memory::notes::orientation::NoteOrientation>>,
    note_memory_dir: Option<std::path::PathBuf>,
    // Phase 3 Task 8: sandbox for exec-class tools.
    sandbox: Option<Arc<dyn alephcore::sandbox::Sandbox>>,
) -> AgentHandlersResult {
    let mut run_manager: Option<Arc<AgentRunManager>> = None;
    let mut exec_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>> = None;
    let mut agent_reg: Option<Arc<AgentRegistry>> = None;
    let mut default_prov: Option<Arc<dyn alephcore::providers::AiProvider>> = None;
    let tool_catalog_out: Option<Arc<alephcore::tool_metadata::ToolCatalog>>;
    let mut embedder_out: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>> = None;
    let mut compression_out: Option<
        std::sync::Arc<alephcore::memory::compression::CompressionService>,
    > = None;
    let mut multi_reg: Option<Arc<alephcore::MultiProviderRegistry>> = None;
    let mut channel_reg_cell: Option<
        Arc<tokio::sync::OnceCell<Arc<alephcore::gateway::channel_registry::ChannelRegistry>>>,
    > = None;
    let mut clar_mgr_cell: Option<
        Arc<tokio::sync::OnceCell<Arc<alephcore::clarification::ClarificationManager>>>,
    > = None;
    let mut tool_reg_out: Option<Arc<BuiltinToolRegistry>> = None;
    let mut orch_cell_out: Option<
        std::sync::Arc<std::sync::OnceLock<std::sync::Arc<alephcore::orchestrator::Orchestrator>>>,
    > = None;
    // Phase 6 follow-up: hold the same MemoryContextProvider that gets injected
    // into the (legacy) ExecutionEngine so the orchestrator path can build a
    // system prompt with curated memory + hybrid retrieval.
    let mut mcp_for_orchestrator: Option<Arc<alephcore::thinker::MemoryContextProvider>> = None;
    // F5 (per-team usage): lift `resilience_db` out of the real-engine branch so
    // the `teams.usage` RPC handler can scan `task_traces` for ProviderUsage
    // events scoped to a team's members. `None` in the simulated branch (no
    // state.db == no historical usage to aggregate).
    let mut state_db_out: Option<Arc<alephcore::resilience::StateDatabase>> = None;

    let arena_manager = Arc::new(RwLock::new(alephcore::arena::ArenaManager::new()));

    // Coord + snapshot stores (single shared connection to coord.db).
    let (coord_store, snapshot_store) = init_coord_and_snapshot(event_bus.clone(), daemon).await;

    // Team store (teams.db).
    let team_store = init_team_store(daemon).await;

    // Teams-evolution stores (artifact / event log / message / session, all on teams.db).
    let (artifact_store, event_store, message_store, team_session_store) =
        init_teams_evolution_stores(daemon).await;

    // Higher-level team components derived from the four stores above.
    let (message_router, inbox, session_coordinator, agent_message_bus) = build_team_components(
        &message_store,
        &event_store,
        &team_session_store,
        &artifact_store,
    );

    // Generation provider registry (TTS / image / video) — independent of
    // the chat AI provider. Also spawns the panel hot-reload subscriber.
    let generation_registry = init_generation_registry(
        app_config,
        app_config_arc.clone(),
        event_bus.clone(),
        shared_token_mgr.clone(),
        daemon,
    );

    // Build MultiProviderRegistry: register ALL configured providers.
    //
    // ProviderConfig.api_key is #[serde(skip)] and never lives on disk — keys live
    // in the vault under "ai:{provider_name}". We must inject from vault before
    // calling create_provider, otherwise the runtime provider has no key and chat
    // requests fail with "API key is required" (test path resolves vault separately,
    // hence the test/runtime asymmetry users hit on Telegram/Feishu paths).
    let provider_registry = {
        use alephcore::providers::create_provider;

        // Read api_key from vault for a given provider name
        let vault_key_for = |name: &str| format!("ai:{}", name);
        let vault_lookup = |name: &str| -> Option<String> {
            match shared_token_mgr.get_secret(&vault_key_for(name)) {
                Ok(Some(secret)) => Some(secret.expose().to_string()),
                _ => None,
            }
        };
        // Hydrate a ProviderConfig clone with its vault api_key (if present)
        let hydrate = |name: &str, cfg: &alephcore::ProviderConfig| -> alephcore::ProviderConfig {
            let mut c = cfg.clone();
            if c.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) {
                c.api_key = vault_lookup(name);
            }
            c
        };
        let has_key = |name: &str, cfg: &alephcore::ProviderConfig| -> bool {
            cfg.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false)
                || vault_lookup(name).is_some()
        };

        // Determine default provider name
        let default_name = app_config.general.default_provider.clone().or_else(|| {
            app_config
                .providers
                .iter()
                .find(|(name, cfg)| cfg.enabled && has_key(name, cfg))
                .map(|(name, _)| name.clone())
        });

        // Try env vars first for the initial provider
        let env_provider = if can_create_provider_from_env() {
            create_provider_registry_from_env().ok().map(|reg| {
                let p = reg.default_provider();
                let name = available_provider_from_env().unwrap_or("env");
                (name, p)
            })
        } else {
            None
        };

        // Build multi-provider registry
        if let Some((env_name, env_prov)) = env_provider {
            let registry = Arc::new(alephcore::MultiProviderRegistry::new(
                env_name.to_string(),
                env_prov,
            ));
            // Also register all config providers
            for (name, provider_cfg) in &app_config.providers {
                if !provider_cfg.enabled || name.as_str() == env_name {
                    continue;
                }
                if !has_key(name, provider_cfg) {
                    continue;
                }
                let hydrated = hydrate(name, provider_cfg);
                if let Ok(p) = create_provider(name, hydrated) {
                    registry.register(name.clone(), p);
                    tracing::info!(provider = %name, "Registered provider from config");
                }
            }
            multi_reg = Some(registry.clone());
            if !daemon {
                println!(
                    "  Providers: {} registered",
                    registry.list_providers().len()
                );
            }
            Some(registry as Arc<alephcore::MultiProviderRegistry>)
        } else if let Some(def_name) = default_name {
            // No env provider — create from config
            if let Some(provider_cfg) = app_config.providers.get(&def_name) {
                let default_hydrated = hydrate(&def_name, provider_cfg);
                if let Ok(default_prov) = create_provider(&def_name, default_hydrated) {
                    let registry = Arc::new(alephcore::MultiProviderRegistry::new(
                        def_name.clone(),
                        default_prov,
                    ));
                    // Register remaining providers
                    for (name, pcfg) in &app_config.providers {
                        if !pcfg.enabled || name == &def_name {
                            continue;
                        }
                        if !has_key(name, pcfg) {
                            continue;
                        }
                        let hydrated = hydrate(name, pcfg);
                        if let Ok(p) = create_provider(name, hydrated) {
                            registry.register(name.clone(), p);
                            tracing::info!(provider = %name, "Registered provider from config");
                        }
                    }
                    multi_reg = Some(registry.clone());
                    if !daemon {
                        println!(
                            "  Providers: {} registered (default: {})",
                            registry.list_providers().len(),
                            def_name
                        );
                    }
                    Some(registry as Arc<alephcore::MultiProviderRegistry>)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    // Wire global fallback provider chain from config
    if let Some(ref registry) = multi_reg {
        let fallbacks = &app_config.general.fallback_providers;
        if !fallbacks.is_empty() {
            registry.set_fallbacks(fallbacks.clone());
            tracing::info!(
                chain = ?fallbacks,
                "Fallback provider chain configured"
            );
            if !daemon {
                println!("  Fallback chain: {:?}", fallbacks);
            }
        }
    }

    // Shared cell for deferred CommandParser injection into chat.send handler.
    // Created here (outer scope) so both the if-branch (chat.send registration)
    // and the tool_catalog block (parser creation) can access it.
    let command_parser_cell: Arc<
        tokio::sync::RwLock<Option<Arc<alephcore::command::CommandParser>>>,
    > = Arc::new(tokio::sync::RwLock::new(None));

    // ── Spec 4 Task 11: construct MemoryExtensionRegistry ────────────────────
    // Built once here (outer scope) so it can be threaded into all subsystems
    // regardless of whether an AI provider is available.
    // Registers the POC first-party extension (no-op at floor=0.0) to prove
    // end-to-end plumbing. Real floor could be plumbed from config later.
    let memory_ext_registry: std::sync::Arc<
        alephcore::memory::extensions::MemoryExtensionRegistry,
    > = {
        use alephcore::memory::extensions::{
            EnvelopeRelevanceFloorExtension, MemoryExtensionRegistry,
        };
        let reg = MemoryExtensionRegistry::new();
        reg.register(std::sync::Arc::new(EnvelopeRelevanceFloorExtension::new(
            0.0,
        )));
        std::sync::Arc::new(reg)
    };

    if let Some(provider_registry) = provider_registry {
        // Create embedding provider from app config for memory tools
        let embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>> = {
            let embedding_settings = &app_config.memory.embedding;
            let manager = alephcore::memory::EmbeddingManager::new(embedding_settings.clone());
            match manager.init().await {
                Ok(()) => {
                    let provider = manager.get_active_provider().await;
                    if provider.is_some() && !daemon {
                        println!("  Embedding provider initialized for memory tools");
                    }
                    provider
                }
                Err(e) => {
                    if !daemon {
                        eprintln!("  Warning: Failed to initialize embedding provider: {}", e);
                    }
                    None
                }
            }
        };

        // Extract Tavily API key from search config (legacy fallback for the
        // case where SearchRegistry can't be built — keeps the old behaviour).
        let tavily_api_key = app_config
            .search
            .as_ref()
            .and_then(|s| s.backends.get(&s.default_provider))
            .and_then(|b| b.api_key.clone());

        // Build a SearchRegistry from `[search]` config so non-Tavily backends
        // (SearXNG, Brave, etc.) actually reach the `search` builtin tool.
        // Without this, the user can configure `[search.backends.searxng]` all
        // they want but `SearchTool` falls back to TAVILY_API_KEY env and
        // reports "No search provider configured" at runtime.
        let search_registry =
            alephcore::search::SearchRegistry::from_config(app_config.search.as_ref())
                .map(Arc::new);

        // Create agent registry before tool config so agent management tools can use it
        let agent_registry = Arc::new(AgentRegistry::new());

        // Wire L0 raw-memory writer so every gateway-mediated agent turn is
        // captured into raw_memories. Without this, only session-compaction
        // residue and SessionEnd writes reach L0; short conversations under
        // the WS path never persist and the L1 pipeline starves.
        agent_registry
            .set_raw_memory_writer(memory_db.clone()
                as std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>)
            .await;

        // Capture embedder before it's moved into tool_config
        embedder_out = embedder.clone();

        // Get extension manager for plugin tool execution
        let extension_manager = {
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            get_extension_manager().ok().map(Arc::clone)
        };

        // Spec 7 T9: construct FsProfileSynthesizer from the note_memory_dir + AI provider.
        // Constructed before tool_config so we can inject into BuiltinToolConfig, and later
        // into MemoryContextProvider and CompressionService.
        let profile_synth: Option<
            std::sync::Arc<dyn alephcore::memory::notes::profile::synthesizer::ProfileSynthesizer>,
        > = {
            use alephcore::memory::notes::profile::synthesizer::FsProfileSynthesizer;
            let prov = provider_registry.default_provider();
            let note_dir_opt = if let Some(dir) = note_memory_dir.clone() {
                Some(dir)
            } else {
                match alephcore::utils::paths::get_note_memory_dir() {
                    Ok(dir) => Some(dir),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to resolve note memory directory; profile synthesizer disabled");
                        None
                    }
                }
            };
            note_dir_opt.map(|note_dir| {
                let mut synth = FsProfileSynthesizer::new(note_dir, prov);
                if let Some(ref wiki) = orientation {
                    synth = synth.with_orientation(wiki.clone());
                }
                synth = synth.with_config(&app_config.memory.profile);
                std::sync::Arc::new(synth)
                    as std::sync::Arc<
                        dyn alephcore::memory::notes::profile::synthesizer::ProfileSynthesizer,
                    >
            })
        };

        // Shared wake handle for the autonomous team dispatcher. `task_create`
        // notifies it; the dispatcher (constructed later, once GatewayContext
        // exists) waits on it.
        let dispatch_signal = std::sync::Arc::new(tokio::sync::Notify::new());

        // Build tool config with memory backend, embedder, search API key, and agent management deps
        let tool_config = alephcore::executor::BuiltinToolConfig {
            memory_db: Some(memory_db.clone()),
            embedder,
            tavily_api_key,
            search_registry: search_registry.clone(),
            agent_registry: Some(agent_registry.clone()),
            workspace_manager: workspace_manager.clone(),
            event_bus: Some(event_bus.clone()),
            tool_policy: Some(alephcore::builtin_tools::agent_manage::new_tool_policy_handle()),
            agent_manager: Some(agent_manager.clone()),
            extension_manager: extension_manager.clone(),
            acp_manager: acp_manager.clone(),
            a2a_tool_handle: a2a_tool_handle.clone(),
            cron_service: cron_service.clone(),
            heartbeat_service: heartbeat_service.clone(),
            generation_registry: Some(generation_registry.clone()),
            tool_context: Some(alephcore::tools::new_tool_context_handle()),
            session_manager: Some(session_store.clone()),
            shared_token_manager: Some(shared_token_mgr.clone()),
            memory_similarity_threshold: Some(app_config.memory.similarity_threshold),
            memory_project_scoped: app_config.memory.project_scoped,
            injection_mode: app_config.memory.injection_mode,
            coord_task_store: coord_store.clone(),
            snapshot_store: snapshot_store.clone(),
            dispatch_signal: Some(dispatch_signal.clone()),
            agent_message_bus: agent_message_bus.clone(),
            team_store: team_store.clone(),
            artifact_store: artifact_store.clone(),
            event_store: event_store.clone(),
            message_store: message_store.clone(),
            message_router: message_router.clone(),
            inbox: inbox.clone(),
            session_store: team_session_store.clone(),
            session_coordinator: session_coordinator.clone(),
            browser_profile_manager: Some(std::sync::Arc::new(
                alephcore::browser::manager::ProfileManager::new(
                    app_config.general.browser.clone(),
                ),
            )),
            // Spec 4 Task 11: thread capture-filter registry into session_complete tool.
            capture_registry: Some(memory_ext_registry.clone()),
            // Spec 5 Task 12: wiki orientation tools.
            orientation: orientation.clone(),
            note_memory_dir: note_memory_dir.clone(),
            // Spec 7 Task 9: user profile tool.
            profile_synthesizer: profile_synth.clone(),
            // Phase 3 Task 8: sandbox for exec-class tools.
            sandbox: sandbox.clone(),
            arena_manager: Some(arena_manager.clone()),
            // Without the live Config arc, BuiltinToolRegistry constructor at
            // executor/builtin_registry/builder/constructor.rs:50 falls back to
            // SsrfPolicy::default() (enabled=true) for WebFetchTool — making
            // `[security.ssrf]` settings (including enabled=false and the
            // allowed_hosts bridge) silently dead. Plumb it through so
            // user-facing SSRF config actually reaches web_fetch.
            config: Some(app_config_arc.clone()),
            // Google Meet transport bridge — opt-in via env (the bridge is an
            // out-of-core plugin process; core only relays JSON-RPC to it).
            google_meet_bridge: alephcore::builtin_tools::google_meet::GoogleMeetBridge::from_env()
                .map(std::sync::Arc::new),
            ..Default::default()
        };
        let mut tool_registry = BuiltinToolRegistry::with_config(tool_config).await;

        use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;
        use alephcore::tool_metadata::{ToolSource, UnifiedTool};
        let mut tools: Vec<UnifiedTool> = BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|def| {
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", def.name),
                    def.name,
                    def.description,
                    ToolSource::Builtin,
                );
                // Attach parameter schemas from the tool registry so LLMs
                // know which arguments each tool expects.
                if let Some(schema) = tool_registry.get_tool_schema(def.name) {
                    ut = ut.with_parameters_schema(schema);
                }
                ut
            })
            .collect();

        // Add plugin tools to both LLM tool list and BuiltinToolRegistry
        {
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            if let Ok(ext_manager) = get_extension_manager() {
                if let Err(e) = ext_manager.ensure_loaded().await {
                    tracing::warn!("Failed to load extensions for plugin tools: {}", e);
                }
                let registry = ext_manager.get_plugin_registry().await;
                for plugin in registry.list_plugins() {
                    if !plugin.status.is_active() {
                        continue;
                    }
                    if let Ok(manifest) =
                        alephcore::extension::manifest::parse_manifest_from_dir_sync(
                            &plugin.root_dir,
                        )
                    {
                        for t in manifest.tools_v2.unwrap_or_default() {
                            let mut tool = UnifiedTool::new(
                                format!("plugin:{}:{}", plugin.id, t.name),
                                &t.name,
                                t.description.as_deref().unwrap_or(""),
                                ToolSource::Plugin {
                                    plugin_id: plugin.id.clone(),
                                },
                            );
                            if let Some(params) = t.parameters {
                                tool = tool.with_parameters_schema(params);
                            }
                            // Register in BuiltinToolRegistry for execution routing
                            tool_registry.register_tool(tool.clone());
                            tools.push(tool);
                        }
                    }
                }
            }
        }

        {
            let plugin_tool_count = tools
                .iter()
                .filter(|t| matches!(t.source, ToolSource::Plugin { .. }))
                .count();
            let builtin_tool_count = tools
                .iter()
                .filter(|t| matches!(t.source, ToolSource::Builtin))
                .count();
            tracing::info!(
                builtin = builtin_tool_count,
                plugin = plugin_tool_count,
                total = tools.len(),
                "LLM tool list assembled"
            );
        }

        // Grab handles to OnceCell's before wrapping in Arc.
        // We'll inject GatewayContext after ExecutionAdapter is created (breaks circular dep).
        // ChannelRegistry is injected after channel initialization.
        // MemoryContextProvider is injected after the MCP is constructed below
        // (Spec A Task 17 — enables the `remember` tool).
        let gateway_context_cell = tool_registry.gateway_context_cell();
        channel_reg_cell = Some(tool_registry.channel_registry_cell());
        clar_mgr_cell = Some(tool_registry.clarification_manager_cell());
        let mcp_cell_for_tools = tool_registry.memory_context_provider_cell();

        // Spec 2 Task 8: construct Arc<MemoryReflector> and inject into memory_reflect tool.
        // Requires embedder (for retrieval) and a default provider (for LLM synthesis).
        // On failure we log a warning and continue — memory_reflect will return a clear error.
        //
        // The default_prov assignment used to live ~140 lines below this block,
        // which left both the reflector and the query-filer wiring permanently
        // disabled at startup. Capture it here so both paths can fire.
        if default_prov.is_none() {
            default_prov = Some(provider_registry.default_provider());
        }
        if let (Some(emb), Some(prov)) = (embedder_out.clone(), default_prov.clone()) {
            use alephcore::memory::reflector::{
                fs_reflector::RecallWriter, recall_signals::SignalRow, MemoryReflector,
            };
            use alephcore::memory::store::sqlite::recall_signals::RecallHit;

            // Build a fresh assembler dedicated to the reflector (same config as MCP).
            let reflector_mcp = super::init_memory_context_provider(
                memory_db,
                emb,
                Some(prov.clone()),
                app_config.memory.assembler_config(),
            );
            let reflector_assembler = reflector_mcp.assembler();

            // Build the recall-signal writer closure: translates SignalRow → record_signals.
            let store_for_writer = memory_db.clone();
            let writer: RecallWriter = Arc::new(move |row: SignalRow| {
                let store = store_for_writer.clone();
                Box::pin(async move {
                    let hit = RecallHit {
                        note_path: row.note_path.clone(),
                        score: row.score as f64,
                    };
                    store
                        .record_signals(
                            &row.query_text,
                            &row.channel,
                            &[hit],
                            row.session_id.as_deref(),
                            &row.namespace,
                        )
                        .map(|_| ())
                })
            });

            let reflector = Arc::new(MemoryReflector::new(reflector_assembler, prov, writer));
            tool_registry.set_memory_reflector(reflector);
        } else if !daemon {
            println!("  memory_reflect tool: MemoryReflector not wired (no embedder or provider)");
        }

        // Spec 8 Task 8: construct DefaultQueryFiler and inject into memory_reflect tool.
        // Requires a default provider (for the LLM novelty gate).
        // On failure we log and continue — filing is fire-and-forget.
        if app_config.memory.query_filer.enabled {
            if let Some(ref prov) = default_prov {
                use alephcore::memory::notes::query_filer::{DefaultQueryFiler, QueryFiler};
                use alephcore::memory::notes::NoteIndexer;

                let note_dir_opt = if let Some(dir) = note_memory_dir.clone() {
                    Some(dir)
                } else {
                    match alephcore::utils::paths::get_note_memory_dir() {
                        Ok(dir) => Some(dir),
                        Err(e) => {
                            if !daemon {
                                eprintln!(
                                    "Warning: Failed to resolve note memory directory: {}. Query filer disabled.",
                                    e
                                );
                            }
                            tracing::warn!(error = %e, "Failed to resolve note memory directory; query filer disabled");
                            None
                        }
                    }
                };
                if let Some(note_dir) = note_dir_opt {
                    let indexer_arc =
                        std::sync::Arc::new(NoteIndexer::new(note_dir.clone(), memory_db.clone()));
                    let query_filer: std::sync::Arc<dyn QueryFiler> =
                        std::sync::Arc::new(DefaultQueryFiler {
                            store: memory_db.clone(),
                            indexer: indexer_arc,
                            provider: prov.clone(),
                            orientation: orientation.clone(),
                            memory_dir: note_dir,
                            config: app_config.memory.query_filer.clone(),
                        });
                    tool_registry.set_query_filer(query_filer);
                    if !daemon {
                        println!("  query_filer: DefaultQueryFiler wired into memory_reflect");
                    }
                }
            } else if !daemon {
                println!("  query_filer: skipped (no AI provider available)");
            }
        } else if !daemon {
            println!("  query_filer: disabled in config");
        }

        let tool_registry = Arc::new(tool_registry);
        let tool_registry_for_heartbeat = tool_registry.clone();

        // Build task router from config
        let task_router: Option<Arc<dyn alephcore::routing::TaskRouter>> = if app_config
            .task_routing
            .enabled
        {
            let rules =
                alephcore::routing::RoutingRules::from_config(&app_config.task_routing.patterns);
            let mut router = alephcore::routing::CompositeRouter::new(
                rules,
                app_config.task_routing.enable_llm_fallback,
                app_config.task_routing.escalation_step_threshold,
            );
            // Wire LLM classify function using the default provider
            if app_config.task_routing.enable_llm_fallback {
                let classify_provider = provider_registry.default_provider();
                let classify_fn: alephcore::routing::composite_router::LlmClassifyFn = Arc::new(
                    move |msg: &str| {
                        let provider = classify_provider.clone();
                        let prompt = alephcore::routing::llm_classifier::build_classify_prompt(msg);
                        Box::pin(async move {
                            let __msgs = [UnifiedMessage::user(&prompt)];
                            match provider.process(RequestPayload::new(&__msgs)).await {
                                Ok(response) => {
                                    alephcore::routing::llm_classifier::parse_classify_response(
                                        &response.text_content(),
                                    )
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        subsystem = "task_router",
                                        error = %e,
                                        "LLM classification provider call failed, defaulting to simple"
                                    );
                                    Ok(alephcore::routing::TaskRoute::Simple)
                                }
                            }
                        })
                    },
                );
                router = router.with_llm_classify_fn(classify_fn);
                tracing::info!(
                    subsystem = "task_router",
                    event = "llm_classify_wired",
                    "LLM classify function wired to default provider"
                );
            }
            tracing::info!(
                subsystem = "task_router",
                event = "initialized",
                llm_fallback = app_config.task_routing.enable_llm_fallback,
                escalation_threshold = app_config.task_routing.escalation_step_threshold,
                "Task router initialized"
            );
            Some(Arc::new(router))
        } else {
            tracing::info!(
                subsystem = "task_router",
                event = "disabled",
                "Task routing disabled by config"
            );
            None
        };

        // Capture default provider before provider_registry is moved into engine
        default_prov = Some(provider_registry.default_provider());

        // Clone provider registry for topic generation before it's moved into engine
        let topic_provider_registry = provider_registry.clone();

        let engine_config = ExecutionEngineConfig {
            default_timeout_secs: app_config.execution.default_timeout_secs,
            ..Default::default()
        };
        let resilience_db: Option<Arc<alephcore::resilience::StateDatabase>> = {
            use alephcore::utils::paths::get_data_dir;

            match get_data_dir() {
                Ok(dir) => {
                    let db_path = dir.join("state.db");
                    match alephcore::resilience::StateDatabase::new(db_path) {
                        Ok(db) => Some(Arc::new(db)),
                        Err(e) => {
                            if !daemon {
                                eprintln!(
                                    "Warning: Failed to open state.db: {}. Replay trace persistence disabled.",
                                    e
                                );
                            }
                            None
                        }
                    }
                }
                Err(e) => {
                    if !daemon {
                        eprintln!(
                            "Warning: Failed to resolve data directory: {}. Replay trace persistence disabled.",
                            e
                        );
                    }
                    tracing::warn!(error = %e, "Failed to resolve data directory; replay trace persistence disabled");
                    None
                }
            }
        };
        let mut engine = ExecutionEngine::new(
            engine_config,
            provider_registry,
            tool_registry,
            tools,
            Some(memory_db.clone()),
        )
        .with_global_tool_permissions(app_config.policies.tool_permissions.clone());
        if let Some(ref state_db) = resilience_db {
            engine = engine.with_state_database(state_db.clone());
        }
        state_db_out = resilience_db.clone();
        if let Some(router) = task_router {
            engine = engine.with_task_router(router);
        }
        if let Some(ref wm) = workspace_manager {
            engine = engine.with_workspace_manager(wm.clone());
        }

        // Create event-sourced memory command handler (requires state_db + memory_db)
        let command_handler: Option<
            std::sync::Arc<alephcore::memory::events::handler::MemoryCommandHandler>,
        > = if let Some(ref state_db_arc) = resilience_db {
            Some(super::init_command_handler(state_db_arc, memory_db, daemon).await)
        } else {
            None
        };

        // Create compression service for Layer 1 -> Layer 2 fact extraction
        let compression_svc: Option<
            std::sync::Arc<alephcore::memory::compression::CompressionService>,
        > = if let Some(ref emb) = embedder_out {
            default_prov.as_ref().map(|prov| {
                // Spec 6 T10: construct DefaultCompoundIngestor and inject into CompressionService.
                let compound_ingestor: Option<
                    std::sync::Arc<dyn alephcore::memory::notes::ingest::CompoundIngestor>,
                > = if app_config.memory.compound_ingest.enabled {
                    use alephcore::memory::notes::ingest::{
                        ingestor::DefaultCompoundIngestor, retrieve::RelatedBudget,
                    };
                    use alephcore::memory::notes::NoteIndexer;

                    let cfg = &app_config.memory.compound_ingest;
                    let budget = RelatedBudget {
                        max_related_pages: cfg.max_related_pages,
                        preview_char_cap: cfg.related_preview_char_cap,
                        total_byte_cap: cfg.related_total_byte_cap,
                    };
                    let note_dir_opt = if let Some(dir) = note_memory_dir.clone() {
                        Some(dir)
                    } else {
                        match alephcore::utils::paths::get_note_memory_dir() {
                            Ok(dir) => Some(dir),
                            Err(e) => {
                                if !daemon {
                                    eprintln!(
                                        "Warning: Failed to resolve note memory directory: {}. Compound ingestor disabled.",
                                        e
                                    );
                                }
                                tracing::warn!(error = %e, "Failed to resolve note memory directory; compound ingestor disabled");
                                None
                            }
                        }
                    };
                    if let Some(note_dir) = note_dir_opt {
                        let indexer =
                            std::sync::Arc::new(NoteIndexer::new(note_dir.clone(), memory_db.clone()));
                        Some(std::sync::Arc::new(DefaultCompoundIngestor {
                            store: memory_db.clone(),
                            indexer,
                            provider: prov.clone(),
                            embedder: emb.clone(),
                            orientation: orientation.clone(),
                            memory_dir: note_dir,
                            budget,
                            // B5.2: ingest-tail flush wiring deferred to a
                            // follow-up (the manager is constructed locally
                            // here and dropped; passing an Arc requires
                            // hoisting it to a long-lived AppContext field).
                            // For now the legacy reembed_all path handles
                            // vector freshness for the production server.
                            embedding_manager: None,
                            // TODO(C2): construct DefaultNoteWriteGate from
                            // config when governance is enabled. C2.3.2 ships
                            // the mount point; activation is a follow-up.
                            gate: None,
                        }))
                    } else {
                        None
                    }
                } else {
                    None
                };

                super::init_compression_service(
                    memory_db,
                    prov.clone(),
                    emb.clone(),
                    &app_config.policies.memory.compression,
                    daemon,
                    command_handler.clone(),
                    compound_ingestor,
                    profile_synth.clone(),
                )
            })
        } else {
            None
        };

        if let Some(ref cs) = compression_svc {
            engine = engine.with_compression_service(cs.clone());
        }
        compression_out = compression_svc;

        // Start DreamDaemon with command handler so decay mutations are event-sourced.
        // Spec 5 Task 12: thread wiki handle into DreamDaemon for IndexRefresherStage.
        // The embedder is required to build a DreamContext (SkillDistill stage).
        alephcore::memory::ensure_dream_daemon_with_orientation(
            memory_db.clone(),
            std::sync::Arc::new(app_config.memory.clone()),
            default_prov.clone(),
            command_handler,
            orientation.clone(),
            embedder_out.clone(),
            note_memory_dir.clone(),
        );

        // Create session compactor for intra-session context compression
        {
            let sc_config = app_config.policies.memory.session_compactor.clone();
            if sc_config.enabled {
                let mut compactor = alephcore::memory::session_compactor::SessionCompactor::new(
                    memory_db.clone(),
                    sc_config,
                )
                .with_project_scoping(app_config.memory.project_scoped);
                if let Some(ref prov) = default_prov {
                    compactor = compactor.with_provider(prov.clone());
                }
                if let Some(ref emb) = embedder_out {
                    compactor = compactor.with_embedder(emb.clone());
                }
                // Spec 1 G1: wire pre-compress hook so chunks are captured before summarisation.
                compactor = compactor.with_raw_memory_writer(memory_db.clone()
                    as std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>);
                // Spec 4 Task 11: wire capture-filter registry.
                compactor = compactor.with_capture_registry(memory_ext_registry.clone());
                let compactor = std::sync::Arc::new(compactor);
                engine = engine.with_session_compactor(compactor);
                if !daemon {
                    println!("  Session compactor enabled (hierarchical summarization)");
                }
            } else if !daemon {
                println!("  Session compactor: disabled");
            }
        }

        // Wire memory context provider for SQLite-backed prompt augmentation.
        // When an AiProvider is available the assembler drives its LLM re-rank
        // path (Spec 1, B strategy); otherwise the deterministic skeleton runs.
        // Spec 4 Task 11: extension registry threaded in for on_retrieve dispatch.
        // Spec 5 Task 12: wiki handle threaded in for build_orientation_user_message.
        if let Some(ref emb) = embedder_out {
            let mcp = super::init_memory_context_provider_with_extensions(
                memory_db,
                emb.clone(),
                default_prov.clone(),
                app_config.memory.assembler_config(),
                Some(memory_ext_registry.clone()),
                orientation.clone(),
                profile_synth.clone(),
            );
            // Spec A Task 17 — inject MCP into the tool registry so the
            // `remember` tool can resolve the per-agent CuratedMemoryStore.
            if let Err(e) = mcp_cell_for_tools.set(mcp.clone()) {
                tracing::warn!("Failed to set MCP cell for tools: {}", e);
            }

            // Spec A Task 18 — wire post-compression invalidation:
            //   1) compression run completes → MCP drops cached
            //      <CuratedMemory> snapshots for that agent
            //   2) session ends → MCP drops snapshots for that session_key
            // Both hooks are fire-and-forget; cache freshness, never gating.
            if let Some(ref cs) = compression_out {
                let mcp_hook: std::sync::Arc<
                    dyn alephcore::memory::compression::PostCompressionHook,
                > = mcp.clone();
                cs.add_post_hook(mcp_hook).await;
            }
            alephcore::thinker::memory_context_provider::register_session_end_mcp(mcp.clone());

            // Spec B Task 9 — register SessionEndSummarizer for on-session-end
            // summary production. Requires an AiProvider for the synthesizer
            // fallback path; skip silently if none is configured.
            if let Some(ref prov) = default_prov {
                use alephcore::memory::assembler::hybrid::AiProviderReranker;
                use alephcore::memory::session_search_summary::{
                    end_hook::SessionEndSummarizer, synthesizer::SummarySynthesizer,
                };
                let summary_llm: std::sync::Arc<
                    dyn alephcore::memory::session_search_summary::synthesizer::SummaryLlm,
                > = AiProviderReranker::new(prov.clone());
                let synthesizer = std::sync::Arc::new(SummarySynthesizer::new(
                    memory_db.clone()
                        as std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
                    session_store.clone(),
                    summary_llm,
                ));
                let summarizer = std::sync::Arc::new(SessionEndSummarizer {
                    store: memory_db.clone()
                        as std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
                    synthesizer,
                });
                alephcore::thinker::memory_context_provider::register_session_end_summarizer(
                    summarizer,
                );
            }

            mcp_for_orchestrator = Some(mcp.clone());
            engine = engine.with_memory_context_provider(mcp);
        }

        // Wire session manager + event bus for auto-topic generation on first message
        engine = engine.with_session_topic_support(session_store.clone(), event_bus.clone());

        // Wire MediaProcessor for multimodal attachment handling (images, audio)
        {
            use alephcore::media::processor::MediaProcessor;
            use alephcore::media::transcription::TranscriptionService;
            use alephcore::media::whisper::WhisperTranscription;

            // Look up transcription provider from the dedicated transcription_providers config.
            // api_key is #[serde(skip)] and injected from vault at runtime,
            // so we must resolve it from the vault (gen:<provider_name>) before checking.
            let transcription: Option<Box<dyn TranscriptionService>> = {
                let gen_cfg = &app_config.generation;

                // Helper: resolve api_key from vault for a transcription provider
                let resolve_key = |name: &str,
                                   pcfg: &alephcore::GenerationProviderConfig|
                 -> Option<String> {
                    // Use config api_key if present
                    if let Some(ref key) = pcfg.api_key {
                        if !key.is_empty() {
                            return Some(key.clone());
                        }
                    }
                    // Fall back to vault
                    if let Ok(Some(secret)) = shared_token_mgr.get_secret(&format!("gen:{}", name))
                    {
                        let val = secret.expose().to_string();
                        if !val.is_empty() {
                            return Some(val);
                        }
                    }
                    None
                };

                let tcfg = gen_cfg
                    .default_transcription_provider
                    .as_ref()
                    .and_then(|name| {
                        gen_cfg
                            .transcription_providers
                            .get(name)
                            .filter(|pcfg| pcfg.enabled)
                            .and_then(|pcfg| resolve_key(name, pcfg).map(|key| (key, pcfg)))
                    })
                    .or_else(|| {
                        gen_cfg
                            .transcription_providers
                            .iter()
                            .find_map(|(name, pcfg)| {
                                if pcfg.enabled {
                                    resolve_key(name, pcfg).map(|key| (key, pcfg))
                                } else {
                                    None
                                }
                            })
                    });

                if let Some((key, pcfg)) = tcfg {
                    let whisper = WhisperTranscription::new(
                        key,
                        pcfg.base_url.clone(),
                        pcfg.models.first().cloned(),
                    );
                    if !daemon {
                        println!("  MediaProcessor: Whisper transcription enabled (from transcription provider)");
                    }
                    Some(Box::new(whisper))
                } else {
                    None
                }
            };

            // VisionPipeline is not currently created at startup — pass None for now.
            let vision: Option<Arc<alephcore::vision::VisionPipeline>> = None;

            let media_processor = Arc::new(MediaProcessor::new(transcription, vision));
            // Clean up stale cache entries from previous runs
            media_processor.cleanup_stale();

            if !daemon {
                println!("  MediaProcessor initialized");
            }
            engine = engine.with_media_processor(media_processor);
        }

        // Wire team stores into ExecutionEngine for SubagentTool builder methods
        if let Some(ref ts) = team_store {
            engine = engine.with_teammate_manager(Arc::new(
                alephcore::agents::teammates::TeammateManager::new(ts.clone()),
            ));
        }
        if let Some(ref mr) = message_router {
            engine = engine.with_message_router(mr.clone());
        }
        if let Some(ref ib) = inbox {
            engine = engine.with_inbox(ib.clone());
        }

        // Capture the OnceLock cell before wrapping engine in Arc so boot code
        // can inject the orchestrator after initialize_orchestrator completes.
        let orch_cell = engine.orchestrator_cell();
        orch_cell_out = Some(orch_cell);
        let engine = Arc::new(engine);

        // Reconcile agent tasks orphaned by a previous crash: a row still
        // marked `running` means the prior process died mid-flight. Mark them
        // interrupted and report each one. This runs before the engine accepts
        // any request, so no live task can be misclassified.
        if let Some(ref state_db) = resilience_db {
            match state_db.reconcile_orphaned_tasks().await {
                Ok(orphans) => {
                    for task in &orphans {
                        let prompt: String = task.task_prompt.chars().take(120).collect();
                        tracing::warn!(
                            task_id = %task.id,
                            agent_id = %task.agent_id,
                            task_prompt = %prompt,
                            "Reconciling agent task orphaned by a previous run"
                        );
                    }
                    if !orphans.is_empty() {
                        tracing::info!(count = orphans.len(), "Reconciled orphaned agent tasks");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "Failed to reconcile orphaned agent tasks");
                }
            }
        }

        if !app_config.agents.list.is_empty() {
            // New path: use ResolvedAgents from AgentDefinitionResolver
            let mut resolver = alephcore::AgentDefinitionResolver::new();
            let resolved_agents = resolver.resolve_all(&app_config.agents, &app_config.profiles, &app_config.providers);
            for agent in &resolved_agents {
                let config = alephcore::gateway::AgentInstanceConfig::from_resolved(agent);
                let agent_id = config.agent_id.clone();
                let agent_workspace = config.workspace.clone();
                let agent_model = config.model.clone();
                agent_registry
                    .register_config(config, session_store.clone())
                    .await;
                // Emit lifecycle event
                let lifecycle_event =
                    alephcore::gateway::agent_lifecycle::AgentLifecycleEvent::Registered {
                        agent_id: agent_id.clone(),
                        workspace: agent_workspace,
                        model: agent_model,
                    };
                if let Err(e) = event_bus.publish_json(&lifecycle_event) {
                    tracing::warn!("Failed to publish agent lifecycle event: {}", e);
                }
                if !daemon {
                    println!("  Registered agent: {} (lazy)", agent_id);
                }
            }
        } else {
            // Legacy path: use FullGatewayConfig agents
            for agent_config in full_config.get_agent_instance_configs() {
                let agent_id = agent_config.agent_id.clone();
                agent_registry
                    .register_config(agent_config, session_store.clone())
                    .await;
                if !daemon {
                    println!("  Registered agent: {} (lazy)", agent_id);
                }
            }
        }

        if !daemon {
            let provider_name = available_provider_from_env().unwrap_or("config");
            println!("  Mode: Real AgentLoop ({} provider)", provider_name);
            println!();
        }

        let engine_clone = engine.clone();
        let event_bus_clone = event_bus.clone();
        let router_clone = router.clone();
        let agent_registry_clone = agent_registry.clone();
        let app_config_run = app_config_arc.clone();
        let wm_run = workspace_manager.clone();
        server.handlers_mut().register("agent.run", move |req| {
            let engine = engine_clone.clone();
            let event_bus = event_bus_clone.clone();
            let router = router_clone.clone();
            let agent_registry = agent_registry_clone.clone();
            let cfg = app_config_run.clone();
            let wm = wm_run.clone();
            async move {
                handle_run_with_engine(req, engine, event_bus, router, agent_registry, cfg, wm)
                    .await
            }
        });

        // chat.send also uses real ExecutionEngine
        let command_parser_for_chat = command_parser_cell.clone();

        let engine_chat = engine.clone();
        let event_bus_chat = event_bus.clone();
        let router_chat = router.clone();
        let agent_registry_chat = agent_registry.clone();
        let app_config_chat = app_config_arc.clone();
        let wm_chat = workspace_manager.clone();
        let pr_chat = topic_provider_registry.clone();
        let sm_chat = session_store.clone();
        server.handlers_mut().register("chat.send", move |req| {
            let engine = engine_chat.clone();
            let event_bus = event_bus_chat.clone();
            let router = router_chat.clone();
            let agent_registry = agent_registry_chat.clone();
            let cfg = app_config_chat.clone();
            let wm = wm_chat.clone();
            let pr = pr_chat.clone();
            let sm = sm_chat.clone();
            let cp = command_parser_for_chat.clone();
            async move {
                let parser = cp.read().await.clone();
                handle_chat_send_with_engine(
                    req,
                    engine,
                    event_bus,
                    router,
                    agent_registry,
                    cfg,
                    wm,
                    pr,
                    sm,
                    parser,
                )
                .await
            }
        });

        // Phase-2 always overrides phase-1 to guarantee a deterministic response.
        // When state DB is absent, the override returns SERVICE_UNAVAILABLE with
        // a tighter, environment-specific reason — never the phase-1 generic.
        match resilience_db.clone() {
            Some(trace_db) => {
                let trace_list_db = trace_db.clone();
                server.handlers_mut().register("trace.list", move |req| {
                    let db = trace_list_db.clone();
                    async move {
                        alephcore::gateway::handlers::trace_replay::handle_list(req, db).await
                    }
                });

                let trace_get_db = trace_db;
                server.handlers_mut().register("trace.get", move |req| {
                    let db = trace_get_db.clone();
                    async move {
                        alephcore::gateway::handlers::trace_replay::handle_get(req, db).await
                    }
                });
            }
            None => {
                server
                    .handlers_mut()
                    .register("trace.list", |req| async move {
                        alephcore::gateway::protocol::JsonRpcResponse::error(
                            req.id,
                            alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                            "trace.list disabled: no state_database configured".to_string(),
                        )
                    });
                server
                    .handlers_mut()
                    .register("trace.get", |req| async move {
                        alephcore::gateway::protocol::JsonRpcResponse::error(
                            req.id,
                            alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                            "trace.get disabled: no state_database configured".to_string(),
                        )
                    });
            }
        }

        // Capture for inbound router
        let engine_arc: Arc<dyn alephcore::gateway::ExecutionAdapter> = engine;
        exec_adapter = Some(engine_arc.clone());
        agent_reg = Some(agent_registry.clone());
        tool_reg_out = Some(tool_registry_for_heartbeat);

        // Create run_manager with real execution dependencies
        let real_run_manager = Arc::new(AgentRunManager::new(
            router.clone(),
            event_bus.clone(),
            agent_registry.clone(),
            engine_arc.clone(),
        ));
        run_manager = Some(real_run_manager);

        // activity.stats — returns active agent run count + coord task count
        {
            let exec_adapter_for_stats: Arc<dyn alephcore::gateway::ExecutionAdapter> =
                engine_arc.clone();
            let coord_store_for_stats = coord_store.clone();
            server
                .handlers_mut()
                .register("activity.stats", move |req| {
                    let adapter = exec_adapter_for_stats.clone();
                    let store = coord_store_for_stats.clone();
                    async move {
                        alephcore::gateway::handlers::activity::handle_stats(req, adapter, store)
                            .await
                    }
                });
        }

        // Deferred injection: now that ExecutionAdapter exists, build GatewayContext
        // and inject it into BuiltinToolRegistry (via shared OnceCell).
        {
            use alephcore::gateway::context::GatewayContext;
            use alephcore::gateway::inter_agent_policy::AgentToAgentPolicy;

            let a2a_policy = Arc::new(AgentToAgentPolicy::default());
            let mut gateway_ctx_inner = GatewayContext::new(
                session_store.clone(),
                agent_registry,
                engine_arc,
                a2a_policy,
            );
            // Wire the optional ACP adapter pool so the team dispatcher can
            // route tasks owned by AcpSession members through external CLIs.
            // Supports both structured `TeamMember.kind=AcpSession` (main) and
            // legacy `agent_id="acp:<harness>"` prefix (teams-r2 WIP) paths.
            if let Some(ref acp) = acp_manager {
                gateway_ctx_inner = gateway_ctx_inner.with_acp_manager(Arc::clone(acp));
            }
            let gateway_ctx = Arc::new(gateway_ctx_inner);

            // Autonomous team dispatcher — drives the coordination-task DAG to
            // completion. Constructed here because it needs the GatewayContext
            // (agent registry + execution adapter), which only now exists.
            if let (Some(cs), Some(ts)) = (coord_store.clone(), team_store.clone()) {
                use alephcore::teams::context::TeamInboxContextProvider;
                use alephcore::teams::dispatcher::{DispatcherConfig, TeamDispatcher};

                let inbox_provider = inbox.as_ref().map(|ib| {
                    let mut p = TeamInboxContextProvider::new(Arc::clone(ib), Arc::clone(&ts));
                    if let Some(ss) = team_session_store.clone() {
                        p = p.with_session_store(ss);
                    }
                    Arc::new(p)
                });
                let dispatcher = Arc::new(TeamDispatcher::new(
                    cs,
                    ts,
                    artifact_store.clone(),
                    inbox_provider,
                    (*gateway_ctx).clone(),
                    DispatcherConfig::default(),
                    dispatch_signal.clone(),
                ));
                dispatcher.spawn_loop();
                if !daemon {
                    println!("  Team dispatcher started");
                }
            }

            if let Err(e) = gateway_context_cell.set(gateway_ctx) {
                tracing::warn!("Failed to set gateway context cell: {}", e);
            }
        }
    } else {
        if !daemon {
            println!(
                "  Mode: Simulated (set ANTHROPIC_API_KEY or OPENAI_API_KEY for real execution)"
            );
            println!();
        }
        use alephcore::gateway::execution_engine::ExecutionEngineConfig;
        use alephcore::gateway::execution_engine::SimpleExecutionEngine;

        let simple_engine = Arc::new(SimpleExecutionEngine::new(ExecutionEngineConfig::default()));
        let simple_adapter: Arc<dyn alephcore::gateway::ExecutionAdapter> = simple_engine;
        let fallback_agent_registry = Arc::new(AgentRegistry::new());
        let fallback_run_manager = Arc::new(AgentRunManager::new(
            router.clone(),
            event_bus.clone(),
            fallback_agent_registry,
            simple_adapter,
        ));
        let run_manager_clone = fallback_run_manager.clone();
        server.handlers_mut().register("agent.run", move |req| {
            let manager = run_manager_clone.clone();
            async move { handle_run(req, manager).await }
        });

        let rm_chat = fallback_run_manager.clone();
        server.handlers_mut().register("chat.send", move |req| {
            let manager = rm_chat.clone();
            async move { chat_handlers::handle_send(req, manager).await }
        });

        // Fallback: activity.stats with no execution engine
        let coord_store_fallback = coord_store.clone();
        server
            .handlers_mut()
            .register("activity.stats", move |req| {
                let store = coord_store_fallback.clone();
                async move {
                    let active_tasks = if let Some(ref s) = store {
                        s.list_tasks(alephcore::agents::swarm::tasks::CoordTaskFilter {
                            status: Some(
                                alephcore::agents::swarm::tasks::CoordTaskStatus::InProgress,
                            ),
                            ..Default::default()
                        })
                        .await
                        .map(|t| t.len() as u64)
                        .unwrap_or(0)
                    } else {
                        0
                    };
                    alephcore::gateway::protocol::JsonRpcResponse::success(
                        req.id,
                        serde_json::json!({
                            "active_agent_runs": 0u64,
                            "active_coord_tasks": active_tasks,
                            "active_total": active_tasks,
                        }),
                    )
                }
            });
    }

    // Create unified dispatch registry (command discovery + resolution)
    // This is independent of the AI provider — it only maps command names to metadata.
    {
        use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;
        use alephcore::tool_metadata::ToolCatalog;

        let tool_catalog = Arc::new(ToolCatalog::new());

        // Register builtin tools (generate_image, generate_speech, read_skill, list_skills, snapshot)
        tool_catalog.register_builtin_tools().await;

        // Also register executor builtin tools as commands (search, screenshot, ocr, etc.)
        for def in BUILTIN_TOOL_DEFINITIONS {
            use alephcore::tool_metadata::{
                ToolSource as DToolSource, UnifiedTool as DUnifiedTool,
            };
            let tool = DUnifiedTool::new(
                format!("builtin:{}", def.name),
                def.name,
                def.description,
                DToolSource::Builtin,
            );
            tool_catalog.register_with_conflict_resolution(tool).await;
        }

        // Register custom commands from config routing rules
        if !app_config.rules.is_empty() {
            tool_catalog
                .register_custom_commands(&app_config.rules)
                .await;
        }

        // Register skills and plugin tools from ExtensionManager (if initialized)
        {
            use alephcore::domain::Entity;
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            if let Ok(ext_manager) = get_extension_manager() {
                // Ensure extensions are discovered and loaded (skills + plugins)
                if let Err(e) = ext_manager.ensure_loaded().await {
                    tracing::warn!("Failed to load extensions: {}", e);
                }

                {
                    let skill_manifests = ext_manager.skill_system().list_skills().await;
                    let skill_infos: Vec<alephcore::skill::SkillInfo> = skill_manifests
                        .iter()
                        .filter(|s| s.is_user_invocable())
                        .map(|s| alephcore::skill::SkillInfo {
                            id: s.id().as_str().to_string(),
                            name: s.name().to_string(),
                            description: s.description().to_string(),
                            triggers: Vec::new(),
                            allowed_tools: Vec::new(),
                            ecosystem: "aleph".to_string(),
                        })
                        .collect();
                    tool_catalog.register_skills(&skill_infos).await;
                    if !daemon {
                        println!(
                            "  Dispatch registry: {} skills registered",
                            skill_infos.len()
                        );
                    }
                }

                // Register plugin commands (from CC-format plugins' commands/ directories)
                {
                    let commands = ext_manager.get_all_commands().await;
                    let command_skill_infos: Vec<alephcore::skill::SkillInfo> = commands
                        .iter()
                        .filter(|cmd| cmd.plugin_name.is_some())
                        .map(|cmd| {
                            let id = if let Some(ref plugin) = cmd.plugin_name {
                                format!("{}:{}", plugin, cmd.name)
                            } else {
                                cmd.name.clone()
                            };
                            alephcore::skill::SkillInfo {
                                id,
                                name: cmd.name.clone(),
                                description: cmd.description.clone(),
                                triggers: Vec::new(),
                                allowed_tools: Vec::new(),
                                ecosystem: "plugin".to_string(),
                            }
                        })
                        .collect();

                    if !command_skill_infos.is_empty() {
                        tool_catalog.register_skills(&command_skill_infos).await;
                        if !daemon {
                            println!(
                                "  Dispatch registry: {} plugin commands registered",
                                command_skill_infos.len()
                            );
                        }
                    }
                }

                // Register plugin tools from discovered manifests
                {
                    let registry = ext_manager.get_plugin_registry().await;
                    let plugin_tools: Vec<(String, String, String)> = registry
                        .list_plugins()
                        .into_iter()
                        .filter(|p| p.status.is_active())
                        .flat_map(|plugin| {
                            match alephcore::extension::manifest::parse_manifest_from_dir_sync(
                                &plugin.root_dir,
                            ) {
                                Ok(manifest) => manifest
                                    .tools_v2
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|t| {
                                        (
                                            plugin.id.clone(),
                                            t.name.clone(),
                                            t.description.unwrap_or_default(),
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                                Err(_) => Vec::new(),
                            }
                        })
                        .collect();

                    if !plugin_tools.is_empty() {
                        tool_catalog.register_plugin_tools(&plugin_tools).await;
                        if !daemon {
                            println!(
                                "  Dispatch registry: {} plugin tools registered",
                                plugin_tools.len()
                            );
                        }
                    }
                }
            }
        }

        if !daemon {
            println!("  Dispatch registry initialized");
        }

        // Wire commands.list to use unified dispatch registry instead of hardcoded builtins
        {
            let reg = tool_catalog.clone();
            server.handlers_mut().register("commands.list", move |req| {
                let registry = reg.clone();
                async move {
                    alephcore::gateway::handlers::commands::handle_list_from_registry(
                        req, &registry,
                    )
                    .await
                }
            });
            if !daemon {
                println!("  commands.list: wired to unified dispatch registry");
            }
        }

        // Wire tools.catalog to return all active tools grouped by source
        {
            let reg = tool_catalog.clone();
            server.handlers_mut().register("tools.catalog", move |req| {
                let registry = reg.clone();
                async move {
                    alephcore::gateway::handlers::tools_visibility::handle_catalog(req, &registry)
                        .await
                }
            });
            if !daemon {
                println!("  tools.catalog: wired to unified dispatch registry");
            }
        }

        // Wire tools.invoke to execute a single builtin tool directly,
        // bypassing the LLM agent loop. Intended for E2E test harnesses
        // (note_layer probes, deterministic tool exercising). Production
        // callers should still go through agent.run.
        //
        // D2/P3 fix: pass the live AgentRegistry so handle_invoke can apply
        // the same allowlist that the LLM faces — preventing arbitrary
        // operators from bypassing per-agent tool scoping.
        //
        // Only wire when a real BuiltinToolRegistry is present (real mode);
        // in simulated mode the SERVICE_UNAVAILABLE placeholder from
        // HandlerRegistry::new remains.
        if let Some(reg) = tool_reg_out.clone() {
            let agents_for_invoke: std::sync::Arc<alephcore::agents::AgentRegistry> = {
                let r = std::sync::Arc::new(alephcore::agents::AgentRegistry::with_builtins());
                let aleph_home = alephcore::discovery::aleph_home_dir().ok();
                let project_dir = std::env::current_dir().ok();
                if let Some(home) = aleph_home.as_deref() {
                    if let Err(e) = r.register_from_dirs(home, project_dir.as_deref()) {
                        tracing::warn!(
                            error = %e,
                            "tools.invoke: failed to load user agent defs; allowlist degrades to builtins-only"
                        );
                    }
                }
                r
            };
            server.handlers_mut().register("tools.invoke", move |req| {
                let registry = reg.clone();
                let agents = Some(agents_for_invoke.clone());
                async move {
                    alephcore::gateway::handlers::tools_invoke::handle_invoke(req, registry, agents)
                        .await
                }
            });
            if !daemon {
                println!("  tools.invoke: wired with agent allowlist gating");
            }
        }

        // Wire tools.effective to return tools available to a specific agent.
        // D1 fix: previously rebuilt a builtins-only AgentRegistry per call,
        // hiding user-customized agents. Mirror the orchestrator's setup
        // (mod.rs ~1112 + orchestrator_init.rs:70) by loading user/project
        // AgentDefs from filesystem so the visibility surface matches what
        // the agent loop actually sees.
        {
            let reg = tool_catalog.clone();
            let agent_def_registry = {
                let r = std::sync::Arc::new(alephcore::agents::AgentRegistry::with_builtins());
                let aleph_home = alephcore::discovery::aleph_home_dir().ok();
                let project_dir = std::env::current_dir().ok();
                if let Some(home) = aleph_home.as_deref() {
                    match r.register_from_dirs(home, project_dir.as_deref()) {
                        Ok(shadows) => {
                            if !daemon && !shadows.is_empty() {
                                println!(
                                    "  tools.effective: loaded user agents (+{} shadow overrides)",
                                    shadows.len()
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "tools.effective: failed to load user agent defs; \
                                 falling back to builtins-only"
                            );
                        }
                    }
                }
                r
            };
            server
                .handlers_mut()
                .register("tools.effective", move |req| {
                    let registry = reg.clone();
                    let agents = agent_def_registry.clone();
                    async move {
                        let agent_id = req
                            .params
                            .as_ref()
                            .and_then(|p| p.get("agent_id"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let agent_def = match &agent_id {
                            Some(id) => agents.get(id),
                            None => agents.get("main"),
                        };
                        alephcore::gateway::handlers::tools_visibility::handle_effective(
                            req,
                            &registry,
                            agent_def.as_ref(),
                        )
                        .await
                    }
                });
            if !daemon {
                println!("  tools.effective: wired to unified dispatch registry + agent defs");
            }
        }

        // Wire tools.cancel_call + tools.in_flight against the process-wide
        // in-flight registry installed in `start/mod.rs` Gap-B-follow-up boot
        // path. `tools.cancel_call` fires the harness-issued per-call
        // CancellationToken keyed by `tool_call_id`; `tools.in_flight` lists
        // every live registration for CLI / panel diagnostics.
        if let Some(reg) = alephcore::tools::in_flight::global_in_flight_tool_calls() {
            let reg_cancel = reg.clone();
            server
                .handlers_mut()
                .register("tools.cancel_call", move |req| {
                    let r = reg_cancel.clone();
                    async move {
                        alephcore::gateway::handlers::tools_cancel::handle_cancel(req, r).await
                    }
                });
            let reg_list = reg.clone();
            server
                .handlers_mut()
                .register("tools.in_flight", move |req| {
                    let r = reg_list.clone();
                    async move {
                        alephcore::gateway::handlers::tools_cancel::handle_in_flight(req, r).await
                    }
                });
            if !daemon {
                println!("  tools.cancel_call + tools.in_flight: wired to in-flight registry");
            }
        }

        // Wire command.execute to resolve slash commands via CommandParser + ToolRegistry
        {
            let parser = Arc::new(alephcore::command::CommandParser::new(tool_catalog.clone()));

            // Inject parser into chat.send handler (created earlier, uses deferred cell)
            {
                let mut cell = command_parser_cell.write().await;
                *cell = Some(parser.clone());
            }

            let reg = tool_catalog.clone();
            server
                .handlers_mut()
                .register("command.execute", move |req| {
                    let p = parser.clone();
                    let r = reg.clone();
                    async move {
                        alephcore::gateway::handlers::commands::handle_execute(req, p, r).await
                    }
                });
            if !daemon {
                println!("  command.execute: wired to unified command parser + registry");
            }
        }

        tool_catalog_out = Some(tool_catalog);

        // ── Spec 4 Task 11: spawn MemoryProducerScheduler ────────────────────
        {
            use alephcore::memory::extensions::MemoryProducerScheduler;
            let raw_store: std::sync::Arc<
                dyn alephcore::memory::store::raw_memory::RawMemoryStore,
            > = memory_db.clone();
            let scheduler = std::sync::Arc::new(MemoryProducerScheduler::new(
                memory_ext_registry.clone(),
                raw_store,
            ));
            let _scheduler_handle = scheduler.spawn();
            // JoinHandle intentionally leaked here — the task runs for the
            // server lifetime. Shutdown via process exit.
            if !daemon {
                println!("  MemoryProducerScheduler: spawned");
            }
        }

        // ── Spec 4 Task 11: wire memory_registry into ExtensionManager ───────
        // After the registry is constructed we inject it into the global
        // ExtensionManager so any plugin loaded at runtime via
        // `load_runtime_plugin` / `ensure_plugin_loaded` also gets
        // `load_plugin_with_memory` (MCP memory extension auto-registration).
        {
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            if let Ok(ext_manager) = get_extension_manager() {
                // ExtensionManager is stored behind Arc; we can only thread the
                // registry through the methods that take `&self`.
                // Inject via set_memory_registry if available (no-op if the
                // Arc is already shared across threads).
                ext_manager.set_memory_registry(memory_ext_registry.clone());
            }
        }
    }

    // Register status/cancel (work for both real and simulated modes)
    if let Some(ref rm) = run_manager {
        let rm_status = rm.clone();
        server.handlers_mut().register("agent.status", move |req| {
            let manager = rm_status.clone();
            async move { handle_agent_status(req, manager).await }
        });

        let rm_cancel = rm.clone();
        server.handlers_mut().register("agent.cancel", move |req| {
            let manager = rm_cancel.clone();
            async move { handle_agent_cancel(req, manager).await }
        });

        // Register chat handlers (abort, history, clear work for both real and simulated)
        let rm_abort = rm.clone();
        server.handlers_mut().register("chat.abort", move |req| {
            let manager = rm_abort.clone();
            async move { chat_handlers::handle_abort(req, manager).await }
        });
    }

    let sm_history = session_store.clone();
    server.handlers_mut().register("chat.history", move |req| {
        let manager = sm_history.clone();
        async move { chat_handlers::handle_history(req, manager).await }
    });

    let sm_clear = session_store;
    server.handlers_mut().register("chat.clear", move |req| {
        let manager = sm_clear.clone();
        async move { chat_handlers::handle_clear(req, manager).await }
    });

    // agent.respondToInput is stateless (no context args)
    server
        .handlers_mut()
        .register("agent.respondToInput", |req| async move {
            handle_respond_to_input(req).await
        });

    // agent.list — returns available agents from the router
    {
        let router_list = router.clone();
        server.handlers_mut().register("agent.list", move |req| {
            let r = router_list.clone();
            async move { agent_handlers::handle_list(req, r).await }
        });
    }

    if !daemon {
        println!("Agent control methods:");
        println!("  - agent.run            : Execute agent request with streaming");
        println!("  - agent.status         : Query run status by run_id");
        println!("  - agent.cancel         : Cancel an active run");
        println!("  - agent.list           : List available agents");
        println!("  - agent.respondToInput : Respond to user input request");
        println!("  - chat.send            : Send chat message (wraps agent.run)");
        println!("  - chat.abort           : Abort message generation");
        println!("  - chat.history         : Get chat history");
        println!("  - chat.clear           : Clear chat history");
        println!();
    }

    // G4: register gateway.identity.get with a small captured snapshot.
    // The snapshot deliberately omits Arc<GatewaySharedState> — capturing
    // the handler-registry Arc here would make this very handlers_mut()
    // call (which uses Arc::get_mut) panic.
    {
        use alephcore::gateway::handlers::gateway_identity::{
            handle_gateway_identity_get, GatewayIdentitySnapshot,
        };
        // +1 accounts for the gateway.identity.get handler itself, which
        // is registered immediately after this count is taken.
        let method_count = server.handlers_mut().len() + 1;
        let identity_snapshot = GatewayIdentitySnapshot {
            instance_id: server.instance_id.clone(),
            started_at_unix: server.started_at_unix,
            state_versions: server.state_versions.clone(),
            method_count,
        };
        server
            .handlers_mut()
            .register("gateway.identity.get", move |req| {
                let snap = identity_snapshot.clone();
                async move { handle_gateway_identity_get(req, snap).await }
            });
        if !daemon {
            println!("  gateway.identity.get: wired");
        }
    }

    // G4b: register gateway.metrics.lanes with the live LaneManager.
    // Cloning Arc<LaneManager> is cheap; the handler reads available_permits()
    // off the underlying tokio::Semaphore — racy by design (gauge, not txn).
    {
        use alephcore::gateway::handlers::gateway_metrics::handle_gateway_metrics_lanes;
        let lane_mgr = server.lane_manager.clone();
        server
            .handlers_mut()
            .register("gateway.metrics.lanes", move |req| {
                let mgr = lane_mgr.clone();
                async move { handle_gateway_metrics_lanes(req, mgr).await }
            });
        if !daemon {
            println!("  gateway.metrics.lanes: wired");
        }
    }

    // G4c: register gateway.credentials with a snapshot of the live
    // GatewayServerConfig. Cloning the config into an Arc once at boot is
    // cheap and avoids holding the FullGatewayConfig across the handler.
    {
        use alephcore::gateway::handlers::gateway_credentials::handle_gateway_credentials;
        let gateway_cfg = std::sync::Arc::new(full_config.gateway.clone());
        server
            .handlers_mut()
            .register("gateway.credentials", move |req| {
                let cfg = gateway_cfg.clone();
                async move { handle_gateway_credentials(req, cfg).await }
            });
        if !daemon {
            println!("  gateway.credentials: wired");
        }
    }

    // G2: signal readiness. /ready returns 200 from this point onward;
    // before this, it returns 503 so proxies don't route to a gateway
    // whose handler tree is still being wired.
    server
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    if !daemon {
        println!("  Gateway readiness: signaled (ready=true)");
    }

    AgentHandlersResult {
        _run_manager: run_manager
            .expect("run_manager must be set in both real and simulated modes"),
        execution_adapter: exec_adapter,
        agent_registry: agent_reg,
        default_provider: default_prov,
        tool_catalog: tool_catalog_out,
        embedder: embedder_out,
        compression_service: compression_out,
        multi_registry: multi_reg,
        channel_registry_cell: channel_reg_cell,
        clarification_manager_cell: clar_mgr_cell,
        generation_registry: Some(generation_registry),
        tool_registry: tool_reg_out,
        team_store,
        coord_task_store: coord_store,
        snapshot_store,
        state_db: state_db_out,
        event_store: event_store.clone(),
        // Concrete Arc<SqliteArtifactStore> upcast to the trait object so
        // `register_teams_handlers` can consume it without knowing the
        // concrete impl. SqliteArtifactStore impls ArtifactStore.
        artifact_store: artifact_store
            .clone()
            .map(|s| s as Arc<dyn alephcore::teams::artifacts::ArtifactStore>),
        message_router: message_router.clone(),
        orchestrator_cell: orch_cell_out,
        memory_context_provider: mcp_for_orchestrator,
        memory_backend: Some(memory_db.clone()),
        arena_manager: Some(arena_manager),
    }
}
