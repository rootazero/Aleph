//! Server startup command handler
//!
//! This module contains the main server initialization and startup logic.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::cli::Args;
use crate::daemon::{expand_path, remove_pid_file};

use alephcore::gateway::GatewayServer;
use alephcore::gateway::bridge::DesktopBridgeManager;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::{
    can_create_provider_from_env, create_provider_registry_from_env,
    GatewayConfig as FullGatewayConfig,
    SessionManager, SessionManagerConfig,
};
use alephcore::gateway::pairing_store::SqlitePairingStore;
use alephcore::cron::CronService;
use alephcore::group_chat::{GroupChatExecutor, GroupChatOrchestrator};
use alephcore::ProviderRegistry as _; // trait needed for .default_provider()

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
            // SkillSystem is now always initialized; load_all() will init it
            // with discovered skill directories automatically.

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

    // Daemonization is handled in main() before tokio starts (fork safety).

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

    // Bootstrap default skills and plugins from GitHub on first run
    alephcore::discovery::bootstrap_repositories(args.daemon);

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

    // Create agent manager (shared between tool config and RPC handlers)
    let agent_manager = Arc::new(alephcore::AgentManager::new(
        alephcore::Config::default_path(),
        dirs::home_dir().unwrap_or_default().join(".aleph/workspaces"),
        dirs::home_dir().unwrap_or_default().join(".aleph/agents"),
        dirs::home_dir().unwrap_or_default().join(".aleph/trash"),
    ));

    let agent_result = register_agent_handlers(
        &mut server, session_manager.clone(), event_bus.clone(),
        router.clone(), &full_config, &*app_config.read().await, app_config.clone(), &memory_db,
        workspace_manager.clone(), agent_manager.clone(), args.daemon,
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
    register_memory_handlers(&mut server, &memory_db, &agent_result.compression_service, args.daemon);
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

    // Agent management (agent_manager created earlier for tool config sharing)
    register_agents_handlers(&mut server, &agent_manager, &event_bus);

    // Identity resolver (shared for session-level overrides)
    let identity_resolver: alephcore::gateway::handlers::identity::SharedIdentityResolver = Arc::new(
        tokio::sync::RwLock::new(
            alephcore::thinker::identity::IdentityResolver::with_defaults()
        )
    );
    register_identity_handlers(&mut server, &identity_resolver);

    // Initialize A2A subsystem (if enabled)
    {
        let app_cfg = app_config_for_channels.read().await;
        let a2a_config = app_cfg.a2a.clone();
        drop(app_cfg);

        if a2a_config.enabled {
            use alephcore::a2a::adapter::server::{TaskStore, StreamHub, AgentLoopBridge};
            use alephcore::a2a::adapter::server::A2AServerState;
            use alephcore::a2a::adapter::auth::TieredAuthenticator;
            use alephcore::a2a::service::{CardBuilder, CardRegistry, SmartRouter, NotificationService};
            use alephcore::a2a::adapter::client::A2AClientPool;
            use alephcore::a2a::sub_agent::A2ASubAgent;
            use alephcore::a2a::port::{A2ATaskManager, A2AMessageHandler, A2AStreamingHandler};
            use alephcore::a2a::port::authenticator::A2AAuthenticator;

            // 1. Create server-side components
            let task_store: Arc<dyn A2ATaskManager> = Arc::new(TaskStore::new());
            let stream_hub: Arc<dyn A2AStreamingHandler> = Arc::new(StreamHub::new());

            // 2. Create bridge (needs execution adapter + agent registry)
            if let (Some(exec_adapter), Some(registry)) =
                (&agent_result.execution_adapter, &agent_result.agent_registry)
            {
                let message_handler: Arc<dyn A2AMessageHandler> = Arc::new(AgentLoopBridge::new(
                    registry.clone(),
                    exec_adapter.clone(),
                    task_store.clone(),
                    stream_hub.clone(),
                ));

                // 3. Create authenticator
                let authenticator: Arc<dyn A2AAuthenticator> = Arc::new(
                    TieredAuthenticator::new(
                        a2a_config.server.security.local_bypass,
                        a2a_config.server.security.tokens.clone(),
                    )
                );

                // 4. Build agent card
                let card = CardBuilder::build(&a2a_config.server, &format!("{}", addr));

                // 5. Create notification service
                let notification = Arc::new(NotificationService::new());

                // 6. Build A2AServerState and set on server
                let a2a_server_state = Arc::new(A2AServerState {
                    task_manager: task_store,
                    message_handler,
                    streaming: stream_hub,
                    authenticator,
                    notification,
                    card,
                });
                server.set_a2a_state(a2a_server_state);

                // 7. Create client-side components (CardRegistry, SmartRouter, ClientPool)
                let card_registry = Arc::new(CardRegistry::new());
                card_registry.load_from_config(&a2a_config).await;
                // 8. Wire LLM semantic matcher (if default provider available)
                let smart_router = if let Some(ref provider) = agent_result.default_provider {
                    use alephcore::a2a::service::SemanticLlmMatcher;
                    let matcher = Arc::new(SemanticLlmMatcher::new(provider.clone()));
                    Arc::new(SmartRouter::new(card_registry).with_llm_matcher(matcher))
                } else {
                    Arc::new(SmartRouter::new(card_registry))
                };

                let client_pool = Arc::new(A2AClientPool::new());

                // 9. Create A2ASubAgent and refresh cached names for can_handle
                let a2a_sub_agent = Arc::new(A2ASubAgent::new(smart_router, client_pool));
                a2a_sub_agent.refresh_agent_names().await;

                // 10. Register with SubAgentDispatcher (enables delegate tool)
                if let Some(ref dispatcher) = agent_result.sub_agent_dispatcher {
                    let mut disp = dispatcher.write().await;
                    disp.register(a2a_sub_agent);
                }

                if !args.daemon {
                    println!("A2A protocol: enabled");
                    println!("  - Agent Card: /.well-known/agent-card.json");
                    println!("  - RPC:        /a2a (sync), /a2a/stream (SSE)");
                    if agent_result.default_provider.is_some() {
                        println!("  - LLM routing: enabled (semantic agent matching)");
                    }
                    println!();
                }
            } else if !args.daemon {
                println!("A2A: skipping server (no execution adapter available)");
                println!();
            }
        } else if !args.daemon {
            println!("A2A protocol: disabled (set [a2a] enabled = true in config)");
            println!();
        }
    }

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
    let channel_registry = initialize_channels(&mut server, &app_config_snapshot, &app_config_for_channels, agent_result.dispatch_registry.as_deref(), args.daemon).await;
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
