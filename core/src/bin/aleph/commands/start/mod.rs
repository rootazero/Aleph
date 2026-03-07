//! Server startup command handler
//!
//! This module contains the main server initialization and startup logic.

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::cli::Args;
use crate::daemon::{expand_path, remove_pid_file, daemonize};

use std::sync::Arc;

use alephcore::gateway::GatewayServer;
use alephcore::gateway::bridge::DesktopBridgeManager;
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
    SessionManager, SessionManagerConfig,
    ChannelRegistry, InboundMessageRouter, RoutingConfig,
};
use alephcore::gateway::pairing_store::SqlitePairingStore;
use alephcore::gateway::handlers::chat as chat_handlers;
use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::security::{TokenManager, PairingManager};
use alephcore::gateway::device_store::DeviceStore;
#[cfg(target_os = "macos")]
use alephcore::gateway::interfaces::{IMessageChannel, IMessageConfig};
use alephcore::gateway::interfaces::{TelegramChannel, TelegramConfig};
use alephcore::gateway::interfaces::{DiscordChannel, DiscordConfig};
use alephcore::gateway::interfaces::{WhatsAppChannel, WhatsAppConfig};
use alephcore::executor::BuiltinToolRegistry;
use alephcore::cron::CronService;
use alephcore::group_chat::{GroupChatExecutor, GroupChatOrchestrator};
use alephcore::ProviderRegistry;
use alephcore::gateway::handlers::poe::{
    handle_run as handle_poe_run, handle_status as handle_poe_status,
    handle_cancel as handle_poe_cancel, handle_list as handle_poe_list,
    handle_prepare, handle_sign, handle_reject, handle_pending,
};
use alephcore::poe::{
    CompositeValidator, GatewayAgentLoopWorker, ManifestBuilder, PoeConfig,
    create_gateway_worker,
    // Service layer
    PoeRunManager, PoeContractService, WorkerFactory, ValidatorFactory,
};
use alephcore::gateway::{
    create_claude_provider_from_env, available_provider_from_env,
};

use crate::cli::DEFAULT_LOG_FILE;
use crate::server_init::{handle_run_with_engine, handle_chat_send_with_engine};

mod builder;
use builder::*;

// ── Subsystem initializer functions ──────────────────────────────────────────
// Each function handles one cohesive initialization concern, extracted from
// start_server() to keep the orchestrator function under 100 lines.

/// Validate that the bind address is available, or exit if not.
fn validate_bind_address(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;
    if !args.force {
        if let Err(e) = std::net::TcpListener::bind(addr) {
            eprintln!("Error: Cannot bind to {}: {}", addr, e);
            eprintln!("Hint: Use --force to attempt to start anyway, or choose a different port with --port");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Print the startup banner and available method list to stdout.
fn print_startup_banner(addr: SocketAddr, full_config: &FullGatewayConfig) {
    println!("PII filtering engine initialized (enabled: {})", full_config.privacy.pii_filtering);
    println!("╔═══════════════════════════════════════════════╗");
    println!("║         Aleph Gateway v{}           ║", env!("CARGO_PKG_VERSION"));
    println!("╠═══════════════════════════════════════════════╣");
    println!("║  WebSocket: ws://{}          ║", addr);
    println!("║  Protocol:  JSON-RPC 2.0                      ║");
    println!("╚═══════════════════════════════════════════════╝");
    println!();
    println!("Available methods:");
    println!("  - health    : Check server health status");
    println!("  - echo      : Echo back parameters (testing)");
    println!("  - version   : Get server version info");
    println!("  - agent.run : Execute agent request with streaming");
    println!();
    println!("Agents: {:?}", full_config.agents.keys().collect::<Vec<_>>());
    println!();
}

/// Initialize the tracing subscriber with file + console logging.
///
/// Uses `aleph_logging::init_component_logging` which provides:
/// - Console output with PII scrubbing
/// - File output to `~/.aleph/logs/aleph-server.log.YYYY-MM-DD`
/// - Daily rotation and 7-day retention
fn initialize_tracing(args: &Args) {
    let filter = format!("aleph_server={},alephcore::gateway={}", args.log_level, args.log_level);
    if let Err(e) = aleph_logging::init_component_logging("server", 7, &filter) {
        eprintln!("Warning: Failed to initialize file logging: {}. Falling back to console only.", e);
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_file(false)
                .with_line_number(false))
            .with(tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter)))
            .init();
    }
}

/// Load gateway configuration, apply CLI overrides, and return resolved values.
/// Returns (full_config, final_bind, final_port, final_max_connections).
fn load_gateway_config(args: &Args) -> (FullGatewayConfig, String, u16, usize) {
    let full_config = match &args.config {
        Some(config_path) => {
            let path = expand_path(&config_path.to_string_lossy());
            match FullGatewayConfig::load(&path) {
                Ok(cfg) => {
                    if !args.daemon {
                        println!("Loaded config from: {}", path.display());
                    }
                    cfg
                }
                Err(e) => {
                    eprintln!("Error loading config from {}: {}", path.display(), e);
                    std::process::exit(1);
                }
            }
        }
        None => {
            match FullGatewayConfig::load_default() {
                Ok(cfg) => cfg,
                Err(e) => {
                    if !args.daemon {
                        eprintln!("Warning: {}, using defaults", e);
                    }
                    FullGatewayConfig::default()
                }
            }
        }
    };

    // CLI args override config file settings
    let final_bind = if args.bind != "127.0.0.1" {
        args.bind.clone()
    } else {
        full_config.gateway.host.clone()
    };
    let final_port = if args.port != 18790 {
        args.port
    } else {
        full_config.gateway.port
    };
    let final_max_connections = if args.max_connections != 1000 {
        args.max_connections
    } else {
        full_config.gateway.max_connections
    };

    (full_config, final_bind, final_port, final_max_connections)
}

