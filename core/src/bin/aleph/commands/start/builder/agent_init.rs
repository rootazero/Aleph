//! Agent handler registration and provider initialization.
//!
//! Extracted from `start/mod.rs` to keep the orchestrator under 500 lines.
//! Contains the complex agent engine setup: provider registry creation,
//! tool registry, task routing, and handler wiring.

use std::sync::Arc;

use alephcore::gateway::GatewayServer;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::handlers::agent::{
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

use crate::server_init::{handle_run_with_engine, handle_chat_send_with_engine};

/// Try to create a provider registry from app config (Settings UI configured providers).
/// Returns None if no usable provider is found.
pub(in crate::commands::start) fn create_provider_registry_from_config(
    app_config: &alephcore::Config,
) -> Option<Arc<alephcore::thinker::SingleProviderRegistry>> {
    use alephcore::providers::create_provider;
    use alephcore::thinker::SingleProviderRegistry;

    // Determine which provider to use: default_provider or first enabled one
    let provider_name = app_config.general.default_provider.clone()
        .or_else(|| {
            app_config.providers.iter()
                .find(|(_, cfg)| cfg.enabled && cfg.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false))
                .map(|(name, _)| name.clone())
        });

    let provider_name = provider_name?;
    let provider_config = app_config.providers.get(&provider_name)?;

    // Must have an API key
    if provider_config.api_key.as_ref().map(|k| k.is_empty()).unwrap_or(true) {
        return None;
    }

    match create_provider(&provider_name, provider_config.clone()) {
        Ok(provider) => {
            tracing::info!(provider = %provider_name, "Created provider from app config");
            Some(Arc::new(SingleProviderRegistry::new(provider)))
        }
        Err(e) => {
            tracing::warn!(provider = %provider_name, error = %e, "Failed to create provider from config");
            None
        }
    }
}

/// Result from registering agent handlers — includes optional execution support
/// for use by InboundMessageRouter.
pub(in crate::commands::start) struct AgentHandlersResult {
    pub _run_manager: Arc<AgentRunManager>,
    pub execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    pub dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>,
    pub sub_agent_dispatcher: Option<Arc<tokio::sync::RwLock<alephcore::agents::sub_agents::SubAgentDispatcher>>>,
    pub _embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    pub compression_service: Option<std::sync::Arc<alephcore::memory::compression::CompressionService>>,
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
    workspace_manager: Option<Arc<alephcore::gateway::WorkspaceManager>>,
    agent_manager: Arc<alephcore::AgentManager>,
    daemon: bool,
) -> AgentHandlersResult {
    let run_manager = Arc::new(AgentRunManager::new(router.clone(), event_bus.clone()));
    let mut exec_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>> = None;
    let mut agent_reg: Option<Arc<AgentRegistry>> = None;
    let mut default_prov: Option<Arc<dyn alephcore::providers::AiProvider>> = None;
    let dispatch_reg: Option<Arc<alephcore::dispatcher::ToolRegistry>>;
    let mut sub_agent_disp: Option<Arc<tokio::sync::RwLock<alephcore::agents::sub_agents::SubAgentDispatcher>>> = None;
    let mut embedder_out: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>> = None;
    let mut compression_out: Option<std::sync::Arc<alephcore::memory::compression::CompressionService>> = None;

    // Try to create provider: env vars first, then app config
    let provider_registry = if can_create_provider_from_env() {
        create_provider_registry_from_env().ok()
    } else {
        // Try from app config providers
        create_provider_registry_from_config(app_config)
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
        // Without A2A, the dispatcher would be empty and the delegate tool
        // would always return NotFound, so we skip registration entirely.
        let sub_agent_dispatcher = if app_config.a2a.enabled {
            let disp = Arc::new(tokio::sync::RwLock::new(
                alephcore::agents::sub_agents::SubAgentDispatcher::new(),
            ));
            sub_agent_disp = Some(disp.clone());
            Some(disp)
        } else {
            None
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
            sub_agent_dispatcher,
            ..Default::default()
        };
        let tool_registry = BuiltinToolRegistry::with_config(tool_config);
        let tool_registry = Arc::new(tool_registry);

        use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;
        use alephcore::dispatcher::{UnifiedTool, ToolSource};
        let tools: Vec<UnifiedTool> = BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|def| UnifiedTool::new(
                format!("builtin:{}", def.name),
                def.name,
                def.description,
                ToolSource::Builtin,
            ))
            .collect();

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
                        match provider.process(&prompt, None).await {
                            Ok(response) => alephcore::routing::llm_classifier::parse_classify_response(&response),
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

        let mut engine = ExecutionEngine::new(
            ExecutionEngineConfig::default(),
            provider_registry,
            tool_registry,
            tools,
            session_manager.clone(),
            Some(memory_db.clone()),
        );
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

        // Wire memory context provider for LanceDB-backed prompt augmentation
        if let Some(ref emb) = embedder_out {
            let mcp = super::init_memory_context_provider(memory_db, emb.clone());
            engine = engine.with_memory_context_provider(mcp);
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
        let engine_chat = engine.clone();
        let event_bus_chat = event_bus.clone();
        let router_chat = router.clone();
        let agent_registry_chat = agent_registry.clone();
        let app_config_chat = app_config_arc.clone();
        let wm_chat = workspace_manager.clone();
        server.handlers_mut().register("chat.send", move |req| {
            let engine = engine_chat.clone();
            let event_bus = event_bus_chat.clone();
            let router = router_chat.clone();
            let agent_registry = agent_registry_chat.clone();
            let cfg = app_config_chat.clone();
            let wm = wm_chat.clone();
            async move {
                handle_chat_send_with_engine(req, engine, event_bus, router, agent_registry, cfg, wm).await
            }
        });

        // Capture for inbound router
        exec_adapter = Some(engine as Arc<dyn alephcore::gateway::ExecutionAdapter>);
        agent_reg = Some(agent_registry);
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

        // Register skills from ExtensionManager (if initialized)
        {
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            use alephcore::domain::Entity;
            if let Ok(ext_manager) = get_extension_manager() {
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

    if !daemon {
        println!("Agent control methods:");
        println!("  - agent.run            : Execute agent request with streaming");
        println!("  - agent.status         : Query run status by run_id");
        println!("  - agent.cancel         : Cancel an active run");
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
        _embedder: embedder_out,
        compression_service: compression_out,
    }
}
