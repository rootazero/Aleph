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
//! - [`coord_stores`] — `coord.db` + `teams.db` `SQLite` store initialization
//! - [`generation_init`] — generation provider registry + hot-reload
//! - this module — orchestration + signature

mod common_handlers;
mod coord_stores;
mod generation_init;
mod provider_registry;
mod tool_catalog_init;

use common_handlers::{register_common_handlers, register_trace_handlers};
use coord_stores::{
    build_team_components, init_coord_and_snapshot, init_team_store, init_teams_evolution_stores,
};
use generation_init::init_generation_registry;
use provider_registry::build_multi_provider_registry;
use tool_catalog_init::init_tool_catalog;

use alephcore::sync_primitives::{Arc, RwLock};

use alephcore::executor::BuiltinToolRegistry;
use alephcore::gateway::handlers::agent::{handle_run, AgentRunManager};
use alephcore::gateway::handlers::chat as chat_handlers;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{
    available_provider_from_env, AgentRegistry, ExecutionEngine, ExecutionEngineConfig,
    GatewayConfig as FullGatewayConfig,
};
use alephcore::ProviderRegistry;

use crate::server_init::{handle_chat_send_with_engine, handle_run_with_engine};

/// Result from registering agent handlers — includes optional execution support
/// for use by `InboundMessageRouter`.
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
    /// Deferred injection cell for `ChannelRegistry` (created after agent handlers)
    pub channel_registry_cell: Option<
        Arc<tokio::sync::OnceCell<Arc<alephcore::gateway::channel_registry::ChannelRegistry>>>,
    >,
    /// Deferred injection cell for `ClarificationManager` (enables the `ask_user` tool)
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
    /// Wake handle for the autonomous team dispatcher, shared with the inbound
    /// router so a reply to a paused workflow `clarify` step can complete its
    /// durable task and wake the dispatcher to unblock dependents.
    pub dispatch_signal: Option<Arc<tokio::sync::Notify>>,
    /// Snapshot store for `teams.snapshot.*` RPC handlers. Shares the coord
    /// connection — None if coord init failed.
    pub snapshot_store: Option<Arc<alephcore::teams::SqliteSnapshotStore>>,
    /// State database (~/.aleph/data/state.db). Holds `task_traces` rows that
    /// the `teams.usage` RPC + `team_usage` tool aggregate by team-member
    /// `agent_id`. `None` in simulated mode.
    pub state_db: Option<Arc<alephcore::resilience::StateDatabase>>,
    /// Event log store for team event logging
    pub event_store: Option<Arc<dyn alephcore::teams::events::EventLogStore>>,
    /// Artifact store for teams.task.trace and team artifact tools (R3).
    /// Trait-object handle is what gateway handlers consume; the concrete
    /// `SqliteArtifactStore` is still used internally by tool wiring.
    pub artifact_store: Option<Arc<dyn alephcore::teams::artifacts::ArtifactStore>>,
    /// Message router for the `TeamNotifier` event handler
    pub message_router: Option<Arc<alephcore::teams::messages::MessageRouter>>,
    /// Raw message store, exposed for `register_teams_handlers` so
    /// `teams.chat.history` and `agents.teams` enrichment can be wired.
    pub message_store: Option<Arc<dyn alephcore::teams::messages::MessageStore>>,
    /// `OnceLock` handle shared with the real `ExecutionEngine`. Boot code calls
    /// `.set(orchestrator)` on this after `initialize_orchestrator` returns so
    /// the run loop, steering rescue, and gate paths can resolve the
    /// orchestrator from the engine's own field rather than an external argument.
    pub orchestrator_cell: Option<
        std::sync::Arc<std::sync::OnceLock<std::sync::Arc<alephcore::orchestrator::Orchestrator>>>,
    >,
    /// Phase 6 follow-up: `MemoryContextProvider` exposed for the orchestrator
    /// path so `AgentHarnessRunner` can build the per-turn system prompt with
    /// curated memory + hybrid retrieval. `ExecutionEngine` receives a clone via
    /// `with_memory_context_provider`; the orchestrator path needs its own
    /// reference because it does not hold an `ExecutionEngine`.
    pub memory_context_provider: Option<Arc<alephcore::thinker::MemoryContextProvider>>,
    /// `SQLite` memory backend exposed for the orchestrator path so the per-run
    /// `ContextCompactor` can reuse hierarchical session summaries for
    /// zero-API-cost compaction.
    pub memory_backend: Option<alephcore::memory::store::MemoryBackend>,
    /// Memory-extension registry (capture-filter + `on_delegation` hooks).
    /// Exposed so the A2A outbound `A2ASubAgent` can route its delegation
    /// writes through the same capture filter the engine/compactor use;
    /// without it, A2A delegation memory bypasses the filter (direct insert).
    pub memory_ext_registry: Arc<alephcore::memory::extensions::MemoryExtensionRegistry>,
}