/// Initialize the SessionManager with SQLite persistence, falling back to a
/// temporary path on error.
async fn initialize_session_manager(daemon: bool) -> Arc<SessionManager> {
    match SessionManager::with_defaults() {
        Ok(sm) => {
            if !daemon {
                println!("Session manager initialized (SQLite persistence)");
            }
            Arc::new(sm)
        }
        Err(e) => {
            eprintln!("Warning: Failed to initialize session manager: {}. Using temp storage.", e);
            let temp_path = std::env::temp_dir().join("aleph_sessions.db");
            match SessionManager::new(SessionManagerConfig {
                db_path: temp_path,
                ..Default::default()
            }) {
                Ok(sm) => Arc::new(sm),
                Err(e2) => {
                    eprintln!("Error: Could not create fallback session manager: {}", e2);
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Initialize the ExtensionManager for the plugin system.
async fn initialize_extension_manager(daemon: bool) {
    match alephcore::extension::ExtensionManager::with_defaults().await {
        Ok(extension_manager) => {
            let manager = Arc::new(extension_manager);
            if let Err(_existing) = alephcore::gateway::init_extension_manager(manager) {
                if !daemon {
                    println!("Extension manager already initialized");
                }
            } else if !daemon {
                println!("Extension manager initialized");
            }
        }
        Err(e) => {
            if !daemon {
                eprintln!("Warning: Failed to initialize extension manager: {}. Plugin tools will be unavailable.", e);
            }
        }
    }
}

/// Try to create a provider registry from app config (Settings UI configured providers).
/// Returns None if no usable provider is found.
fn create_provider_registry_from_config(
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
struct AgentHandlersResult {
    _run_manager: Arc<AgentRunManager>,
    execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>,
}

/// Register agent.run / agent.status / agent.cancel / chat.* handlers.
/// Selects real ExecutionEngine when an API key is available (env or config),
/// otherwise uses the simulated AgentRunManager.
/// Returns execution support components for inbound routing.
async fn register_agent_handlers(
    server: &mut GatewayServer,
    session_manager: Arc<SessionManager>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    router: Arc<AgentRouter>,
    full_config: &FullGatewayConfig,
    app_config: &alephcore::Config,
    app_config_arc: Arc<tokio::sync::RwLock<alephcore::Config>>,
    memory_db: &alephcore::memory::store::MemoryBackend,
    workspace_manager: Option<Arc<alephcore::gateway::WorkspaceManager>>,
    daemon: bool,
) -> AgentHandlersResult {
    let run_manager = Arc::new(AgentRunManager::new(router.clone(), event_bus.clone()));
    let mut exec_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>> = None;
    let mut agent_reg: Option<Arc<AgentRegistry>> = None;
    let mut default_prov: Option<Arc<dyn alephcore::providers::AiProvider>> = None;
    let mut dispatch_reg: Option<Arc<alephcore::dispatcher::ToolRegistry>> = None;

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

        // Build tool config with memory backend, embedder, search API key, and agent management deps
        let tool_config = alephcore::executor::BuiltinToolConfig {
            memory_db: Some(memory_db.clone()),
            embedder,
            tavily_api_key,
            agent_registry: Some(agent_registry.clone()),
            workspace_manager: workspace_manager.clone(),
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

        // Create unified dispatch registry (command discovery + resolution)
        use alephcore::dispatcher::ToolRegistry as DispatchRegistry;
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
                if let Some(skill_sys) = ext_manager.skill_system() {
                    let skill_manifests = skill_sys.list_skills().await;
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
        server.handlers_mut().register("agent.run", move |req| {
            let engine = engine_clone.clone();
            let event_bus = event_bus_clone.clone();
            let router = router_clone.clone();
            let agent_registry = agent_registry_clone.clone();
            let cfg = app_config_run.clone();
            async move {
                handle_run_with_engine(req, engine, event_bus, router, agent_registry, cfg).await
            }
        });

        // chat.send also uses real ExecutionEngine
        let engine_chat = engine.clone();
        let event_bus_chat = event_bus.clone();
        let router_chat = router.clone();
        let agent_registry_chat = agent_registry.clone();
        let app_config_chat = app_config_arc.clone();
        server.handlers_mut().register("chat.send", move |req| {
            let engine = engine_chat.clone();
            let event_bus = event_bus_chat.clone();
            let router = router_chat.clone();
            let agent_registry = agent_registry_chat.clone();
            let cfg = app_config_chat.clone();
            async move {
                handle_chat_send_with_engine(req, engine, event_bus, router, agent_registry, cfg).await
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
    }
}

/// Register POE (Principle-Operation-Evaluation) handlers when an Anthropic
/// API key is available. Skips silently (with a note) if the key is absent.
async fn register_poe_handlers(
    server: &mut GatewayServer,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    daemon: bool,
) {
    if let Ok(poe_provider) = create_claude_provider_from_env() {
        let poe_provider_arc: Arc<dyn alephcore::providers::AiProvider> = poe_provider;
        let manifest_builder = Arc::new(ManifestBuilder::new(poe_provider_arc.clone()));

        let provider_for_worker = poe_provider_arc.clone();
        let worker_factory: WorkerFactory<GatewayAgentLoopWorker> = Arc::new(move || {
            create_gateway_worker(
                provider_for_worker.clone(),
                PathBuf::from("/tmp/poe-workspace"),
            )
        });

        let provider_for_validator = poe_provider_arc.clone();
        let validator_factory: ValidatorFactory = Arc::new(move || {
            CompositeValidator::new(provider_for_validator.clone())
        });

        let poe_run_manager = Arc::new(PoeRunManager::new(
            event_bus.clone(),
            worker_factory,
            validator_factory,
            PoeConfig::default(),
        ));

        let poe_contract_service = Arc::new(PoeContractService::new(
            manifest_builder,
            poe_run_manager.clone(),
            event_bus.clone(),
        ));

        let poe_rm_run = poe_run_manager.clone();
        server.handlers_mut().register("poe.run", move |req| {
            let manager = poe_rm_run.clone();
            async move { handle_poe_run(req, manager).await }
        });

        let poe_rm_status = poe_run_manager.clone();
        server.handlers_mut().register("poe.status", move |req| {
            let manager = poe_rm_status.clone();
            async move { handle_poe_status(req, manager).await }
        });

        let poe_rm_cancel = poe_run_manager.clone();
        server.handlers_mut().register("poe.cancel", move |req| {
            let manager = poe_rm_cancel.clone();
            async move { handle_poe_cancel(req, manager).await }
        });

        let poe_rm_list = poe_run_manager.clone();
        server.handlers_mut().register("poe.list", move |req| {
            let manager = poe_rm_list.clone();
            async move { handle_poe_list(req, manager).await }
        });

        let poe_cs_prepare = poe_contract_service.clone();
        server.handlers_mut().register("poe.prepare", move |req| {
            let service = poe_cs_prepare.clone();
            async move { handle_prepare(req, service).await }
        });

        let poe_cs_sign = poe_contract_service.clone();
        server.handlers_mut().register("poe.sign", move |req| {
            let service = poe_cs_sign.clone();
            async move { handle_sign(req, service).await }
        });

        let poe_cs_reject = poe_contract_service.clone();
        server.handlers_mut().register("poe.reject", move |req| {
            let service = poe_cs_reject.clone();
            async move { handle_reject(req, service).await }
        });

        let poe_cs_pending = poe_contract_service.clone();
        server.handlers_mut().register("poe.pending", move |req| {
            let service = poe_cs_pending.clone();
            async move { handle_pending(req, service).await }
        });

        if !daemon {
            println!("POE (First Principles) methods:");
            println!("  - poe.prepare : Generate contract from instruction");
            println!("  - poe.sign    : Sign contract and start execution");
            println!("  - poe.reject  : Reject pending contract");
            println!("  - poe.pending : List pending contracts");
            println!("  - poe.run     : Execute with pre-built manifest");
            println!("  - poe.status  : Query task status");
            println!("  - poe.cancel  : Cancel running task");
            println!("  - poe.list    : List active tasks");
            println!();
        }
    } else if !daemon {
        println!("POE methods: Disabled (requires ANTHROPIC_API_KEY or OPENAI_API_KEY)");
        println!();
    }
}

/// Return type for initialize_auth: all security objects needed by the caller.
struct AuthBundle {
    device_store: Arc<DeviceStore>,
    auth_ctx: Arc<auth_handlers::AuthContext>,
    mdns_broadcaster: Option<alephcore::gateway::MdnsBroadcaster>,
    invitation_manager: Arc<alephcore::gateway::security::InvitationManager>,
    guest_session_manager: Arc<alephcore::gateway::security::GuestSessionManager>,
}

/// Initialize authentication and security subsystems (construction only).
/// Does NOT register handlers — the orchestrator layer is responsible for that.
fn initialize_auth(
    port: u16,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    require_auth: bool,
    daemon: bool,
) -> AuthBundle {
    use alephcore::utils::paths;
    use tracing::{info, warn};

    let device_store_path = paths::get_devices_db_path()
        .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_devices.db"));

    let device_store = Arc::new(
        DeviceStore::open(&device_store_path)
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to load device store from {:?}: {}. Using in-memory.", device_store_path, e);
                DeviceStore::in_memory().expect("Failed to create in-memory device store")
            })
    );

    let security_store_path = paths::get_security_db_path()
        .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_security.db"));
    let security_store = Arc::new(
        alephcore::gateway::security::SecurityStore::open(&security_store_path)
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to load security store from {:?}: {}. Using in-memory.", security_store_path, e);
                alephcore::gateway::security::SecurityStore::in_memory().expect("Failed to create in-memory security store")
            })
    );

    let token_manager = Arc::new(TokenManager::new(security_store.clone()));
    let pairing_manager = Arc::new(PairingManager::new(security_store.clone()));
    let invitation_manager = Arc::new(alephcore::gateway::security::InvitationManager::new());
    let guest_session_manager = Arc::new(alephcore::gateway::security::GuestSessionManager::new());

    let mdns_broadcaster = match alephcore::gateway::MdnsBroadcaster::new(port, "aleph") {
        Ok(broadcaster) => {
            info!("mDNS service discovery enabled");
            Some(broadcaster)
        }
        Err(e) => {
            warn!("Failed to start mDNS broadcaster: {} (discovery disabled)", e);
            None
        }
    };

    let auth_ctx = Arc::new(auth_handlers::AuthContext {
        token_manager,
        pairing_manager,
        device_store: device_store.clone(),
        security_store,
        invitation_manager: invitation_manager.clone(),
        guest_session_manager: guest_session_manager.clone(),
        event_bus: event_bus.clone(),
        require_auth,
    });

    if !daemon {
        println!("Auth methods:");
        println!("  - connect         : Authenticate connection");
        println!("  - pairing.approve : Approve device pairing");
        println!("  - pairing.reject  : Reject device pairing");
        println!("  - pairing.list    : List pending pairings");
        println!("  - devices.list    : List approved devices");
        println!("  - devices.revoke  : Revoke device access");
        println!();
    }

    AuthBundle { device_store, auth_ctx, mdns_broadcaster, invitation_manager, guest_session_manager }
}

/// Load and return the application config, running secrets vault migration if needed.
async fn load_app_config() -> alephcore::Config {
    use tracing::{info, warn, debug};
    use alephcore::secrets::{SecretVault, resolve_master_key};
    use alephcore::secrets::migration::{needs_migration, migrate_api_keys, save_migrated_config};
    use alephcore::secrets::vault::resolve_provider_secrets;

    let mut config = match alephcore::Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error loading application config: {}", e);
            std::process::exit(1);
        }
    };

    if let Ok(master_key) = resolve_master_key() {
        let vault_path = SecretVault::default_path();
        match SecretVault::open(&vault_path, &master_key) {
            Ok(mut vault) => {
                if needs_migration(&config) {
                    match migrate_api_keys(&mut config, &mut vault) {
                        Ok(result) => {
                            if result.migrated_count > 0 {
                                let _ = save_migrated_config(&config);
                                info!(count = result.migrated_count, "Migrated plaintext API keys to vault");
                            }
                        }
                        Err(e) => warn!(error = %e, "Failed to migrate API keys to vault"),
                    }
                }
                // Build SecretRouter with providers from config
                use alephcore::secrets::provider::local_vault::LocalVaultProvider;
                use alephcore::secrets::provider::onepassword::OnePasswordProvider;
                use alephcore::secrets::provider::SecretProvider;
                use alephcore::secrets::router::SecretRouter;
                use std::sync::Arc;

                let mut providers: std::collections::HashMap<String, Arc<dyn SecretProvider>> = std::collections::HashMap::new();

                // Always register local vault as default "local" provider
                providers.insert(
                    "local".into(),
                    Arc::new(LocalVaultProvider::new(vault)) as Arc<dyn SecretProvider>,
                );

                // Register external providers from config.secret_providers
                for (key, provider_config) in &config.secret_providers {
                    match provider_config.provider_type.as_str() {
                        "local_vault" => {
                            debug!(key = key.as_str(), "Local vault provider already registered as 'local'");
                        }
                        "1password" => {
                            let token = provider_config
                                .service_account_token_env
                                .as_ref()
                                .and_then(|env_name| std::env::var(env_name).ok());
                            let op = OnePasswordProvider::new(
                                provider_config.account.clone(),
                                token,
                            );
                            providers.insert(key.clone(), Arc::new(op) as Arc<dyn SecretProvider>);
                            info!(key = key.as_str(), "Registered 1Password secret provider");
                        }
                        other => {
                            warn!(key = key.as_str(), provider_type = other, "Unknown secret provider type, skipping");
                        }
                    }
                }

                let router = SecretRouter::new(
                    config.secrets.clone(),
                    providers,
                    config.secrets_config.default_provider.clone(),
                );

                if let Err(e) = resolve_provider_secrets(&mut config, &router).await {
                    warn!(error = %e, "Failed to resolve provider secrets");
                }
            }
            Err(e) => warn!(error = %e, "Failed to open secret vault"),
        }
    } else {
        debug!("ALEPH_MASTER_KEY not set, secret vault disabled");
    }

    config
}

/// Spawn Ctrl-C and SIGTERM handlers; return the oneshot receiver for run_until_shutdown.
fn setup_graceful_shutdown(args: &Args) -> tokio::sync::oneshot::Receiver<()> {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let pid_file = args.pid_file.clone();
    let daemon_mode = args.daemon;
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        if !daemon_mode {
            println!("\nShutting down gateway...");
        }
        remove_pid_file(&pid_file);
        let _ = shutdown_tx.send(());
    });

    #[cfg(unix)]
    {
        let pid_file_term = args.pid_file.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            if let Ok(mut stream) = signal(SignalKind::terminate()) {
                stream.recv().await;
                remove_pid_file(&pid_file_term);
                std::process::exit(0);
            }
        });
    }

    shutdown_rx
}

