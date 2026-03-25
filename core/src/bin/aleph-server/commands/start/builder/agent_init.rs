//! Agent handler registration and provider initialization.
//!
//! Extracted from `start/mod.rs` to keep the orchestrator under 500 lines.
//! Contains the complex agent engine setup: provider registry creation,
//! tool registry, task routing, and handler wiring.

use std::sync::Arc;

use alephcore::gateway::GatewayServer;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::handlers::agent::{
    self as agent_handlers,
    AgentRunManager, handle_run,
    handle_status as handle_agent_status,
    handle_cancel as handle_agent_cancel,
    handle_respond_to_input,
};
use alephcore::gateway::{
    can_create_provider_from_env, create_provider_registry_from_env,
    ExecutionEngine, ExecutionEngineConfig, AgentRegistry,
    GatewayConfig as FullGatewayConfig,
    SessionManager, available_provider_from_env,
};
use alephcore::gateway::handlers::chat as chat_handlers;
use alephcore::executor::BuiltinToolRegistry;
use alephcore::ProviderRegistry;

use alephcore::generation::{GenerationProviderRegistry, providers as gen_providers};
use crate::server_init::{handle_run_with_engine, handle_chat_send_with_engine};
use alephcore::providers::message::UnifiedMessage;
use alephcore::providers::adapter::RequestPayload;

/// Result from registering agent handlers — includes optional execution support
/// for use by InboundMessageRouter.
pub(in crate::commands::start) struct AgentHandlersResult {
    pub _run_manager: Arc<AgentRunManager>,
    pub execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    pub dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>,
    pub sub_agent_dispatcher: Option<Arc<tokio::sync::RwLock<alephcore::agents::sub_agents::SubAgentDispatcher>>>,
    pub embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    pub compression_service: Option<std::sync::Arc<alephcore::memory::compression::CompressionService>>,
    /// Multi-provider registry for routing and hot-switching
    pub multi_registry: Option<Arc<alephcore::MultiProviderRegistry>>,
    /// Deferred injection cell for ChannelRegistry (created after agent handlers)
    pub channel_registry_cell: Option<Arc<tokio::sync::OnceCell<Arc<alephcore::gateway::channel_registry::ChannelRegistry>>>>,
    /// Generation provider registry for TTS voice output
    pub generation_registry: Option<Arc<std::sync::RwLock<alephcore::generation::GenerationProviderRegistry>>>,
}