/// Register agent.run / agent.status / agent.cancel / chat.* handlers.
/// Selects real `ExecutionEngine` when an API key is available (env or config),
/// otherwise uses the simulated `AgentRunManager`.
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
    // Cluster membership store: the `role=node` device records behind
    // `node_manage`. The same Arc the gateway auth layer and `cluster.enroll`
    // write through — a second store would fork the fleet roster.
    security_store: Arc<alephcore::gateway::security::SecurityStore>,
    // Spec 5 Task 12: wiki orientation handle for DreamDaemon + MemoryContextProvider + tools.
    orientation: Option<std::sync::Arc<dyn alephcore::memory::notes::orientation::NoteOrientation>>,
    note_memory_dir: Option<std::path::PathBuf>,
    // Phase 3 Task 8: sandbox for exec-class tools.
    sandbox: Option<Arc<dyn alephcore::sandbox::Sandbox>>,
    // P4 T5: store tools dep seam — catalog cache and marketplace configs.
    // `None` if the catalog DB failed to open (store tools degrade gracefully).
    catalog_cache: Option<std::sync::Arc<alephcore::hub::cache::CatalogCache>>,
    hub_marketplace_configs: Option<
        std::collections::HashMap<
            String,
            alephcore::extension::marketplace::types::MarketplaceConfig,
        >,
    >,
    // P4 T7: live MCP manager handle for `hub_install_run`. The SAME shared
    // handle the gateway `extensions.*` handlers use — it cannot be
    // reconstructed. `None` → agent-driven MCP installs degrade gracefully.
    hub_mcp_handle: Option<alephcore::mcp::manager::McpManagerHandle>,
    // Whiteboard canvas store for the `canvas` tool — the SAME Arc the
    // `canvas.*` RPC handlers hold (only that instance carries the event
    // bus). `None` when the canvas root could not be created at boot.
    canvas_store: Option<Arc<alephcore::canvas::CanvasStore>>,
) -> alephcore::Result<AgentHandlersResult> {
    // Assigned in both the real-execution branch and the simulated branch
    // below; deferred init keeps the dead initial value out (and lets the
    // compiler prove both modes set it before the `.expect(...)` read).
    let run_manager: Option<Arc<AgentRunManager>>;
    let mut exec_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>> = None;
    let mut agent_reg: Option<Arc<AgentRegistry>> = None;
    let mut default_prov: Option<Arc<dyn alephcore::providers::AiProvider>> = None;
    let tool_catalog_out: Option<Arc<alephcore::tool_metadata::ToolCatalog>>;
    // One runtime tool-health cache, shared by the `ExecutionEngine` and the
    // `ToolCatalog`. It has to be created out here, ahead of both: the engine is
    // wrapped in an `Arc` long before `init_tool_catalog` runs, so it cannot be
    // handed the catalog's own cache afterwards. Sharing one handle is what
    // connects "probes registered at boot" to "the model never receives the
    // schema of a tool whose dependency is dead".
    let tool_health = Arc::new(alephcore::tool_metadata::ToolHealthCache::new());
    let mut embedder_out: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>> = None;
    // Long-lived embedding manager (B5.2): hoisted so the compound ingestor's
    // embedding queue has a real producer/consumer instead of the manager
    // being constructed locally and dropped.
    let mut embedding_manager_out: Option<std::sync::Arc<alephcore::memory::EmbeddingManager>> =
        None;
    let mut compression_out: Option<
        std::sync::Arc<alephcore::memory::compression::CompressionService>,
    > = None;
    // Assigned unconditionally below (from `build_multi_provider_registry`)
    // before any read, so deferred init keeps the dead initial value out.
    let multi_reg: Option<Arc<alephcore::MultiProviderRegistry>>;
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

    let resolve_note_memory_dir = || -> Option<std::path::PathBuf> {
        if let Some(ref dir) = note_memory_dir {
            return Some(dir.clone());
        }
        match alephcore::utils::paths::get_note_memory_dir() {
            Ok(dir) => Some(dir),
            Err(e) => {
                if !daemon {
                    eprintln!("Warning: Failed to resolve note memory directory: {e}.");
                }
                tracing::warn!(error = %e, "Failed to resolve note memory directory");
                None
            }
        }
    };
    let note_dir = resolve_note_memory_dir();

    // Coord + snapshot stores (single shared connection to coord.db).
    let (coord_store, snapshot_store) = init_coord_and_snapshot(event_bus.clone(), daemon).await;

    // Team store (teams.db).
    let team_store = init_team_store(daemon).await;

    // Teams-evolution stores (artifact / event log / message / session, all on teams.db).
    let (artifact_store, event_store, message_store, team_session_store) =
        init_teams_evolution_stores(daemon).await;

    // Map the optional [team_messages] TOML onto EscalationRule (the third
    // teams storm/escalation guard, alongside [team_dispatcher] / [team_broadcast]).
    // Each field falls back to the live EscalationRule::default() (no default
    // duplication / drift). A threshold of 0 would escalate on the first reply,
    // so 0 ⇒ default (P7 boundary clamp); escalation_enabled is honoured
    // verbatim (including false) so operators can disable escalation.
    let team_escalation_rules = {
        use alephcore::teams::messages::EscalationRule;
        let d = EscalationRule::default();
        match &app_config.team_messages {
            None => d,
            Some(t) => EscalationRule {
                thread_message_threshold: t
                    .thread_message_threshold
                    .filter(|&v| v > 0)
                    .unwrap_or(d.thread_message_threshold),
                enabled: t.escalation_enabled.unwrap_or(d.enabled),
            },
        }
    };
    // Higher-level team components derived from the four stores above.
    let (message_router, inbox, session_coordinator) = build_team_components(
        &message_store,
        &event_store,
        &team_session_store,
        &artifact_store,
        &team_store,
        team_escalation_rules,
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

    // Build MultiProviderRegistry: register ALL configured providers (vault
    // key hydration, deterministic default selection). The returned registry
    // (if any) is also published through the outer `multi_reg` Option, which
    // downstream wiring (`fallback_providers`) and the final
    // `AgentHandlersResult` both read. See `provider_registry.rs`.
    let provider_registry = build_multi_provider_registry(app_config, &shared_token_mgr, daemon);
    multi_reg = provider_registry.clone();

    // `[general] fallback_providers` used to be copied onto the registry here.
    // Nothing routed off that copy — its only reader was the registry's own
    // pre-request resolver, which named a badge and dialed nothing. The key is
    // now read where the chain that actually gets walked is assembled
    // (`deps_builder::provider_chain::assemble_fallbacks`, as the back-compat
    // source when `[fallback_provider].chain` is empty), so it finally does what
    // its doc comment has always claimed.
    if !app_config.general.fallback_providers.is_empty() && !daemon {
        println!(
            "  Fallback chain: {:?}",
            app_config.general.fallback_providers
        );
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
        )))
        .expect("first registration into a fresh registry cannot collide");
        std::sync::Arc::new(reg)
    };

    // Shared wake handle for the autonomous team dispatcher. `task_create`
    // notifies it; the dispatcher (constructed later, once GatewayContext
    // exists) waits on it. Declared at function scope so it can be surfaced on
    // AgentHandlersResult — the inbound router needs it to wake the dispatcher
    // when a reply resolves a paused workflow `clarify` step.
    let dispatch_signal = std::sync::Arc::new(tokio::sync::Notify::new());

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
                    // Keep the manager alive: its pending queue feeds the
                    // compound ingestor's write-tail embedding flush.
                    if provider.is_some() {
                        embedding_manager_out = Some(std::sync::Arc::new(manager));
                    }
                    provider
                }
                Err(e) => {
                    if !daemon {
                        eprintln!("  Warning: Failed to initialize embedding provider: {e}");
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
        //
        // The operator's `[ssrf] allow_private_network` switch is threaded
        // into provider construction: it is what admits a self-hosted SearXNG
        // on the LAN (loopback / RFC1918 base_url). Cloud metadata endpoints
        // stay refused under every policy — the switch is not a waiver for
        // those, only for internal services.
        let search_registry = alephcore::search::SearchRegistry::from_config(
            app_config.search.as_ref(),
            app_config.ssrf.allow_private_network,
        )
        .map(Arc::new);

        // `[search]` is a declared-live config section: publish the registry
        // behind a process-global `SearchHandle` so a Panel `search_config.update`
        // (or a `config.patch` of `search.*`) hot-applies via
        // `config::live_apply` instead of waiting for a restart. The seed is
        // exactly the registry the tool face would have resolved without a
        // handle (configured, else bare Tavily key, else empty), so installing
        // the handle changes nothing until the first write lands. The vault
        // `Arc` travels with it because a patched `Config` provably carries no
        // API keys (`SearchBackendConfig::api_key` is `skip_serializing`) and
        // the live rebuild must re-resolve them — see `search::handle`'s doc.
        let search_handle = Arc::new(alephcore::search::SearchHandle::new(
            search_registry.unwrap_or_else(|| {
                alephcore::search::SearchRegistry::for_tool(None, tavily_api_key.as_deref())
            }),
            shared_token_mgr.clone(),
        ));
        let _ = alephcore::search::install_global_search_handle(search_handle.clone());

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
            note_dir.clone().map(|note_dir| {
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

        // Strategic planner provider — resolved ONCE here, above the Think→Act
        // loop (R10). Default-on: absent [strategy] ⇒ enabled. The builder
        // returns None for BOTH "disabled" and "no planner_model set"; None means
        // "use the executor's model", so when enabled we fall back to the default
        // provider (else the planner would never fire in the default config).
        // enabled=false is the only off switch.
        let planner_provider = if app_config.strategy.as_ref().is_none_or(|s| s.enabled) {
            let primary_provider_key = app_config
                .general
                .default_provider
                .clone()
                .unwrap_or_default();
            Some(
                alephcore::orchestrator::build_strategy_planner_provider(
                    app_config,
                    &primary_provider_key,
                )
                .unwrap_or_else(|| provider_registry.default_provider()),
            )
        } else {
            None
        };

        // Round 2: the engine's copy of the planner provider is additionally
        // gated on [strategy].plan_naked_loop (default true). `None` here ⇒ the
        // naked agent-loop first-message planner trigger in execute.rs stays
        // dormant, while goal/loop/workflow tools keep their (enabled-gated)
        // planner_provider. Cloned BEFORE planner_provider is moved into
        // tool_config below (E0382 guard). `enabled` is already folded in:
        // planner_provider is None when [strategy].enabled = false.
        let naked_loop_planner_provider = if app_config
            .strategy
            .as_ref()
            .is_none_or(|s| s.plan_naked_loop)
        {
            planner_provider.clone()
        } else {
            None
        };

        // Round 2 (teams): the team-chat planner provider, gated additionally on
        // [strategy].plan_team (default true). `None` ⇒ the first-message team
        // planner + leader-first hard gate in the broadcaster stay dormant.
        // Cloned BEFORE planner_provider is moved into tool_config (E0382).
        // `enabled` already folded in (planner_provider is None when disabled).
        let team_planner_provider = if app_config.strategy.as_ref().is_none_or(|s| s.plan_team) {
            planner_provider.clone()
        } else {
            None
        };

        // Build tool config with memory backend, embedder, search API key, and agent management deps
        let tool_config = alephcore::executor::BuiltinToolConfig {
            memory_db: Some(memory_db.clone()),
            embedder,
            tavily_api_key,
            search_handle: Some(search_handle.clone()),
            agent_registry: Some(agent_registry.clone()),
            workspace_manager: workspace_manager.clone(),
            event_bus: Some(event_bus.clone()),
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
            planner_provider,
            catalog_cache,
            hub_marketplace_configs,
            hub_mcp_handle,
            // The gateway's own canvas store Arc — see the parameter doc.
            canvas_store: canvas_store.clone(),
            ..Default::default()
        };
        let mut tool_registry = BuiltinToolRegistry::with_config(tool_config).await?;

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

        // Runtime-map completion: the registry constructor conditionally
        // registers every executable tool — with full parameter schemas —
        // into its unified metadata map, but the loop tool list above is
        // sourced from the static BUILTIN_TOOL_DEFINITIONS. A hand-maintained
        // patch list used to bridge the gap (scratchpad / voice_mode_set /
        // goal) and silently drifted: meta discovery tools (`list_tools` —
        // which act.rs error nudges tell the model to call), provider-gated
        // generation tools (video/audio/speech_generate), store-gated team
        // tools (`task_exit_journal` — "LLM-self-called" per its registration
        // comment), and the desktop capability family were all dispatchable
        // yet never advertised to the model. Complete the list from the map
        // itself so registration is the single source of truth: anything the
        // registry can execute, the model can see. Conditional registration
        // already guarantees map entries have live dependencies, and the
        // appended tail is sorted for a deterministic tool order
        // (prompt-cache stability).
        {
            let existing: std::collections::HashSet<String> =
                tools.iter().map(|t| t.name.clone()).collect();
            let mut completion: Vec<UnifiedTool> = tool_registry
                .unified_tools()
                .filter(|t| !existing.contains(&t.name))
                .cloned()
                .collect();
            completion.sort_by(|a, b| a.name.cmp(&b.name));
            if !completion.is_empty() {
                tracing::info!(
                    count = completion.len(),
                    "LLM tool list completed from registry runtime map"
                );
                tools.extend(completion);
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
        // Ensure default provider is available for the reflector / query-filer
        // wiring below.  Previously this assignment happened ~140 lines later,
        // which left both subsystems permanently disabled at startup.
        default_prov = Some(provider_registry.default_provider());
        if let (Some(emb), Some(prov)) = (embedder_out.clone(), default_prov.clone()) {
            use alephcore::memory::reflector::{
                fs_reflector::RecallWriter, recall_signals::SignalRow, MemoryReflector,
            };
            use alephcore::memory::store::sqlite::recall_signals::RecallHit;

            // Build a fresh assembler dedicated to the reflector (same config as MCP).
            // This provider is built for its `assembler()` alone and never
            // renders the curated envelope, so the section is passed for
            // uniformity rather than need — but it is passed, because a second
            // construction site quietly holding different budgets from the
            // first is exactly how `[memory.curated]` went inert to begin with.
            let reflector_mcp = super::init_memory_context_provider(
                memory_db,
                Some(emb),
                Some(prov.clone()),
                app_config.memory.assembler_config(),
                app_config.memory.injection_mode,
                app_config.memory.curated.into(),
                app_config.memory.orientation.max_tokens,
            );
            let reflector_assembler = reflector_mcp.assembler();

            // Build the recall-signal writer closure: translates SignalRow → record_signals.
            let store_for_writer = memory_db.clone();
            let writer: RecallWriter = Arc::new(move |row: SignalRow| {
                let store = store_for_writer.clone();
                Box::pin(async move {
                    let hit = RecallHit {
                        note_path: row.note_path.clone(),
                        score: f64::from(row.score),
                    };
                    store
                        .record_signals(
                            &row.query_text,
                            &row.channel,
                            &[hit],
                            row.session_id.as_deref(),
                            &row.agent_id,
                            &row.namespace,
                        )
                        .map(|_| ())
                })
            });

            let reflector = Arc::new(MemoryReflector::new(reflector_assembler, prov, writer));
            tool_registry.set_memory_reflector(reflector);
            // Startup-time guard: the `memory_reflect` tool silently returns
            // an error string if its reflector is missing. Surfacing the
            // wiring decision here (instead of only at first call) keeps a
            // misconfigured deploy from looking like a model issue later.
            if !daemon {
                println!("  memory_reflect tool: MemoryReflector wired");
            }
        } else if !daemon {
            eprintln!(
                "  memory_reflect tool: MemoryReflector NOT wired (no embedder or default provider). \
                 Calls will return an error; configure an embedder + provider to enable it."
            );
        }

        // Spec 8 Task 8: construct DefaultQueryFiler and inject into memory_reflect tool.
        // Requires a default provider (for the LLM novelty gate).
        // On failure we log and continue — filing is fire-and-forget.
        if app_config.memory.query_filer.enabled {
            if let Some(ref prov) = default_prov {
                use alephcore::memory::notes::query_filer::{DefaultQueryFiler, QueryFiler};
                use alephcore::memory::notes::NoteIndexer;

                if let Some(ref note_dir) = note_dir {
                    let indexer_arc =
                        std::sync::Arc::new(NoteIndexer::new(note_dir.clone(), memory_db.clone()));
                    let query_filer: std::sync::Arc<dyn QueryFiler> =
                        std::sync::Arc::new(DefaultQueryFiler {
                            store: memory_db.clone(),
                            indexer: indexer_arc,
                            provider: prov.clone(),
                            orientation: orientation.clone(),
                            memory_dir: note_dir.clone(),
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

        // Cluster Phase 0c Task 5 — inject the gateway's NodeRegistry so the
        // `node_invoke` tool can address connected nodes. The registry is the
        // same Arc the auth layer threads into AuthContext (built once in the
        // gateway server). Always available; unconditional unlike the MCP
        // wiring which depends on a memory backend.
        tool_registry.set_node_registry(server.node_registry.clone());
        // …and the device records the registry does NOT hold: enrolled-but-
        // offline nodes, and the `revoked_at` flag that makes a deregister
        // stick. Without it `node_manage` degrades to a clear "not injected".
        tool_registry.set_node_security_store(security_store.clone());
        // …and the bus, so `project_manage`'s writes announce themselves on the
        // same `projects.changed` topic the `projects.*` RPC handlers publish
        // to. Without it a room renamed in words still changes on disk, but no
        // open sidebar or room page learns of it until a reload — the "one
        // verb, two faces, only one publishes" asymmetry.
        tool_registry.set_gateway_event_bus(event_bus.clone());

        // Capture default provider before provider_registry is moved into engine
        default_prov = Some(provider_registry.default_provider());

        // Answer MCP `sampling/createMessage` with this provider. The MCP
        // manager already declared the capability at boot (its callback resolves
        // lazily precisely because it spawns before this point); registering
        // here is what makes that declaration true.
        if let Some(ref prov) = default_prov {
            alephcore::mcp::register_sampling_llm(prov.clone());
        } else {
            // The MCP manager already declared the sampling capability at boot;
            // without this the declaration is simply never made true and every
            // `sampling/createMessage` is refused with nothing saying why.
            alephcore::mcp::decline_sampling_llm(
                "the provider registry produced no default provider, so there is \
                 no model to answer MCP `sampling/createMessage` with. Set \
                 `[general] default_provider` to a configured provider.",
            );
        }

        // Clone provider registry for topic generation before it's moved into engine
        let topic_provider_registry = provider_registry.clone();

        // Install `[execution] max_concurrent_subagents` into the sub-agent
        // fan-out enforcement point. Clamped inside the setter, so a typo
        // cannot produce a semaphore of width 0. Deliberately NOT an
        // `ExecutionEngineConfig` field: `SubagentTool::new` has several
        // construction sites and reads this process-global directly, which is
        // exactly why a builder-only knob would have been configurable on one
        // code path only. The live-reload leg is wired in
        // `config::live_apply`'s "execution" arm.
        alephcore::agents::subagent_tool::set_max_concurrent_subagents(
            app_config.execution.max_concurrent_subagents,
        );

        let engine_config = ExecutionEngineConfig {
            default_timeout_secs: app_config.execution.default_timeout_secs,
            scratchpad_progress_push: app_config.execution.progress_push,
            mid_turn_steering: app_config.execution.mid_turn_steering,
            // Zero would mean "reject every steering injection"; fall back to
            // the default rather than let a typo disable mid-loop steering
            // wholesale (P7, same posture as the concurrency caps below).
            max_pending_steering: if app_config.execution.max_pending_steering == 0 {
                ExecutionEngineConfig::default().max_pending_steering
            } else {
                app_config.execution.max_pending_steering
            },
            core_tools: app_config.tools.core.clone(),
            truncate_tool_descriptions: app_config.tools.truncate_tool_descriptions,
            defer_mcp_tools: app_config.tools.defer_mcp_tools,
            max_runs_global: app_config.execution.max_runs_global.max(1),
            max_runs_per_agent: app_config.execution.max_runs_per_agent.max(1),
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
                                    "Warning: Failed to open state.db: {e}. Replay trace persistence disabled."
                                );
                            }
                            None
                        }
                    }
                }
                Err(e) => {
                    if !daemon {
                        eprintln!(
                            "Warning: Failed to resolve data directory: {e}. Replay trace persistence disabled."
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
        .with_app_config(app_config_arc.clone())
        .with_tool_health(tool_health.clone())
        // Share the one parser cell with `command.execute`, `chat.send` and
        // `agent.run`, so all four slash surfaces resolve `/foo` identically.
        // Filled by `init_tool_catalog` further down, before readiness.
        .with_command_parser_cell(command_parser_cell.clone());
        if let Some(ref state_db) = resilience_db {
            engine = engine.with_state_database(state_db.clone());
        }
        // R5 progress push: share the deferred channel-registry cell so the
        // progress sink can reach user channels once boot populates it.
        if let Some(ref cell) = channel_reg_cell {
            engine = engine.with_channel_registry_cell(cell.clone());
        }
        state_db_out = resilience_db.clone();
        if let Some(ref wm) = workspace_manager {
            engine = engine.with_workspace_manager(wm.clone());
        }
        engine = engine.with_planner_provider(naked_loop_planner_provider);
        // Fix 4: thread the capture-filter registry so spawned subagents fire
        // `on_delegation` memory-extension hooks on completion (and their memory
        // writes go through the capture filter). Same registry the compactor +
        // session_complete tool already use; unconditional so it does not depend
        // on the compactor being configured.
        engine = engine.with_capture_registry(memory_ext_registry.clone());

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
                        dedup_enabled: cfg.dedup_enabled,
                        dedup_similarity_threshold: cfg.dedup_similarity_threshold,
                        dedup_noop_threshold: cfg.dedup_noop_threshold,
                    };
                    if let Some(ref note_dir) = note_dir {
                        let indexer = std::sync::Arc::new(NoteIndexer::new(
                            note_dir.clone(),
                            memory_db.clone(),
                        ));
                        Some(std::sync::Arc::new(DefaultCompoundIngestor {
                            store: memory_db.clone(),
                            indexer,
                            provider: prov.clone(),
                            embedder: emb.clone(),
                            orientation: orientation.clone(),
                            memory_dir: note_dir.clone(),
                            budget,
                            // Same ceiling the boot sweep uses. The ingest-time
                            // caller is what covers a *live* process whose
                            // `remove_dir_all` failed — boot only catches what a
                            // dead one left behind, and this daemon is resident.
                            tx_residue_gc_seconds: cfg.tx_residue_gc_seconds,
                            // B5.2 closed: the manager is hoisted above and
                            // lives as long as the ingestor, so the ingest
                            // tail pushes each touched note into the pending
                            // queue and flushes — fresh notes are visible to
                            // vector search without a manual reembed.
                            embedding_manager: embedding_manager_out.clone(),
                            // Admission governance gate (Phase C2). Wired to
                            // config but OFF by default (`governance_enabled =
                            // false`), so existing deployments keep byte-
                            // identical immediate-admit ingest. Flipping the
                            // flag routes low-confidence / high-severity-under-
                            // confidence / contradicting Creates through the
                            // gate → `notes_review_queue` → `NoteReviewStage`
                            // (LLM re-adjudication) next dream cycle.
                            gate: if cfg.governance_enabled {
                                let gate =
                                    alephcore::memory::notes::governance::gate::DefaultNoteWriteGate::new(
                                        memory_db.clone(),
                                        alephcore::memory::notes::governance::gate::GateThresholds::default(),
                                    );
                                Some(std::sync::Arc::new(gate)
                                    as std::sync::Arc<
                                        dyn alephcore::memory::notes::governance::gate::NoteWriteGate,
                                    >)
                            } else {
                                None
                            },
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
                    compound_ingestor,
                    profile_synth.clone(),
                    Some(memory_ext_registry.clone()),
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
        //
        // Dreaming runs unattended every night and its LLM stages are all small
        // classification / summarization calls, so route them to the cheap tier
        // ([memory.dreaming] model, else the preset's aux model). Falls back to
        // the main provider when no cheap tier resolves.
        let dream_prov = alephcore::orchestrator::build_dream_provider(
            app_config,
            &app_config
                .general
                .default_provider
                .clone()
                .unwrap_or_default(),
        )
        .or_else(|| default_prov.clone());

        alephcore::memory::ensure_dream_daemon_with_orientation(
            memory_db.clone(),
            std::sync::Arc::new(app_config.memory.clone()),
            dream_prov,
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
        // Wired unconditionally: without an embedder the assembler degrades to
        // FTS-only retrieval, so notes auto-recall, orientation, and profile
        // injection all survive a deployment with no embedding provider.
        {
            let mcp = super::init_memory_context_provider_with_extensions(
                memory_db,
                embedder_out.clone(),
                default_prov.clone(),
                app_config.memory.assembler_config(),
                // `memory.injection_mode` — Tools deployments must not have
                // orientation/profile/memory auto-injected into prompts.
                app_config.memory.injection_mode,
                // `[memory.curated]` — this is the provider that renders the
                // curated envelope, so this is the call that makes the section
                // mean anything at all.
                app_config.memory.curated.into(),
                // `[memory.orientation] max_tokens` — threaded like curated so
                // the knob is real at the provider that renders orientation.
                app_config.memory.orientation.max_tokens,
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

            // Real-time Memory Pillar 2 — register the CompressionService for the
            // on-session-end flush. The session-end path reads this cell and
            // spawns an immediate compress→link flush, guarded in a FlushRegistry
            // so a back-to-back follow-on session can await consolidated memory.
            if let Some(ref cs) = compression_out {
                alephcore::thinker::memory_context_provider::register_session_end_compression(
                    cs.clone(),
                );
            } else {
                alephcore::thinker::memory_context_provider::decline_session_end_compression(
                    "no CompressionService was built (it needs a provider and an \
                     embedder, and `[memory.compression]` must be on), so pending \
                     raw memories are not flushed into linked notes at session \
                     end. The periodic path still consolidates them; only the \
                     immediacy is lost.",
                );
            }

            // Spec B Task 9 — register SessionEndSummarizer for on-session-end
            // summary production. Requires an AiProvider for the synthesizer
            // fallback path; skip silently if none is configured.
            if let Some(ref prov) = default_prov {
                use alephcore::memory::assembler::hybrid::AiProviderSummaryLlm;
                use alephcore::memory::session_search_summary::{
                    end_hook::SessionEndSummarizer, synthesizer::SummarySynthesizer,
                };
                // Prose wrapper, NOT `AiProviderReranker` — both consumers
                // below (the /end-summary synthesizer and SessionReflector)
                // ask the model for plain text in a shape their own prompt
                // defines; the reranker pins a "strict JSON, no prose" system
                // message that would break both.
                let summary_llm: std::sync::Arc<
                    dyn alephcore::memory::session_search_summary::synthesizer::SummaryLlm,
                > = AiProviderSummaryLlm::new(prov.clone());
                let synthesizer = std::sync::Arc::new(SummarySynthesizer::new(
                    memory_db.clone()
                        as std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
                    session_store.clone(),
                    summary_llm.clone(),
                ));
                let summarizer = std::sync::Arc::new(SessionEndSummarizer {
                    store: memory_db.clone()
                        as std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
                    synthesizer,
                });
                alephcore::thinker::memory_context_provider::register_session_end_summarizer(
                    summarizer,
                );

                // Batch 2 — session-end reflection ("lessons learned"). Opt-in: only
                // registered when [memory.reflection] enabled = true, so the
                // disabled default adds zero session-end overhead. Reuses the
                // same prose SummaryLlm wrapper as the Spec B summarizer. The
                // cooldown watermark persists to compression_metadata so a
                // daemon restart cannot reset the per-agent throttle.
                if app_config.memory.reflection.enabled {
                    use alephcore::memory::session_reflection::SessionReflector;
                    let reflector = std::sync::Arc::new(
                        SessionReflector::new(
                            memory_db.clone()
                                as std::sync::Arc<
                                    dyn alephcore::memory::store::raw_memory::RawMemoryStore,
                                >,
                            session_store.clone(),
                            summary_llm.clone(),
                            app_config.memory.reflection.clone(),
                        )
                        .with_cooldown_store(memory_db.clone()),
                    );
                    alephcore::thinker::memory_context_provider::register_session_reflector(
                        reflector,
                    );
                    // Open-loop injection: surface last session's unresolved
                    // follow-ups in the next session's curated context (R5).
                    alephcore::thinker::memory_context_provider::set_open_loop_inject(
                        app_config.memory.reflection.open_loop_inject_prompt,
                    );
                } else {
                    // Note the asymmetry: `set_open_loop_inject(false)` a few
                    // lines up is an INSTALL of `false`. This arm never reaches
                    // the setter at all, which is a different fact.
                    const NO_REFLECTION: &str =
                        "`[memory.reflection] enabled = false`: no session-end \
                         lesson distillation runs, and last session's open loops \
                         are never injected into the next session's context.";
                    alephcore::thinker::memory_context_provider::decline_session_reflector(
                        NO_REFLECTION,
                    );
                    alephcore::thinker::memory_context_provider::decline_open_loop_inject(
                        NO_REFLECTION,
                    );
                }
            } else {
                // One missing input, three handles. `register_session_end_mcp`
                // above is NOT among them — it ran unconditionally in this
                // block.
                const NO_PROVIDER: &str =
                    "the provider registry produced no default provider, so no \
                     summary LLM could be built: session-end summaries, lesson \
                     reflection and open-loop injection are all off. Set \
                     `[general] default_provider` to a configured provider.";
                alephcore::thinker::memory_context_provider::decline_session_end_summarizer(
                    NO_PROVIDER,
                );
                alephcore::thinker::memory_context_provider::decline_session_reflector(NO_PROVIDER);
                alephcore::thinker::memory_context_provider::decline_open_loop_inject(NO_PROVIDER);
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

            // Which transcription backend is configured is answered in exactly
            // one place (`media::resolve`), shared with the builtin tool
            // registry's MediaPipeline — otherwise `audio_transcribe` and
            // attachment transcription can disagree about whether transcription
            // exists at all, which is the state this replaced.
            let resolved_transcription =
                alephcore::media::transcription_service(&app_config.generation, &|name: &str| {
                    shared_token_mgr
                        .get_secret(&format!("gen:{name}"))
                        .ok()
                        .flatten()
                        .map(|secret| secret.expose().to_string())
                });
            let transcription: Option<Box<dyn TranscriptionService>> = match resolved_transcription
            {
                Ok(Some(resolved)) => {
                    if !daemon {
                        println!("  MediaProcessor: {}", resolved.label);
                    }
                    Some(resolved.service)
                }
                Ok(None) => None,
                // A refused provider is not an absent one. Say which entry and
                // which setting, then start without transcription: this call
                // used to panic, which turned one bad config line into a daemon
                // that would not boot at all.
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "transcription provider refused; starting without transcription"
                    );
                    if !daemon {
                        eprintln!("  MediaProcessor: DISABLED - {e}");
                    }
                    None
                }
            };

            // Build the VisionPipeline so the image-attachment fallback in
            // MediaProcessor::describe_image_fallback actually fires for
            // text-only models (previously hard-coded None meant attached
            // images degraded to "[Image: … — not viewable]"). The bare
            // PlatformOcrProvider (no platform injected) is enough to light
            // up the wire; the desktop-screenshot path uses the same
            // provider with an injected platform in the builtin_registry
            // constructor and is unaffected.
            let vision: Option<Arc<alephcore::vision::VisionPipeline>> = {
                let mut pipeline = alephcore::vision::VisionPipeline::new();
                pipeline.add_provider(Box::new(
                    alephcore::vision::providers::PlatformOcrProvider::new(),
                ));
                Some(Arc::new(pipeline))
            };

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
        // Capture continuation-deps cell before Arc erasure (same pattern as
        // orchestrator_cell). Populated below after engine_arc is available.
        let continuation_cell = engine.continuation_cell();
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
                        // Leave the user a durable receipt in each orphaned
                        // task's session so a mid-run restart no longer ends
                        // their conversation silently.
                        let notified = alephcore::gateway::orphan_notice::notify_interrupted_tasks(
                            session_store.as_ref(),
                            &orphans,
                        )
                        .await;
                        tracing::info!(
                            count = orphans.len(),
                            notified,
                            "Reconciled orphaned agent tasks"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "Failed to reconcile orphaned agent tasks");
                }
            }
        }

        if app_config.agents.list.is_empty() {
            // Legacy path: use FullGatewayConfig agents
            for agent_config in full_config.get_agent_instance_configs() {
                let agent_id = agent_config.agent_id.clone();
                agent_registry
                    .register_config(agent_config, session_store.clone())
                    .await;
                if !daemon {
                    println!("  Registered agent: {agent_id} (lazy)");
                }
            }
        } else {
            // New path: use ResolvedAgents from AgentDefinitionResolver
            let mut resolver = alephcore::AgentDefinitionResolver::new();
            let resolved_agents = resolver.resolve_all(
                &app_config.agents,
                &app_config.profiles,
                &app_config.providers,
            );
            for agent in &resolved_agents {
                let config = alephcore::gateway::AgentInstanceConfig::from_resolved(agent);
                let agent_id = config.agent_id.clone();
                let agent_workspace = config.workspace.clone();
                let agent_model = config.model.clone();
                agent_registry
                    .register_config(config, session_store.clone())
                    .await;
                // Emit lifecycle event. Wrap in TopicEvent so the WS forwarder's
                // topic filter delivers it — a bare publish_json is topic-less and
                // dropped by concrete subscriptions (same fix as switch/delete).
                let lifecycle_event =
                    alephcore::gateway::agent_lifecycle::AgentLifecycleEvent::Registered {
                        agent_id: agent_id.clone(),
                        workspace: agent_workspace,
                        model: agent_model,
                    };
                let topic_event = alephcore::gateway::event_bus::TopicEvent::new(
                    lifecycle_event.topic(),
                    serde_json::to_value(&lifecycle_event).unwrap_or_default(),
                );
                if let Err(e) = event_bus.publish_json(&topic_event) {
                    tracing::warn!("Failed to publish agent lifecycle event: {}", e);
                }
                if !daemon {
                    println!("  Registered agent: {agent_id} (lazy)");
                }
            }
        }

        if !daemon {
            let provider_name = available_provider_from_env().unwrap_or("config");
            println!("  Mode: Real AgentLoop ({provider_name} provider)");
            println!();
        }

        let engine_clone = engine.clone();
        let event_bus_clone = event_bus.clone();
        let router_clone = router.clone();
        let agent_registry_clone = agent_registry.clone();
        let app_config_run = app_config_arc.clone();
        let wm_run = workspace_manager.clone();
        let sm_run = session_store.clone();
        server.handlers_mut().register("agent.run", move |req| {
            let engine = engine_clone.clone();
            let event_bus = event_bus_clone.clone();
            let router = router_clone.clone();
            let agent_registry = agent_registry_clone.clone();
            let cfg = app_config_run.clone();
            let wm = wm_run.clone();
            let sm = sm_run.clone();
            async move {
                handle_run_with_engine(req, engine, event_bus, router, agent_registry, cfg, wm, sm)
                    .await
            }
        });

        // chat.send also uses real ExecutionEngine
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
            async move {
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
                )
                .await
            }
        });

        // trace.list / trace.get / trace.by_runs — replay durable traces when a
        // state DB exists, else SERVICE_UNAVAILABLE. See `common_handlers.rs`.
        register_trace_handlers(server, resilience_db.clone(), session_store.clone());

        // Capture for inbound router
        let engine_arc: Arc<dyn alephcore::gateway::ExecutionAdapter> = engine;
        exec_adapter = Some(engine_arc.clone());
        agent_reg = Some(agent_registry.clone());
        tool_reg_out = Some(tool_registry_for_heartbeat);

        // Wire goal-pursuit autonomous-continuation deps (opt-in, only fires
        // for sessions whose goal has PursuitMode::Active). Idempotent: a
        // second set is ignored by the OnceLock. Registry clone is cheap (Arc).
        // Goal-loop objective gate: reuses the global config.toml [[stop_hooks]]
        // (same source as the within-turn StopHookVerifier). None → no gate,
        // the complete claim terminates immediately (behavior identical to
        // before this feature was introduced).
        let goal_gate =
            alephcore::verification::stop_hooks::build_from_config(&app_config.stop_hooks);
        let goal_deps = alephcore::gateway::execution_engine::ContinuationDeps {
            registry: agent_registry.clone(),
            adapter: engine_arc.clone(),
            gate: goal_gate,
            // Broadcast autonomous continuations live + fan the final reply to
            // the session's origin channel (G1). Same bus the Panel/channels read.
            event_bus: Some(event_bus.clone()),
        };
        let _ = continuation_cell.set(goal_deps.clone());

        // Goal wait-barrier wakes: task-settle event subscription (parked-on-
        // task goals) + boot sweep (re-arm timers / recheck tasks that settled
        // while the daemon was down). The bus holds the subscription for the
        // process lifetime.
        {
            let goal_wake = std::sync::Arc::new(
                alephcore::gateway::execution_engine::goal_wait::GoalWakeService::new(
                    goal_deps,
                    coord_store.clone(),
                    Some(session_store.clone()),
                ),
            );
            goal_wake.subscribe();
            // Periodic task-barrier recheck backstops the event subscription
            // (settle-before-park race, deleted task, derived-Unsatisfiable
            // which emits no event). 60s bounded latency.
            goal_wake.spawn_periodic_recheck(60);
            let goal_wake_boot = goal_wake.clone();
            tokio::spawn(async move {
                // Spawned before `initialize_inbound_router` publishes the
                // channel-config snapshot; park until it is ready so `wake_identity`
                // honors the per-channel deny layer rather than the fail-open miss
                // default.
                alephcore::gateway::channel_policy::wait_for_channel_config_snapshot(
                    std::time::Duration::from_secs(30),
                )
                .await;
                goal_wake_boot.rearm_parked_goals().await;
            });
        }

        // Create run_manager with real execution dependencies.
        // Inject the live config so Panel runs honor the global output_mode
        // toggle (typewriter/instant) fresh per run — same source the chat
        // channels read.
        let real_run_manager = Arc::new(
            AgentRunManager::new(
                router.clone(),
                event_bus.clone(),
                agent_registry.clone(),
                engine_arc.clone(),
            )
            .with_app_config(app_config_arc.clone()),
        );
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
                // Map the optional [team_dispatcher] TOML onto the runtime config.
                // Each field falls back to the live DispatcherConfig::default()
                // (no default duplication / drift). zombie_ttl is clamped to never
                // drop below task_timeout_secs, per the field's own safety note.
                let dispatcher_cfg = {
                    let d = DispatcherConfig::default();
                    match &app_config.team_dispatcher {
                        None => d,
                        Some(t) => {
                            let task_timeout_secs =
                                t.task_timeout_secs.unwrap_or(d.task_timeout_secs);
                            DispatcherConfig {
                                max_concurrent: t.max_concurrent.unwrap_or(d.max_concurrent),
                                lock_ttl_secs: t.lock_ttl_secs.unwrap_or(d.lock_ttl_secs),
                                task_timeout_secs,
                                fallback_tick_secs: t
                                    .fallback_tick_secs
                                    .unwrap_or(d.fallback_tick_secs),
                                zombie_ttl_secs: t
                                    .zombie_ttl_secs
                                    .unwrap_or(d.zombie_ttl_secs)
                                    .max(task_timeout_secs),
                                max_per_owner: t.max_per_owner.unwrap_or(d.max_per_owner),
                                default_max_retries: t
                                    .default_max_retries
                                    .unwrap_or(d.default_max_retries),
                                retry_backoff_base_secs: t
                                    .retry_backoff_base_secs
                                    .unwrap_or(d.retry_backoff_base_secs),
                                retry_backoff_cap_secs: t
                                    .retry_backoff_cap_secs
                                    .unwrap_or(d.retry_backoff_cap_secs),
                            }
                        }
                    }
                };
                let mut dispatcher = TeamDispatcher::new(
                    cs,
                    ts,
                    artifact_store.clone(),
                    inbox_provider,
                    (*gateway_ctx).clone(),
                    dispatcher_cfg,
                    dispatch_signal.clone(),
                );
                // Wire the channel-registry cell so workflow `clarify` steps can
                // push their question to the user's originating channel once the
                // registry is injected (deferred, like the tool registry's).
                if let Some(cell) = channel_reg_cell.clone() {
                    dispatcher = dispatcher.with_clarify_delivery(cell);
                }
                let dispatcher = Arc::new(dispatcher);
                // Event-driven wake: any coord-task transition on the
                // GlobalBus (review verdicts, admin controls, panel edits,
                // clarify answers) pokes the loop immediately instead of
                // waiting for the fallback tick. Subscribe BEFORE the loop
                // starts so no early transition slips between the two
                // (spawn_loop consumes the Arc).
                dispatcher.subscribe_task_events();
                dispatcher.spawn_loop();

                // Wake-on-transition seam: every coord-task mutation flows
                // through `CoordTaskStore::emit_task_topic` → GlobalBus, so one
                // subscription here wakes the dispatcher on ANY task
                // transition — review approvals, manual retries/skips/resumes
                // (2 tools + 4 teams.* RPCs), snapshot restores — none of
                // which thread the direct `dispatch_signal`. Without this,
                // each such transition waited for the fallback tick (up to
                // `fallback_tick_secs`, 60s) before dependents were claimed;
                // a review-gated DAG crawled one approval-plus-a-minute at a
                // time. Self-wakes from the dispatcher's own writes are
                // harmless: `Notify` stores a single permit and a no-op tick
                // is cheap. The direct signal wires (task_create / workflow
                // materialize / clarify resolve) stay as the primary,
                // bus-independent path.
                let wake = dispatch_signal.clone();
                tokio::spawn(async move {
                    use alephcore::event::{EventFilter, EventType, GlobalBus};
                    let _sub_id = GlobalBus::global()
                        .subscribe_async(
                            EventFilter::new(vec![
                                EventType::TeamTaskAssigned,
                                EventType::TeamTaskUpdated,
                                EventType::TeamTaskCompleted,
                                EventType::TeamTaskFailed,
                            ]),
                            move |_event| {
                                wake.notify_one();
                            },
                        )
                        .await;
                    tracing::info!("Team dispatcher wake-on-task-event subscription registered");
                });

                if !daemon {
                    println!("  Team dispatcher started");
                }
            }

            // Collaborative-session deadlock sweeper — `Deadlocked` was a
            // declared SessionStatus with no writer: a session whose
            // transcript filled `max_rounds` (or that went silent) stayed
            // Active forever. Sweep every 5 minutes; grace 10 min after the
            // final round, 24 h of silence for the stale path. The sweep
            // notifies participants to conclude or cancel (see
            // SessionCoordinator::sweep_deadlocked).
            if let Some(sweep_coord) = session_coordinator.clone() {
                tokio::spawn(async move {
                    let mut tick = tokio::time::interval(std::time::Duration::from_mins(5));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    loop {
                        tick.tick().await;
                        match sweep_coord.sweep_deadlocked(600, 86_400).await {
                            Ok(0) => {}
                            Ok(n) => {
                                tracing::info!(
                                    count = n,
                                    "session sweeper: marked deadlocked sessions"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "session sweeper: sweep failed");
                            }
                        }
                    }
                });
            }

            // teams.chat.send runs the team LEADER agent (orchestration), so unlike the
            // store-only teams.* handlers (register_teams_handlers) it needs the execution
            // context. Registered here where GatewayContext + team_store both exist, next
            // to the TeamDispatcher that shares the same dep.
            if let Some(ts) = team_store.clone() {
                use alephcore::teams::broadcast::BroadcastConfig;
                let chat_ctx = gateway_ctx.clone();
                let chat_msg_store = message_store.clone();
                // Author-stamping (multi-user teamchat P3): resolves the
                // human-readable display name for the `.message` event's
                // `author_display_name` field. Same injection shape as the
                // other cloned handles below.
                let chat_security_store = security_store.clone();
                // Resolve a cheap provider (haiku → default) for first-message
                // team auto-naming, mirroring single chat's auto-topic provider.
                let chat_topic_provider: Option<Arc<dyn alephcore::providers::AiProvider>> =
                    topic_provider_registry
                        .get("haiku")
                        .or_else(|| Some(topic_provider_registry.default_provider()));
                let chat_team_planner = team_planner_provider.clone();
                let chat_event_bus = event_bus.clone();
                let chat_coord_store = coord_store.clone();
                // teams.chat.cancel resolves its run_id to a team and gates it
                // through the same ScopedTeamStore teams.chat.send uses, so it
                // needs its own handle taken before `ts` moves into the send
                // closure below.
                let chat_cancel_store = ts.clone();
                // Map the optional [team_broadcast] TOML onto the runtime config.
                // Each field falls back to the live BroadcastConfig::default() (no
                // default duplication / drift). A `0` for any guard would make a
                // "born-dead" group chat (blocked at depth 0, nobody ever woken),
                // so `0` is treated as "use default" (P7 boundary clamp), mirroring
                // [team_dispatcher]'s zombie-ttl clamp. Parallels §4.4 / §4.5.
                let chat_broadcast_cfg = {
                    let d = BroadcastConfig::default();
                    match &app_config.team_broadcast {
                        None => d,
                        Some(t) => BroadcastConfig {
                            max_chain_depth: t
                                .max_chain_depth
                                .filter(|&v| v > 0)
                                .unwrap_or(d.max_chain_depth),
                            max_fanout_width: t
                                .max_fanout_width
                                .filter(|&v| v > 0)
                                .unwrap_or(d.max_fanout_width),
                            max_total_activations: t
                                .max_total_activations
                                .filter(|&v| v > 0)
                                .unwrap_or(d.max_total_activations),
                            transcript_token_budget: t
                                .transcript_token_budget
                                .filter(|&v| v > 0)
                                .unwrap_or(d.transcript_token_budget),
                            member_run_timeout_secs: t
                                .member_run_timeout_secs
                                .filter(|&v| v > 0)
                                .unwrap_or(d.member_run_timeout_secs),
                        },
                    }
                };
                server
                    .handlers_mut()
                    .register("teams.chat.send", move |req| {
                        let store = ts.clone();
                        let ctx = chat_ctx.clone();
                        let msg_store = chat_msg_store.clone();
                        let provider = chat_topic_provider.clone();
                        let planner = chat_team_planner.clone();
                        let bus = chat_event_bus.clone();
                        let coord = chat_coord_store.clone();
                        let bcast = chat_broadcast_cfg.clone();
                        let sec = chat_security_store.clone();
                        async move {
                            alephcore::gateway::handlers::teams::handle_chat_send(
                                req,
                                store,
                                msg_store,
                                ctx,
                                provider,
                                planner,
                                Some(bus),
                                coord,
                                bcast,
                                sec,
                            )
                            .await
                        }
                    });

                // teams.chat.cancel — stop an in-flight group-chat fan-out tree
                // by the run_id teams.chat.send returned. Needs the execution
                // context (engine per-run cancel), so it lives here beside
                // teams.chat.send rather than with the store-only handlers.
                let chat_cancel_ctx = gateway_ctx.clone();
                server
                    .handlers_mut()
                    .register("teams.chat.cancel", move |req| {
                        let ctx = chat_cancel_ctx.clone();
                        let store = chat_cancel_store.clone();
                        async move {
                            alephcore::gateway::handlers::teams::handle_chat_cancel(req, ctx, store)
                                .await
                        }
                    });
            }

            if let Err(e) = gateway_context_cell.set(gateway_ctx) {
                tracing::warn!("Failed to set gateway context cell: {}", e);
            }
        }
    } else {
        // Simulated mode: no provider registry, so none of this function's
        // agent-engine branch ran. Eight capability handles are installed in
        // there and every one of them now reads "never reached" — the state an
        // operator cannot tell from a wiring bug. Name the input instead.
        //
        // Scope is deliberate: only the handles installed LEXICALLY in the
        // branch above. `goal/store`, `looping/registry`, `strategy/store` and
        // the loop-graph handles are also absent here, but they are installed
        // by `BuiltinToolRegistry::with_config` in `src/executor/`, and a
        // sentence written here would be a second, remote authority on their
        // absence — wrong the first time that constructor is called from
        // anywhere else.
        const NO_ENGINE: &str =
            "this process built no agent engine: no provider registry could be \
             created, so no API key and no `[providers]` entry resolved. Configure \
             a provider (or set ANTHROPIC_API_KEY / OPENAI_API_KEY) and restart.";
        alephcore::mcp::decline_sampling_llm(NO_ENGINE);
        alephcore::memory::decline_dream_daemon(NO_ENGINE);
        alephcore::thinker::memory_context_provider::decline_session_end_mcp(NO_ENGINE);
        alephcore::thinker::memory_context_provider::decline_session_end_compression(NO_ENGINE);
        alephcore::thinker::memory_context_provider::decline_session_end_summarizer(NO_ENGINE);
        alephcore::thinker::memory_context_provider::decline_session_reflector(NO_ENGINE);
        alephcore::thinker::memory_context_provider::decline_open_loop_inject(NO_ENGINE);
        alephcore::search::decline_global_search_handle(NO_ENGINE);
        if !daemon {
            println!(
                "  Mode: Simulated (set ANTHROPIC_API_KEY or OPENAI_API_KEY for real execution)"
            );
            println!();
        }
        use alephcore::gateway::execution_engine::ExecutionEngineConfig;
        use alephcore::gateway::execution_engine::SimpleExecutionEngine;

        let simple_engine = Arc::new(
            SimpleExecutionEngine::new(ExecutionEngineConfig::default())
                .with_event_bus(event_bus.clone()),
        );
        let simple_adapter: Arc<dyn alephcore::gateway::ExecutionAdapter> = simple_engine;
        let fallback_agent_registry = Arc::new(AgentRegistry::new());
        let fallback_run_manager = Arc::new(
            AgentRunManager::new(
                router.clone(),
                event_bus.clone(),
                fallback_agent_registry,
                simple_adapter,
            )
            .with_app_config(app_config_arc.clone()),
        );
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

        // Publish the fallback run manager through the outer `run_manager`
        // Option. The `agent.status`/`agent.cancel`/`chat.abort` registrations
        // below and the `_run_manager` field at the end of this fn are both
        // gated on `Some(run_manager)`; without this assignment simulated mode
        // skips those handlers and panics on the terminal `.expect(...)`.
        run_manager = Some(fallback_run_manager);

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
                        .map_or(0, |t| t.len() as u64)
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

    // Create unified dispatch registry (command discovery + resolution).
    // AI-provider-independent — maps command names to metadata, registers the
    // command/tool RPC handlers, spawns the memory producer scheduler, and
    // threads the memory extension registry into the ExtensionManager. See
    // `tool_catalog_init.rs`. `tool_reg_out` is `None` in simulated mode.
    tool_catalog_out = Some(
        init_tool_catalog(
            server,
            &generation_registry,
            app_config,
            tool_reg_out.clone(),
            &command_parser_cell,
            memory_db,
            &memory_ext_registry,
            daemon,
            tool_health.clone(),
        )
        .await,
    );

    // Register status/cancel/chat/agent.list/gateway.* handlers shared by both
    // execution modes, then signal readiness. `session_store` is moved here.
    // See `common_handlers.rs`.
    register_common_handlers(
        server,
        &run_manager,
        session_store,
        &router,
        &agent_reg,
        full_config,
        daemon,
    );

    Ok(AgentHandlersResult {
        // Both branches (real-execution at line ~1443 and simulated-fallback at
        // line ~1772) assign `Some(...)` to `run_manager`; this `.expect` is
        // a design-by-contract check, not a real error path. We use `.expect`
        // (not `panic!`) so the diagnostic reads as "this invariant was
        // violated" rather than "something exploded", and so the line is
        // greppable by anyone scanning for production panics.
        _run_manager: run_manager.expect(
            "run_manager must be set by either the real-execution branch or the \
             simulated-fallback branch before reaching this point",
        ),
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
        dispatch_signal: Some(dispatch_signal),
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
        message_store: message_store.clone(),
        orchestrator_cell: orch_cell_out,
        memory_context_provider: mcp_for_orchestrator,
        memory_backend: Some(memory_db.clone()),
        memory_ext_registry: memory_ext_registry.clone(),
    })
}