/// Register all messaging channels (iMessage, Telegram, Discord, WhatsApp)
/// and the LinkManager for external bridge plugins.
/// Returns the populated ChannelRegistry.
async fn initialize_channels(
    server: &mut GatewayServer,
    gateway_config: &FullGatewayConfig,
    app_config: &alephcore::Config,
    app_config_arc: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    dispatch_registry: Option<&alephcore::dispatcher::ToolRegistry>,
    daemon: bool,
) -> Arc<ChannelRegistry> {
    let channel_registry = Arc::new(ChannelRegistry::new());

    #[cfg(target_os = "macos")]
    {
        let imessage_config = if let Some(app_im) = app_config.channels.get("imessage") {
            serde_json::from_value::<IMessageConfig>(app_im.clone()).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse imessage config from app config: {}, falling back", e);
                IMessageConfig::default()
            })
        } else {
            IMessageConfig::default()
        };
        let imessage_channel = IMessageChannel::new(imessage_config);
        let channel_id = channel_registry.register(Box::new(imessage_channel)).await;
        if !daemon {
            println!("Registered channel: {} (iMessage)", channel_id);
        }
    }

    {
        // Priority: app config (config.toml, set by Panel UI) > gateway config (aleph.toml) > env var
        let telegram_config = if let Some(app_tg) = app_config.channels.get("telegram") {
            serde_json::from_value::<TelegramConfig>(app_tg.clone()).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse telegram config from app config: {}, falling back", e);
                TelegramConfig::from_env().unwrap_or_default()
            })
        } else if let Some(ref gw_tg) = gateway_config.channels.telegram {
            let mut cfg = TelegramConfig::default();
            cfg.bot_token = gw_tg.token.clone();
            cfg
        } else {
            TelegramConfig::from_env().unwrap_or_default()
        };
        // Build slash commands list from dispatch registry for Telegram menu
        let slash_commands = if let Some(reg) = dispatch_registry {
            use alephcore::dispatcher::ChannelType;
            let tools = reg.list_for_channel(ChannelType::Telegram).await;
            tools.iter()
                .map(|t| (t.name.clone(), t.description.clone()))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let telegram_channel = TelegramChannel::new("telegram", telegram_config)
            .with_slash_commands(slash_commands);
        let channel_id = channel_registry.register(Box::new(telegram_channel)).await;
        if !daemon {
            println!("Registered channel: {} (Telegram)", channel_id);
        }
    }

    {
        let discord_config = DiscordConfig::default();
        let discord_channel = DiscordChannel::new("discord", discord_config);
        let channel_id = channel_registry.register(Box::new(discord_channel)).await;
        if !daemon {
            println!("Registered channel: {} (Discord)", channel_id);
        }
    }

    {
        let whatsapp_config = if let Some(app_wa) = app_config.channels.get("whatsapp") {
            serde_json::from_value::<WhatsAppConfig>(app_wa.clone()).unwrap_or_else(|e| {
                tracing::warn!("Failed to parse whatsapp config from app config: {}, falling back", e);
                WhatsAppConfig::default()
            })
        } else {
            WhatsAppConfig::default()
        };
        let whatsapp_channel = WhatsAppChannel::new("whatsapp", whatsapp_config);
        let channel_id = channel_registry.register(Box::new(whatsapp_channel)).await;
        if !daemon {
            println!("Registered channel: {} (WhatsApp)", channel_id);
        }
    }

    register_channel_handlers(server, &channel_registry, app_config_arc);

    // Auto-start all registered channels
    let start_results = channel_registry.start_all().await;
    for (ch_id, result) in &start_results {
        match result {
            Ok(()) => println!("  ✓ Channel {} started", ch_id),
            Err(e) => eprintln!("  ✗ Channel {} failed: {}", ch_id, e),
        }
    }
    if !daemon {
        let ok_count = start_results.iter().filter(|(_, r)| r.is_ok()).count();
        println!("Auto-started {}/{} channels", ok_count, start_results.len());
    }

    if !daemon {
        println!("Channel methods:");
        println!("  - channels.list   : List all channels");
        println!("  - channels.status : Get channel status");
        println!("  - channel.start   : Start a channel");
        println!("  - channel.stop    : Stop a channel");
        println!("  - channel.send    : Send message via channel");
        println!();
    }

    // Start external bridge plugins via LinkManager
    {
        use alephcore::gateway::link::LinkManager;
        let base_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph");
        let link_manager = LinkManager::new(base_dir);
        if let Err(e) = link_manager.start().await {
            tracing::warn!("LinkManager startup encountered errors: {}", e);
        }
        if !daemon {
            println!("LinkManager started (external bridge plugins)");
            println!();
        }
    }

    channel_registry
}