/// Register agent.run / agent.status / agent.cancel / chat.* handlers.
/// Selects real ExecutionEngine when an API key is available (env or config),
/// otherwise uses the simulated AgentRunManager.
/// Returns execution support components for inbound routing.
#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) async fn register_agent_handlers(
    server: &mut GatewayServer,
    session_manager: Arc<SessionManager>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    router: Arc<AgentRouter>,
    full_config: &FullGatewayConfig,
    app_config: &alephcore::Config,
    app_config_arc: Arc<tokio::sync::RwLock<alephcore::Config>>,
    memory_db: &alephcore::memory::store::MemoryBackend,
    workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
    agent_manager: Arc<alephcore::AgentManager>,
    acp_manager: Option<Arc<alephcore::acp::manager::AcpHarnessManager>>,
    cron_service: Option<alephcore::tasks::cron::SharedCronService>,
    daemon: bool,
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
) -> AgentHandlersResult {
    let run_manager = Arc::new(AgentRunManager::new(router.clone(), event_bus.clone()));
    let mut exec_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>> = None;
    let mut agent_reg: Option<Arc<AgentRegistry>> = None;
    let mut default_prov: Option<Arc<dyn alephcore::providers::AiProvider>> = None;
    let dispatch_reg: Option<Arc<alephcore::dispatcher::ToolRegistry>>;
    let mut sub_agent_disp: Option<Arc<tokio::sync::RwLock<alephcore::agents::sub_agents::SubAgentDispatcher>>> = None;
    let mut embedder_out: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>> = None;
    let mut compression_out: Option<std::sync::Arc<alephcore::memory::compression::CompressionService>> = None;
    let mut multi_reg: Option<Arc<alephcore::MultiProviderRegistry>> = None;
    let mut channel_reg_cell: Option<Arc<tokio::sync::OnceCell<Arc<alephcore::gateway::channel_registry::ChannelRegistry>>>> = None;

    // Create coord task store (SQLite-backed task/team coordination for swarm tools).
    // Created unconditionally so it is available regardless of AI provider availability.
    let coord_store: Option<Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>> = {
        use alephcore::agents::swarm::tasks::store::SqliteCoordTaskStore;
        use alephcore::utils::paths::get_data_dir;

        let db_path = get_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("aleph_data"))
            .join("coord.db");

        match rusqlite::Connection::open(&db_path) {
            Ok(conn) => {
                let store = Arc::new(SqliteCoordTaskStore::new(conn));
                // Run schema migration synchronously-ish via block_in_place
                let store_clone = store.clone();
                match tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(store_clone.migrate())
                }) {
                    Ok(()) => {
                        if !daemon {
                            println!("  Coord task store initialized (SQLite)");
                        }
                        Some(store as Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>)
                    }
                    Err(e) => {
                        if !daemon {
                            eprintln!("Warning: Coord task store migration failed: {}. Task coordination tools disabled.", e);
                        }
                        None
                    }
                }
            }
            Err(e) => {
                if !daemon {
                    eprintln!("Warning: Failed to open coord.db: {}. Task coordination tools disabled.", e);
                }
                None
            }
        }
    };

    // Initialize SwarmCoordinator and attach the coord task store.
    let swarm_coordinator: Option<Arc<alephcore::agents::swarm::SwarmCoordinator>> = {
        use alephcore::agents::swarm::{SwarmCoordinator, SwarmConfig};

        match SwarmCoordinator::with_config(SwarmConfig::default()).await {
            Ok(coordinator) => {
                let coordinator = if let Some(ref store) = coord_store {
                    coordinator.with_task_store(store.clone())
                } else {
                    coordinator
                };
                let coordinator = Arc::new(coordinator);
                coordinator.clone().start().await;
                if !daemon {
                    println!("  Swarm coordinator started");
                }
                Some(coordinator)
            }
            Err(e) => {
                if !daemon {
                    eprintln!("Warning: Failed to initialize swarm coordinator: {}.", e);
                }
                None
            }
        }
    };

    // Extract the AgentMessageBus from the coordinator for tool config wiring.
    let agent_message_bus: Option<Arc<alephcore::agents::swarm::AgentMessageBus>> =
        swarm_coordinator.as_ref().map(|c| c.bus.clone());

    // Build generation provider registry (independent of chat AI provider)
    let generation_registry = {
        let mut registry = GenerationProviderRegistry::new();
        for (name, mut provider_cfg, gen_type) in app_config.generation.merged_providers() {
            if !provider_cfg.enabled { continue; }
            // Resolve API key from vault if not in config
            if provider_cfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) {
                if let Ok(Some(secret)) = shared_token_mgr.get_secret(&format!("gen:{}", name)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
            if provider_cfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) { continue; }
            match gen_providers::create_provider(&name, &provider_cfg, gen_type) {
                Ok(provider) => {
                    if registry.register(name.clone(), provider).is_ok() {
                        tracing::info!(provider = %name, gen_type = ?gen_type, "Registered generation provider");
                    }
                }
                Err(e) => {
                    tracing::warn!(provider = %name, error = %e, "Skip generation provider");
                }
            }
        }
        if !registry.is_empty() && !daemon {
            println!("  Generation providers: {} registered", registry.len());
        }
        Arc::new(std::sync::RwLock::new(registry))
    };

    // Hot-reload: rebuild generation registry when Panel updates providers
    {
        let gen_reg = generation_registry.clone();
        let config_handle = app_config_arc.clone();
        let vault = shared_token_mgr.clone();
        let mut rx = event_bus.subscribe();

        tokio::spawn(async move {
            while let Ok(event_json) = rx.recv().await {
                let is_gen_event = serde_json::from_str::<serde_json::Value>(&event_json)
                    .ok()
                    .and_then(|v| v.get("topic")?.as_str().map(|s| s.to_string()))
                    == Some("config.generation.providers.changed".to_string());
                if !is_gen_event {
                    continue;
                }

                // Snapshot merged providers (drop read guard before creating providers)
                let merged_snapshot = {
                    let cfg = config_handle.read().await;
                    cfg.generation.merged_providers()
                };

                let mut new_registry = GenerationProviderRegistry::new();
                for (name, mut provider_cfg, gen_type) in merged_snapshot {
                    if !provider_cfg.enabled {
                        continue;
                    }
                    // Resolve API key from vault (RPC handlers store keys in vault, not config)
                    if provider_cfg.api_key.is_none() {
                        if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                            provider_cfg.api_key = Some(secret.expose().to_string());
                        }
                    }
                    if provider_cfg
                        .api_key
                        .as_ref()
                        .map(|k| k.is_empty())
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    match gen_providers::create_provider(&name, &provider_cfg, gen_type) {
                        Ok(provider) => {
                            new_registry.register(name.clone(), provider).ok();
                        }
                        Err(e) => {
                            tracing::warn!(
                                provider = %name, error = %e,
                                "Skip generation provider on reload"
                            );
                        }
                    }
                }

                let mut guard = gen_reg.write().unwrap_or_else(|e| e.into_inner());
                *guard = new_registry;
                tracing::info!(
                    "Generation provider registry reloaded ({} providers)",
                    guard.len()
                );
            }
        });
    }

    // Build MultiProviderRegistry: register ALL configured providers
    let provider_registry = {
        use alephcore::providers::create_provider;

        // Determine default provider name
        let default_name = app_config.general.default_provider.clone()
            .or_else(|| {
                app_config.providers.iter()
                    .find(|(_, cfg)| cfg.enabled && cfg.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false))
                    .map(|(name, _)| name.clone())
            });

        // Try env vars first for the initial provider
        let env_provider = if can_create_provider_from_env() {
            create_provider_registry_from_env()
                .ok()
                .map(|reg| {
                    let p = reg.default_provider();
                    let name = available_provider_from_env().unwrap_or("env");
                    (name, p)
                })
        } else {
            None
        };

        // Build multi-provider registry
        if let Some((env_name, env_prov)) = env_provider {
            let registry = Arc::new(alephcore::MultiProviderRegistry::new(env_name.to_string(), env_prov));
            // Also register all config providers
            for (name, provider_cfg) in &app_config.providers {
                if !provider_cfg.enabled || name.as_str() == env_name { continue; }
                if provider_cfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) { continue; }
                if let Ok(p) = create_provider(name, provider_cfg.clone()) {
                    registry.register(name.clone(), p);
                    tracing::info!(provider = %name, "Registered provider from config");
                }
            }
            multi_reg = Some(registry.clone());
            if !daemon { println!("  Providers: {} registered", registry.list_providers().len()); }
            Some(registry as Arc<alephcore::MultiProviderRegistry>)
        } else if let Some(def_name) = default_name {
            // No env provider — create from config
            if let Some(provider_cfg) = app_config.providers.get(&def_name) {
                if let Ok(default_prov) = create_provider(&def_name, provider_cfg.clone()) {
                    let registry = Arc::new(alephcore::MultiProviderRegistry::new(def_name.clone(), default_prov));
                    // Register remaining providers
                    for (name, pcfg) in &app_config.providers {
                        if !pcfg.enabled || name == &def_name { continue; }
                        if pcfg.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) { continue; }
                        if let Ok(p) = create_provider(name, pcfg.clone()) {
                            registry.register(name.clone(), p);
                            tracing::info!(provider = %name, "Registered provider from config");
                        }
                    }
                    multi_reg = Some(registry.clone());
                    if !daemon { println!("  Providers: {} registered (default: {})", registry.list_providers().len(), def_name); }
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
    // and the dispatch_registry block (parser creation) can access it.
    let command_parser_cell: Arc<tokio::sync::RwLock<Option<Arc<alephcore::command::CommandParser>>>> =
        Arc::new(tokio::sync::RwLock::new(None));

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

        // Extract Tavily API key from search config
        let tavily_api_key = app_config
            .search
            .as_ref()
            .and_then(|s| s.backends.get(&s.default_provider))
            .and_then(|b| b.api_key.clone());

        // Create agent registry before tool config so agent management tools can use it
        let agent_registry = Arc::new(AgentRegistry::new());

        // Capture embedder before it's moved into tool_config
        embedder_out = embedder.clone();

        // Create SubAgentDispatcher only when A2A is enabled.
        // Used by A2A subsystem to register A2ASubAgent for remote delegation.
        if app_config.a2a.enabled {
            let disp = Arc::new(tokio::sync::RwLock::new(
                alephcore::agents::sub_agents::SubAgentDispatcher::new(),
            ));
            sub_agent_disp = Some(disp);
        }

        // Get extension manager for plugin tool execution
        let extension_manager = {
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            get_extension_manager().ok().map(Arc::clone)
        };

        // Build tool config with memory backend, embedder, search API key, and agent management deps
        let tool_config = alephcore::executor::BuiltinToolConfig {
            memory_db: Some(memory_db.clone()),
            embedder,
            tavily_api_key,
            agent_registry: Some(agent_registry.clone()),
            workspace_manager: workspace_manager.clone(),
            event_bus: Some(event_bus.clone()),
            tool_policy: Some(alephcore::builtin_tools::agent_manage::new_tool_policy_handle()),
            agent_manager: Some(agent_manager.clone()),
            extension_manager: extension_manager.clone(),
            acp_manager: acp_manager.clone(),
            cron_service: cron_service.clone(),
            generation_registry: Some(generation_registry.clone()),
            tool_context: Some(alephcore::tools::new_tool_context_handle()),
            session_manager: Some(session_manager.clone()),
            shared_token_manager: Some(shared_token_mgr.clone()),
            memory_similarity_threshold: Some(app_config.memory.similarity_threshold),
            coord_task_store: coord_store.clone(),
            agent_message_bus: agent_message_bus.clone(),
            browser_profile_manager: Some(std::sync::Arc::new(
                alephcore::browser::manager::ProfileManager::new(app_config.general.browser.clone()),
            )),
            ..Default::default()
        };
        let mut tool_registry = BuiltinToolRegistry::with_config(tool_config).await;

        use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;
        use alephcore::dispatcher::{UnifiedTool, ToolSource};
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
                    if let Ok(manifest) = alephcore::extension::manifest::parse_manifest_from_dir_sync(&plugin.root_dir) {
                        for t in manifest.tools_v2.unwrap_or_default() {
                            let mut tool = UnifiedTool::new(
                                format!("plugin:{}:{}", plugin.id, t.name),
                                &t.name,
                                t.description.as_deref().unwrap_or(""),
                                ToolSource::Plugin { plugin_id: plugin.id.clone() },
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
            let plugin_tool_count = tools.iter().filter(|t| matches!(t.source, ToolSource::Plugin { .. })).count();
            let builtin_tool_count = tools.iter().filter(|t| matches!(t.source, ToolSource::Builtin)).count();
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
        let gateway_context_cell = tool_registry.gateway_context_cell();
        channel_reg_cell = Some(tool_registry.channel_registry_cell());

        let tool_registry = Arc::new(tool_registry);

        // Build task router from config
        let task_router: Option<Arc<dyn alephcore::routing::TaskRouter>> = if app_config.task_routing.enabled {
            let rules = alephcore::routing::RoutingRules::from_config(&app_config.task_routing.patterns);
            let mut router = alephcore::routing::CompositeRouter::new(
                rules,
                app_config.task_routing.enable_llm_fallback,
                app_config.task_routing.escalation_step_threshold,
            );
            // Wire LLM classify function using the default provider
            if app_config.task_routing.enable_llm_fallback {
                let classify_provider = provider_registry.default_provider();
                let classify_fn: alephcore::routing::composite_router::LlmClassifyFn = Arc::new(move |msg: &str| {
                    let provider = classify_provider.clone();
                    let prompt = alephcore::routing::llm_classifier::build_classify_prompt(msg);
                    Box::pin(async move {
                        let __msgs = [UnifiedMessage::user(&prompt)];
                        match provider.process(RequestPayload::new(&__msgs)).await {
                            Ok(response) => alephcore::routing::llm_classifier::parse_classify_response(&response.text_content()),
                            Err(e) => {
                                tracing::warn!(
                                    subsystem = "task_router",
                                    error = %e,
                                    "LLM classification failed, defaulting to simple"
                                );
                                alephcore::routing::TaskRoute::Simple
                            }
                        }
                    })
                });
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
        let mut engine = ExecutionEngine::new(
            engine_config,
            provider_registry,
            tool_registry,
            tools,
            Some(memory_db.clone()),
        )
        .with_global_tool_permissions(app_config.policies.tool_permissions.clone());
        if let Some(router) = task_router {
            engine = engine.with_task_router(router);
        }
        if let Some(ref wm) = workspace_manager {
            engine = engine.with_workspace_manager(wm.clone());
        }

        // Create compression service for Layer 1 -> Layer 2 fact extraction
        let compression_svc: Option<std::sync::Arc<alephcore::memory::compression::CompressionService>> =
            if let Some(ref emb) = embedder_out {
                default_prov.as_ref().map(|prov| super::init_compression_service(
                        memory_db, prov.clone(), emb.clone(),
                        &app_config.policies.memory.compression, daemon,
                    ))
            } else {
                None
            };

        if let Some(ref cs) = compression_svc {
            engine = engine.with_compression_service(cs.clone());
        }
        compression_out = compression_svc;

        // Create session compactor for intra-session context compression
        {
            let sc_config = app_config.policies.memory.session_compactor.clone();
            if sc_config.enabled {
                let mut compactor = alephcore::memory::session_compactor::SessionCompactor::new(
                    memory_db.clone(),
                    sc_config,
                );
                if let Some(ref prov) = default_prov {
                    compactor = compactor.with_provider(prov.clone());
                }
                let compactor = std::sync::Arc::new(compactor);
                engine = engine.with_session_compactor(compactor);
                if !daemon {
                    println!("  Session compactor enabled (hierarchical summarization)");
                }
            } else if !daemon {
                println!("  Session compactor: disabled");
            }
        }

        // Wire memory context provider for LanceDB-backed prompt augmentation
        if let Some(ref emb) = embedder_out {
            let mcp = super::init_memory_context_provider(memory_db, emb.clone());
            engine = engine.with_memory_context_provider(mcp);
        }

        // Wire session manager + event bus for auto-topic generation on first message
        engine = engine.with_session_topic_support(session_manager.clone(), event_bus.clone());

        // Wire MediaProcessor for multimodal attachment handling (images, audio)
        {
            use alephcore::media::processor::MediaProcessor;
            use alephcore::media::whisper::WhisperTranscription;
            use alephcore::media::transcription::TranscriptionService;

            // Try to get an OpenAI-compatible API key for Whisper transcription.
            // Look for "openai" provider first, then any provider with openai protocol.
            let transcription: Option<Box<dyn TranscriptionService>> = {
                let openai_cfg = app_config.providers.get("openai")
                    .or_else(|| {
                        app_config.providers.values().find(|cfg| {
                            cfg.enabled
                                && cfg.protocol.as_deref() == Some("openai")
                                && cfg.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false)
                        })
                    });

                if let Some(cfg) = openai_cfg {
                    if let Some(ref key) = cfg.api_key {
                        if !key.is_empty() {
                            let whisper = WhisperTranscription::new(
                                key.clone(),
                                cfg.base_url.clone(),
                                None, // use default whisper model
                            );
                            if !daemon {
                                println!("  MediaProcessor: Whisper transcription enabled");
                            }
                            Some(Box::new(whisper))
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

        let engine = Arc::new(engine);

        if !app_config.agents.list.is_empty() {
            // New path: use ResolvedAgents from AgentDefinitionResolver
            let mut resolver = alephcore::AgentDefinitionResolver::new();
            let resolved_agents = resolver.resolve_all(
                &app_config.agents,
                &app_config.profiles,
            );
            for agent in &resolved_agents {
                let config = alephcore::gateway::AgentInstanceConfig::from_resolved(agent);
                let agent_id = config.agent_id.clone();
                let agent_workspace = config.workspace.clone();
                let agent_model = config.model.clone();
                match alephcore::gateway::AgentInstance::with_session_manager(
                    config,
                    session_manager.clone(),
                ) {
                    Ok(instance) => {
                        agent_registry.register(instance).await;
                        // Emit lifecycle event
                        let lifecycle_event = alephcore::gateway::agent_lifecycle::AgentLifecycleEvent::Registered {
                            agent_id: agent_id.clone(),
                            workspace: agent_workspace,
                            model: agent_model,
                        };
                        let _ = event_bus.publish_json(&lifecycle_event);
                        if !daemon {
                            println!("  Registered agent: {} (config-driven)", agent_id);
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to create agent '{}': {}", agent_id, e);
                    }
                }
            }
        } else {
            // Legacy path: use FullGatewayConfig agents
            for agent_config in full_config.get_agent_instance_configs() {
                let agent_id = agent_config.agent_id.clone();
                match alephcore::gateway::AgentInstance::with_session_manager(
                    agent_config,
                    session_manager.clone(),
                ) {
                    Ok(agent) => {
                        agent_registry.register(agent).await;
                        if !daemon {
                            println!("  Registered agent: {} (with SQLite persistence)", agent_id);
                        }
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to create agent '{}': {}", agent_id, e);
                    }
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
                handle_run_with_engine(req, engine, event_bus, router, agent_registry, cfg, wm).await
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
        let sm_chat = session_manager.clone();
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
                handle_chat_send_with_engine(req, engine, event_bus, router, agent_registry, cfg, wm, pr, sm, parser).await
            }
        });

        // Capture for inbound router
        let engine_arc: Arc<dyn alephcore::gateway::ExecutionAdapter> = engine;
        exec_adapter = Some(engine_arc.clone());
        agent_reg = Some(agent_registry.clone());

        // activity.stats — returns active agent run count + coord task count
        {
            let exec_adapter_for_stats: Arc<dyn alephcore::gateway::ExecutionAdapter> = engine_arc.clone();
            let coord_store_for_stats = coord_store.clone();
            server.handlers_mut().register("activity.stats", move |req| {
                let adapter = exec_adapter_for_stats.clone();
                let store = coord_store_for_stats.clone();
                async move {
                    alephcore::gateway::handlers::activity::handle_stats(req, adapter, store).await
                }
            });
        }

        // Deferred injection: now that ExecutionAdapter exists, build GatewayContext
        // and inject it into BuiltinToolRegistry (via shared OnceCell).
        {
            use alephcore::gateway::context::GatewayContext;
            use alephcore::gateway::inter_agent_policy::AgentToAgentPolicy;

            let a2a_policy = Arc::new(AgentToAgentPolicy::default());
            let gateway_ctx = Arc::new(GatewayContext::new(
                session_manager.clone(),
                agent_registry,
                engine_arc,
                a2a_policy,
            ));
            let _ = gateway_context_cell.set(gateway_ctx);
        }
    } else {
        if !daemon {
            println!("  Mode: Simulated (set ANTHROPIC_API_KEY or OPENAI_API_KEY for real execution)");
            println!();
        }
        let run_manager_clone = run_manager.clone();
        server.handlers_mut().register("agent.run", move |req| {
            let manager = run_manager_clone.clone();
            async move { handle_run(req, manager).await }
        });

        let rm_chat = run_manager.clone();
        server.handlers_mut().register("chat.send", move |req| {
            let manager = rm_chat.clone();
            async move { chat_handlers::handle_send(req, manager).await }
        });

        // Fallback: activity.stats with no execution engine
        let coord_store_fallback = coord_store.clone();
        server.handlers_mut().register("activity.stats", move |req| {
            let store = coord_store_fallback.clone();
            async move {
                let active_tasks = if let Some(ref s) = store {
                    s.list_tasks(alephcore::agents::swarm::tasks::CoordTaskFilter {
                        status: Some(alephcore::agents::swarm::tasks::CoordTaskStatus::InProgress),
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
        use alephcore::dispatcher::ToolRegistry as DispatchRegistry;
        use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;

        let dispatch_registry = Arc::new(DispatchRegistry::new());

        // Register builtin tools (generate_image, generate_speech, read_skill, list_skills, snapshot)
        dispatch_registry.register_builtin_tools().await;

        // Also register executor builtin tools as commands (search, screenshot, ocr, etc.)
        for def in BUILTIN_TOOL_DEFINITIONS {
            use alephcore::dispatcher::{UnifiedTool as DUnifiedTool, ToolSource as DToolSource};
            let tool = DUnifiedTool::new(
                format!("builtin:{}", def.name),
                def.name,
                def.description,
                DToolSource::Builtin,
            );
            dispatch_registry.register_with_conflict_resolution(tool).await;
        }

        // Register custom commands from config routing rules
        if !app_config.rules.is_empty() {
            dispatch_registry.register_custom_commands(&app_config.rules).await;
        }

        // Register skills and plugin tools from ExtensionManager (if initialized)
        {
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            use alephcore::domain::Entity;
            if let Ok(ext_manager) = get_extension_manager() {
                // Ensure extensions are discovered and loaded (skills + plugins)
                if let Err(e) = ext_manager.ensure_loaded().await {
                    tracing::warn!("Failed to load extensions: {}", e);
                }

                {
                    let skill_manifests = ext_manager.skill_system().list_skills().await;
                    let skill_infos: Vec<alephcore::skills::SkillInfo> = skill_manifests
                        .iter()
                        .filter(|s| s.is_user_invocable())
                        .map(|s| alephcore::skills::SkillInfo {
                            id: s.id().as_str().to_string(),
                            name: s.name().to_string(),
                            description: s.description().to_string(),
                            triggers: Vec::new(),
                            allowed_tools: Vec::new(),
                            ecosystem: "aleph".to_string(),
                        })
                        .collect();
                    dispatch_registry.register_skills(&skill_infos).await;
                    if !daemon {
                        println!("  Dispatch registry: {} skills registered", skill_infos.len());
                    }
                }

                // Register plugin commands (from CC-format plugins' commands/ directories)
                {
                    let commands = ext_manager.get_all_commands().await;
                    let command_skill_infos: Vec<alephcore::skills::SkillInfo> = commands
                        .iter()
                        .filter(|cmd| cmd.plugin_name.is_some())
                        .map(|cmd| {
                            let id = if let Some(ref plugin) = cmd.plugin_name {
                                format!("{}:{}", plugin, cmd.name)
                            } else {
                                cmd.name.clone()
                            };
                            alephcore::skills::SkillInfo {
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
                        dispatch_registry.register_skills(&command_skill_infos).await;
                        if !daemon {
                            println!("  Dispatch registry: {} plugin commands registered", command_skill_infos.len());
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
                            match alephcore::extension::manifest::parse_manifest_from_dir_sync(&plugin.root_dir) {
                                Ok(manifest) => manifest.tools_v2.unwrap_or_default().into_iter().map(|t| {
                                    (
                                        plugin.id.clone(),
                                        t.name.clone(),
                                        t.description.unwrap_or_default(),
                                    )
                                }).collect::<Vec<_>>(),
                                Err(_) => Vec::new(),
                            }
                        })
                        .collect();

                    if !plugin_tools.is_empty() {
                        dispatch_registry.register_plugin_tools(&plugin_tools).await;
                        if !daemon {
                            println!("  Dispatch registry: {} plugin tools registered", plugin_tools.len());
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
            let reg = dispatch_registry.clone();
            server.handlers_mut().register("commands.list", move |req| {
                let registry = reg.clone();
                async move {
                    alephcore::gateway::handlers::commands::handle_list_from_registry(req, &registry).await
                }
            });
            if !daemon {
                println!("  commands.list: wired to unified dispatch registry");
            }
        }

        // Wire command.execute to resolve slash commands via CommandParser + ToolRegistry
        {
            let parser = Arc::new(alephcore::command::CommandParser::new(dispatch_registry.clone()));

            // Inject parser into chat.send handler (created earlier, uses deferred cell)
            {
                let mut cell = command_parser_cell.write().await;
                *cell = Some(parser.clone());
            }

            let reg = dispatch_registry.clone();
            server.handlers_mut().register("command.execute", move |req| {
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

        dispatch_reg = Some(dispatch_registry);
    }

    // Register status/cancel (work for both real and simulated modes)
    let run_manager_status = run_manager.clone();
    server.handlers_mut().register("agent.status", move |req| {
        let manager = run_manager_status.clone();
        async move { handle_agent_status(req, manager).await }
    });

    let run_manager_cancel = run_manager.clone();
    server.handlers_mut().register("agent.cancel", move |req| {
        let manager = run_manager_cancel.clone();
        async move { handle_agent_cancel(req, manager).await }
    });

    // Register chat handlers (abort, history, clear work for both real and simulated)
    let rm_abort = run_manager.clone();
    server.handlers_mut().register("chat.abort", move |req| {
        let manager = rm_abort.clone();
        async move { chat_handlers::handle_abort(req, manager).await }
    });

    let sm_history = session_manager.clone();
    server.handlers_mut().register("chat.history", move |req| {
        let manager = sm_history.clone();
        async move { chat_handlers::handle_history(req, manager).await }
    });

    let sm_clear = session_manager;
    server.handlers_mut().register("chat.clear", move |req| {
        let manager = sm_clear.clone();
        async move { chat_handlers::handle_clear(req, manager).await }
    });

    // agent.respondToInput is stateless (no context args)
    server.handlers_mut().register("agent.respondToInput", |req| async move {
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

    AgentHandlersResult {
        _run_manager: run_manager,
        execution_adapter: exec_adapter,
        agent_registry: agent_reg,
        default_provider: default_prov,
        dispatch_registry: dispatch_reg,
        sub_agent_dispatcher: sub_agent_disp,
        embedder: embedder_out,
        compression_service: compression_out,
        multi_registry: multi_reg,
        channel_registry_cell: channel_reg_cell,
        generation_registry: Some(generation_registry),
    }
}