/// Initialize InboundMessageRouter and start it.
/// Connects the channel registry to the agent router for unified routing.
/// When execution support is available, messages trigger real agent execution.
/// When group chat support is available, `/groupchat` commands are intercepted.
async fn initialize_inbound_router(
    channel_registry: Arc<ChannelRegistry>,
    router: Arc<AgentRouter>,
    execution_adapter: Option<Arc<dyn alephcore::gateway::ExecutionAdapter>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    pairing_store: Arc<dyn alephcore::gateway::pairing_store::PairingStore>,
    group_chat_orch: alephcore::gateway::handlers::group_chat::SharedOrchestrator,
    group_chat_executor: Option<Arc<GroupChatExecutor>>,
    workspace_manager: Option<Arc<alephcore::gateway::WorkspaceManager>>,
    default_provider: Option<Arc<dyn alephcore::providers::AiProvider>>,
    dispatch_registry: Option<Arc<alephcore::dispatcher::ToolRegistry>>,
    daemon: bool,
) {
    let routing_config = RoutingConfig::default();

    // Use full execution support when available, otherwise basic routing
    let mut inbound_router = match (execution_adapter, agent_registry) {
        (Some(ea), Some(ar)) => {
            if !daemon {
                println!("  Inbound router: execution support enabled");
            }
            InboundMessageRouter::with_unified_routing(
                channel_registry.clone(),
                pairing_store.clone(),
                routing_config,
                ar,
                ea,
                router.clone(),
            )
        }
        _ => {
            if !daemon {
                println!("  Inbound router: routing only (no execution support)");
            }
            InboundMessageRouter::new(
                channel_registry.clone(),
                pairing_store.clone(),
                routing_config,
            )
            .with_agent_router(router.clone())
        }
    };

    // Wire group chat support if executor is available
    if let Some(executor) = group_chat_executor {
        inbound_router = inbound_router.with_group_chat(group_chat_orch, executor);
        if !daemon {
            println!("  Inbound router: group chat enabled (/groupchat commands)");
        }
    }

    // Wire workspace manager for /switch command and channel-level agent routing
    if let Some(wm) = workspace_manager {
        inbound_router = inbound_router.with_workspace_manager(wm);
    }

    // Wire intent detector for natural language agent switching
    {
        use alephcore::gateway::IntentDetector;
        let detector = IntentDetector::new();
        // TODO: LLM classify function can be wired here for ambiguous messages
        inbound_router = inbound_router.with_intent_detector(detector);
        if let Some(provider) = default_provider {
            inbound_router = inbound_router.with_llm_provider(provider);
        }
        if !daemon {
            println!("  Inbound router: intent detection enabled (dynamic agent switching)");
        }
    }

    // Default DM policy is Pairing — owner must approve each sender.
    // Channel-specific overrides can be registered here if needed.

    // Wire command parser for unified slash command resolution
    if let Some(reg) = dispatch_registry {
        let command_parser = Arc::new(alephcore::command::CommandParser::new(reg));
        inbound_router = inbound_router.with_command_parser(command_parser);
        if !daemon {
            println!("  Inbound router: slash command resolution enabled (unified registry)");
        }
    }

    let inbound_router = Arc::new(inbound_router);

    let _inbound_router_handle = inbound_router.clone().start().await;
    if !daemon {
        println!("Inbound message router started");
        println!();
    }
}

// ─────────────────────────────────────────────────────────────────────────────

/// Start the gateway server
pub async fn start_server(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::server::GatewayConfig as ServerConfig;

    // Ensure ~/.aleph/ directory structure exists
    if let Ok(config_dir) = alephcore::utils::paths::get_config_dir() {
        let _ = std::fs::create_dir_all(&config_dir);
    }

    // Migrate legacy database files from ~/.aleph/*.db to ~/.aleph/data/*.db
    alephcore::utils::paths::migrate_legacy_db_files();

    // Handle daemon mode
    if args.daemon {
        let log_file = args.log_file.clone().or_else(|| {
            Some(PathBuf::from(DEFAULT_LOG_FILE))
        });
        daemonize(&args.pid_file, log_file.as_ref())?;
    }

    initialize_tracing(args);
    validate_bind_address(args)?;

    let (full_config, final_bind, final_port, final_max_connections) = load_gateway_config(args);

    let addr: SocketAddr = format!("{}:{}", final_bind, final_port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    alephcore::pii::PiiEngine::init(full_config.privacy.clone());
    if !args.daemon {
        print_startup_banner(addr, &full_config);
    }

    let start_time = std::time::Instant::now();

    let server_config = ServerConfig {
        max_connections: final_max_connections,
        require_auth: full_config.gateway.require_auth,
        timeout_secs: 300,
    };
    let mut server = GatewayServer::with_config(addr, server_config);

    let session_manager = initialize_session_manager(args.daemon).await;
    initialize_extension_manager(args.daemon).await;

    // Log desktop capability mode
    if !args.daemon {
        println!("Desktop capabilities: in-process (native)");
    }

    // Initialize memory backend (LanceDB)
    let memory_db: Arc<alephcore::memory::store::LanceMemoryBackend> = {
        let data_dir = alephcore::utils::paths::get_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("aleph_data"));
        match alephcore::memory::store::LanceMemoryBackend::open_or_create(&data_dir).await {
            Ok(backend) => {
                if !args.daemon {
                    println!("Memory backend initialized (LanceDB)");
                }
                Arc::new(backend)
            }
            Err(e) => {
                eprintln!("Error: Failed to initialize memory backend: {}", e);
                std::process::exit(1);
            }
        }
    };

    let event_bus = server.event_bus().clone();

    // Load app config early so agent handlers can use configured providers
    let loaded_app_config = load_app_config().await;

    // Resolve agent definitions from config (initializes workspace directories)
    let mut agent_resolver = alephcore::AgentDefinitionResolver::new();
    let resolved_agents = agent_resolver.resolve_all(&loaded_app_config.agents, &loaded_app_config.profiles);

    // Find default agent
    let default_agent_id = resolved_agents
        .iter()
        .find(|a| a.is_default)
        .map(|a| a.id.clone())
        .unwrap_or_else(|| "main".to_string());

    if !args.daemon {
        for agent in &resolved_agents {
            println!("  Agent '{}': workspace={}", agent.id, agent.workspace_path.display());
        }
    }

    // Build router from config-driven bindings
    let router = Arc::new(AgentRouter::from_bindings(
        loaded_app_config.bindings.clone(),
        &default_agent_id,
    ));

    // Wrap app config in Arc<RwLock> early so agent handlers can read output_mode dynamically
    let app_config = Arc::new(tokio::sync::RwLock::new(loaded_app_config));

    // Initialize WorkspaceManager early so agent management tools can use it
    let workspace_manager: Option<Arc<alephcore::gateway::WorkspaceManager>> = {
        use alephcore::gateway::WorkspaceManager;
        match WorkspaceManager::with_defaults() {
            Ok(wm) => {
                let wm = Arc::new(wm);
                if !args.daemon {
                    println!("Workspace manager initialized (SQLite persistence)");
                }
                Some(wm)
            }
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: Failed to initialize workspace manager: {}. workspace.switch/getActive disabled.", e);
                }
                None
            }
        }
    };

    let agent_result = register_agent_handlers(
        &mut server, session_manager.clone(), event_bus.clone(),
        router.clone(), &full_config, &*app_config.read().await, app_config.clone(), &memory_db,
        workspace_manager.clone(), args.daemon,
    ).await;

    register_poe_handlers(&mut server, event_bus.clone(), args.daemon).await;

    // Auth subsystem construction
    let auth_bundle = initialize_auth(
        args.port, event_bus.clone(),
        full_config.gateway.require_auth, args.daemon,
    );
    register_auth_handlers(&mut server, &auth_bundle.auth_ctx);
    register_guest_handlers(&mut server, &auth_bundle.invitation_manager, &auth_bundle.guest_session_manager, &event_bus);
    server.set_guest_session_manager(auth_bundle.guest_session_manager.clone());
    let config_patcher = {
        let config_path = alephcore::Config::default_path();
        let backup = alephcore::ConfigBackup::new(
            alephcore::ConfigBackup::default_dir(),
            10,
        );
        Arc::new(alephcore::ConfigPatcher::new(
            app_config.clone(),
            config_path,
            None, // Vault will be wired in a future iteration
            backup,
        ))
    };
    let app_config_for_channels = app_config.clone();
    let app_config_for_reload = app_config.clone();
    let app_config_for_oauth = app_config.clone();
    let app_config_for_models = app_config.clone();
    register_config_handlers(&mut server, app_config, config_patcher, event_bus.clone(), auth_bundle.device_store.clone());

    register_session_handlers(&mut server, &session_manager, args.daemon);
    register_memory_handlers(&mut server, &memory_db, args.daemon);
    register_models_handlers(&mut server, &app_config_for_models, args.daemon);
    register_daemon_handlers(&mut server, start_time, args.daemon);

    // OAuth state: restore from config if chatgpt provider has an api_key
    let oauth_state: alephcore::gateway::handlers::oauth::SharedOAuthState = {
        use alephcore::gateway::handlers::oauth::restore_from_config;
        let restored = restore_from_config(&*app_config_for_oauth.read().await);
        if restored.is_some() && !args.daemon {
            println!("OAuth: restored ChatGPT token from config");
        }
        Arc::new(tokio::sync::RwLock::new(restored))
    };
    register_oauth_handlers(&mut server, &oauth_state, &app_config_for_oauth, args.daemon);

    if let Some(ref wm) = workspace_manager {
        register_workspace_handlers(&mut server, wm, &memory_db, args.daemon);
    }

    // Identity resolver (shared for session-level overrides)
    let identity_resolver: alephcore::gateway::handlers::identity::SharedIdentityResolver = Arc::new(
        tokio::sync::RwLock::new(
            alephcore::thinker::identity::IdentityResolver::with_defaults()
        )
    );
    register_identity_handlers(&mut server, &identity_resolver);

    // Initialize CronService
    {
        let app_cfg = app_config_for_channels.read().await;
        let cron_config = app_cfg.cron.clone();
        drop(app_cfg);

        match CronService::new(cron_config) {
            Ok(mut cron_service) => {
                // Wire JobExecutor using the provider registry (if available)
                if can_create_provider_from_env() {
                    if let Ok(provider_reg) = create_provider_registry_from_env() {
                        let provider = provider_reg.default_provider();
                        let executor: alephcore::cron::JobExecutor = Arc::new(move |_job_id, _agent_id, prompt| {
                            let provider = provider.clone();
                            Box::pin(async move {
                                provider.process(&prompt, None).await.map_err(|e| format!("{e}"))
                            }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
                        });
                        cron_service.set_executor(executor);
                    }
                }

                let shared_cron: alephcore::gateway::handlers::cron::SharedCronService =
                    Arc::new(tokio::sync::Mutex::new(cron_service));
                register_cron_handlers(&mut server, &shared_cron, args.daemon);
            }
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: Failed to initialize CronService: {}. Cron stubs remain.", e);
                }
            }
        }
    }

    // Initialize GroupChat Orchestrator + Executor
    // (shared_orch and gc_executor are kept alive for both RPC handlers and InboundMessageRouter)
    let (shared_orch, gc_executor) = {
        let app_cfg = app_config_for_channels.read().await;
        let gc_config = app_cfg.group_chat.clone();
        let persona_configs = app_cfg.personas.clone();
        drop(app_cfg);

        let orchestrator = GroupChatOrchestrator::new(gc_config, &persona_configs);
        let shared_orch: alephcore::gateway::handlers::group_chat::SharedOrchestrator =
            Arc::new(tokio::sync::Mutex::new(orchestrator));

        // Create executor with default provider (if available)
        let gc_executor: Option<Arc<GroupChatExecutor>> = if can_create_provider_from_env() {
            create_provider_registry_from_env()
                .ok()
                .map(|reg| Arc::new(GroupChatExecutor::new(reg.default_provider())))
        } else {
            None
        };

        if let Some(ref executor) = gc_executor {
            register_group_chat_handlers(&mut server, &shared_orch, executor, args.daemon);
        } else if !args.daemon {
            println!("Group Chat: Disabled (requires ANTHROPIC_API_KEY or OPENAI_API_KEY)");
            println!();
        }

        (shared_orch, gc_executor)
    };

    // Create channel pairing store (shared between InboundMessageRouter and RPC handlers)
    let channel_pairing_store: Arc<dyn alephcore::gateway::pairing_store::PairingStore> = {
        let pairing_store_path = alephcore::utils::paths::get_pairing_db_path()
            .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_pairing.db"));
        Arc::new(
            SqlitePairingStore::new(&pairing_store_path)
                .unwrap_or_else(|e| {
                    eprintln!("Warning: Failed to create pairing store: {}. Using in-memory.", e);
                    SqlitePairingStore::in_memory().expect("Failed to create in-memory pairing store")
                })
        )
    };

    // Register channel pairing RPC handlers (uses same store as InboundMessageRouter)
    {
        use alephcore::gateway::handlers::pairing as pairing_handlers;

        let store = channel_pairing_store.clone();
        server.handlers_mut().register("channel.pairing.list", move |req| {
            let store = store.clone();
            async move { pairing_handlers::handle_list(req, store).await }
        });

        let store = channel_pairing_store.clone();
        server.handlers_mut().register("channel.pairing.approve", move |req| {
            let store = store.clone();
            async move { pairing_handlers::handle_approve(req, store).await }
        });

        let store = channel_pairing_store.clone();
        server.handlers_mut().register("channel.pairing.reject", move |req| {
            let store = store.clone();
            async move { pairing_handlers::handle_reject(req, store).await }
        });

        let store = channel_pairing_store.clone();
        server.handlers_mut().register("channel.pairing.approved", move |req| {
            let store = store.clone();
            async move { pairing_handlers::handle_approved_list(req, store).await }
        });

        let store = channel_pairing_store.clone();
        server.handlers_mut().register("channel.pairing.revoke", move |req| {
            let store = store.clone();
            async move { pairing_handlers::handle_revoke(req, store).await }
        });

        if !args.daemon {
            println!("Channel pairing methods:");
            println!("  - channel.pairing.list     : List pending channel pairing requests");
            println!("  - channel.pairing.approve  : Approve a channel sender");
            println!("  - channel.pairing.reject   : Reject a channel sender");
            println!("  - channel.pairing.approved : List approved channel senders");
            println!("  - channel.pairing.revoke   : Revoke a channel sender");
            println!();
        }
    }

    let app_config_snapshot = app_config_for_channels.read().await.clone();
    let channel_registry = initialize_channels(&mut server, &full_config, &app_config_snapshot, &app_config_for_channels, agent_result.dispatch_registry.as_deref(), args.daemon).await;
    initialize_inbound_router(
        channel_registry, router,
        agent_result.execution_adapter, agent_result.agent_registry,
        channel_pairing_store,
        shared_orch, gc_executor,
        workspace_manager,
        agent_result.default_provider,
        agent_result.dispatch_registry,
        args.daemon,
    ).await;

    let config_path = args.config.clone()
        .map(|p| expand_path(&p.to_string_lossy()))
        .or_else(|| dirs::home_dir().map(|h| h.join(".aleph/config.toml")));
    let _config_watcher = setup_config_watcher(&mut server, config_path, &event_bus, args.daemon, Some(app_config_for_reload)).await;

    start_webchat_server(args, &final_bind, final_port).await;

    // Start desktop bridge (non-blocking — server runs headless if bridge not found)
    let run_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("run");
    let mut bridge_manager = DesktopBridgeManager::new(run_dir, final_port);
    if let Err(e) = bridge_manager.start().await {
        tracing::warn!("Desktop bridge not started: {e} — running headless");
    }

    if !args.daemon {
        println!();
        println!("Aleph Server:");
        println!("  - URL:       http://{}:{}", final_bind, final_port);
        println!("  - WebSocket: ws://{}:{}/ws", final_bind, final_port);
        println!("  - Panel UI:  http://{}:{}/", final_bind, final_port);
        println!();
    }

    let shutdown_rx = setup_graceful_shutdown(args);
    server.run_until_shutdown(shutdown_rx).await?;

    // Graceful shutdown: stop desktop bridge and mDNS
    bridge_manager.stop().await;

    if let Some(broadcaster) = auth_bundle.mdns_broadcaster {
        broadcaster.shutdown();
    }

    Ok(())
}
