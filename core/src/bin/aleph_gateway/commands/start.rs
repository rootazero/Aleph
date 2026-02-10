//! Server startup command handler
//!
//! This module contains the main server initialization and startup logic.

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::cli::Args;
use crate::daemon::{expand_path, remove_pid_file, daemonize};

#[cfg(feature = "gateway")]
use std::sync::Arc;

#[cfg(feature = "gateway")]
use alephcore::gateway::GatewayServer;
#[cfg(feature = "gateway")]
use alephcore::gateway::router::AgentRouter;
#[cfg(feature = "gateway")]
use alephcore::gateway::handlers::agent::{
    AgentRunManager, handle_run,
    handle_status as handle_agent_status,
    handle_cancel as handle_agent_cancel,
};
#[cfg(feature = "gateway")]
use alephcore::gateway::{
    can_create_provider_from_env, create_provider_registry_from_env,
    ExecutionEngine, ExecutionEngineConfig, AgentRegistry,
    GatewayConfig as FullGatewayConfig,
    SessionManager, SessionManagerConfig,
    ChannelRegistry, InboundMessageRouter, RoutingConfig,
    ConfigWatcher, ConfigWatcherConfig, ConfigEvent,
};
#[cfg(feature = "gateway")]
use alephcore::gateway::pairing_store::SqlitePairingStore;
#[cfg(feature = "gateway")]
use alephcore::gateway::handlers::session as session_handlers;
#[cfg(feature = "gateway")]
use alephcore::gateway::handlers::channel as channel_handlers;
#[cfg(feature = "gateway")]
use alephcore::gateway::handlers::config as config_handlers;
#[cfg(feature = "gateway")]
use alephcore::gateway::handlers::auth as auth_handlers;
#[cfg(feature = "gateway")]
use alephcore::gateway::security::{TokenManager, PairingManager};
#[cfg(feature = "gateway")]
use alephcore::gateway::device_store::DeviceStore;
#[cfg(all(feature = "gateway", target_os = "macos"))]
use alephcore::gateway::channels::imessage::{IMessageChannel, IMessageConfig};
#[cfg(feature = "gateway")]
use alephcore::executor::BuiltinToolRegistry;
#[cfg(feature = "gateway")]
use alephcore::gateway::handlers::poe::{
    handle_run as handle_poe_run, handle_status as handle_poe_status,
    handle_cancel as handle_poe_cancel, handle_list as handle_poe_list,
    handle_prepare, handle_sign, handle_reject, handle_pending,
};
#[cfg(feature = "gateway")]
use alephcore::poe::{
    CompositeValidator, GatewayAgentLoopWorker, ManifestBuilder, PoeConfig,
    create_gateway_worker,
    // Service layer
    PoeRunManager, PoeContractService, WorkerFactory, ValidatorFactory,
};
#[cfg(feature = "gateway")]
use alephcore::gateway::{
    create_claude_provider_from_env, available_provider_from_env,
};

use crate::cli::DEFAULT_LOG_FILE;
use crate::server_init::{serve_webchat, handle_run_with_engine};

/// Start the gateway server
#[cfg(feature = "gateway")]
pub async fn start_server(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::server::GatewayConfig as ServerConfig;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    use tracing::{info, warn};

    // Handle daemon mode
    if args.daemon {
        let log_file = args.log_file.clone().or_else(|| {
            Some(PathBuf::from(DEFAULT_LOG_FILE))
        });
        daemonize(&args.pid_file, log_file.as_ref())?;
    }

    // Initialize tracing
    let filter = format!("aleph_gateway={},alephcore::gateway={}", args.log_level, args.log_level);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false))
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter)))
        .init();

    let addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    // Check if port is available (unless --force is specified)
    if !args.force {
        if let Err(e) = std::net::TcpListener::bind(addr) {
            eprintln!("Error: Cannot bind to {}: {}", addr, e);
            eprintln!("Hint: Use --force to attempt to start anyway, or choose a different port with --port");
            std::process::exit(1);
        }
    }

    // Load configuration from file or defaults
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
            // Try default location, fall back to defaults if not found
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
    let final_port = if args.port != 18789 {
        args.port
    } else {
        full_config.gateway.port
    };
    let final_max_connections = if args.max_connections != 1000 {
        args.max_connections
    } else {
        full_config.gateway.max_connections
    };

    // Update addr with possibly overridden values
    let addr: SocketAddr = format!("{}:{}", final_bind, final_port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    if !args.daemon {
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

    let config = ServerConfig {
        max_connections: final_max_connections,
        require_auth: full_config.gateway.require_auth,
        timeout_secs: 300,
    };

    let mut server = GatewayServer::with_config(addr, config);

    // Initialize SessionManager for persistent session storage (before creating agents)
    let session_manager: Arc<SessionManager> = match SessionManager::with_defaults() {
        Ok(sm) => {
            if !args.daemon {
                println!("Session manager initialized (SQLite persistence)");
            }
            Arc::new(sm)
        }
        Err(e) => {
            eprintln!("Warning: Failed to initialize session manager: {}. Using temp storage.", e);
            // Create fallback with temporary path
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
    };

    // Initialize ExtensionManager for plugin system
    match alephcore::extension::ExtensionManager::with_defaults().await {
        Ok(extension_manager) => {
            let manager = Arc::new(extension_manager);
            if let Err(_existing) = alephcore::gateway::init_extension_manager(manager) {
                // Already initialized (shouldn't happen in normal flow)
                if !args.daemon {
                    println!("Extension manager already initialized");
                }
            } else if !args.daemon {
                println!("Extension manager initialized");
            }
        }
        Err(e) => {
            if !args.daemon {
                eprintln!("Warning: Failed to initialize extension manager: {}. Plugin tools will be unavailable.", e);
            }
        }
    }

    // Set up agent.run handler with dependencies
    let event_bus = server.event_bus().clone();
    let router = Arc::new(AgentRouter::new());

    // Create shared AgentRunManager for tracking run states (used by both modes)
    let run_manager = Arc::new(AgentRunManager::new(router.clone(), event_bus.clone()));

    // Try to create real ExecutionEngine with Claude provider
    if can_create_provider_from_env() {
        match create_provider_registry_from_env() {
            Ok(provider_registry) => {
                // Create BuiltinToolRegistry
                let tool_registry = Arc::new(BuiltinToolRegistry::new());

                // Build tools list from builtin definitions
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

                // Create ExecutionEngine
                let engine = Arc::new(ExecutionEngine::new(
                    ExecutionEngineConfig::default(),
                    provider_registry,
                    tool_registry,
                    tools,
                    session_manager.clone(),
                ));

                // Create agent registry with agents from config (using SessionManager for persistence)
                let agent_registry = Arc::new(AgentRegistry::new());
                for agent_config in full_config.get_agent_instance_configs() {
                    let agent_id = agent_config.agent_id.clone();
                    match alephcore::gateway::AgentInstance::with_session_manager(
                        agent_config,
                        session_manager.clone(),
                    ) {
                        Ok(agent) => {
                            agent_registry.register(agent).await;
                            if !args.daemon {
                                println!("  Registered agent: {} (with SQLite persistence)", agent_id);
                            }
                        }
                        Err(e) => {
                            eprintln!("Warning: Failed to create agent '{}': {}", agent_id, e);
                        }
                    }
                }

                if !args.daemon {
                    let provider_name = available_provider_from_env().unwrap_or("unknown");
                    println!("  Mode: Real AgentLoop ({} API)", provider_name);
                    println!();
                }

                // Register agent.run handler with real execution
                let engine_clone = engine.clone();
                let event_bus_clone = event_bus.clone();
                let router_clone = router.clone();
                let agent_registry_clone = agent_registry.clone();
                server.handlers_mut().register("agent.run", move |req| {
                    let engine = engine_clone.clone();
                    let event_bus = event_bus_clone.clone();
                    let router = router_clone.clone();
                    let agent_registry = agent_registry_clone.clone();
                    async move {
                        handle_run_with_engine(req, engine, event_bus, router, agent_registry).await
                    }
                });
            }
            Err(e) => {
                if !args.daemon {
                    eprintln!("Warning: Failed to create provider: {}. Falling back to simulated mode.", e);
                }
                // Fall back to simulated mode using shared run_manager
                let run_manager_clone = run_manager.clone();
                server.handlers_mut().register("agent.run", move |req| {
                    let manager = run_manager_clone.clone();
                    async move { handle_run(req, manager).await }
                });
            }
        }
    } else {
        if !args.daemon {
            println!("  Mode: Simulated (set ANTHROPIC_API_KEY or OPENAI_API_KEY for real execution)");
            println!();
        }

        // Use simulated AgentRunManager (shared)
        let run_manager_clone = run_manager.clone();
        server.handlers_mut().register("agent.run", move |req| {
            let manager = run_manager_clone.clone();
            async move { handle_run(req, manager).await }
        });
    }

    // Register agent.status and agent.cancel handlers (work for both real and simulated modes)
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

    if !args.daemon {
        println!("Agent control methods:");
        println!("  - agent.run     : Execute agent request with streaming");
        println!("  - agent.status  : Query run status by run_id");
        println!("  - agent.cancel  : Cancel an active run");
        println!();
    }

    // Initialize POE (Principle-Operation-Evaluation) services
    if let Ok(poe_provider) = create_claude_provider_from_env() {
        // Create ManifestBuilder for contract generation
        let poe_provider_arc: Arc<dyn alephcore::providers::AiProvider> = poe_provider;
        let manifest_builder = Arc::new(ManifestBuilder::new(poe_provider_arc.clone()));

        // Create factories for PoeRunManager
        // WorkerFactory creates a GatewayAgentLoopWorker for each run
        let provider_for_worker = poe_provider_arc.clone();
        let worker_factory: WorkerFactory<GatewayAgentLoopWorker> = Arc::new(move || {
            create_gateway_worker(
                provider_for_worker.clone(),
                PathBuf::from("/tmp/poe-workspace"),
            )
        });

        // ValidatorFactory creates a CompositeValidator for each run
        let provider_for_validator = poe_provider_arc.clone();
        let validator_factory: ValidatorFactory = Arc::new(move || {
            CompositeValidator::new(provider_for_validator.clone())
        });

        // Create PoeRunManager for direct execution
        let poe_run_manager = Arc::new(PoeRunManager::new(
            event_bus.clone(),
            worker_factory,
            validator_factory,
            PoeConfig::default(),
        ));

        // Create PoeContractService for contract signing workflow
        let poe_contract_service = Arc::new(PoeContractService::new(
            manifest_builder,
            poe_run_manager.clone(),
            event_bus.clone(),
        ));

        // Register POE direct execution handlers
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

        // Register POE contract signing handlers
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

        if !args.daemon {
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
    } else if !args.daemon {
        println!("POE methods: Disabled (requires ANTHROPIC_API_KEY or OPENAI_API_KEY)");
        println!();
    }

    // Initialize authentication context
    let device_store_path = dirs::home_dir()
        .map(|h| h.join(".aleph/devices.db"))
        .unwrap_or_else(|| PathBuf::from("/tmp/aleph_devices.db"));

    // Ensure parent directory exists
    if let Some(parent) = device_store_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let device_store = Arc::new(
        DeviceStore::open(&device_store_path)
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to load device store from {:?}: {}. Using in-memory.", device_store_path, e);
                DeviceStore::in_memory().expect("Failed to create in-memory device store")
            })
    );

    // Initialize security store for tokens
    let security_store_path = device_store_path.parent()
        .map(|p| p.join("security.db"))
        .unwrap_or_else(|| PathBuf::from("/tmp/aleph_security.db"));
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

    // Start mDNS broadcaster for local network discovery
    let mdns_broadcaster = match alephcore::gateway::MdnsBroadcaster::new(args.port, "aleph") {
        Ok(broadcaster) => {
            info!("mDNS service discovery enabled");
            Some(broadcaster)
        }
        Err(e) => {
            warn!("Failed to start mDNS broadcaster: {} (discovery disabled)", e);
            None
        }
    };

    let auth_ctx = Arc::new(auth_handlers::AuthContext::new(
        token_manager,
        pairing_manager,
        device_store.clone(),
        security_store,
        invitation_manager.clone(),
        guest_session_manager.clone(),
        event_bus.clone(),
        full_config.gateway.require_auth,
    ));

    // Register auth handlers
    register_auth_handlers(&mut server, &auth_ctx);

    // Register guest handlers
    register_guest_handlers(&mut server, &invitation_manager, &guest_session_manager, &event_bus);

    // Set guest session manager on server for disconnect cleanup
    server.set_guest_session_manager(guest_session_manager.clone());

    // Register config handlers (for ConfigManager SDK)
    let app_config = Arc::new(tokio::sync::RwLock::new(alephcore::Config::default()));
    register_config_handlers(&mut server, app_config, event_bus.clone(), device_store.clone());

    if !args.daemon {
        println!("Auth methods:");
        println!("  - connect         : Authenticate connection");
        println!("  - pairing.approve : Approve device pairing");
        println!("  - pairing.reject  : Reject device pairing");
        println!("  - pairing.list    : List pending pairings");
        println!("  - devices.list    : List approved devices");
        println!("  - devices.revoke  : Revoke device access");
        println!();
    }

    // Register session handlers with SessionManager
    register_session_handlers(&mut server, &session_manager);

    if !args.daemon {
        println!("  - sessions.list   : List all sessions");
        println!("  - sessions.history: Get session message history");
        println!("  - sessions.reset  : Clear session messages");
        println!("  - sessions.delete : Delete a session");
        println!();
    }

    // Initialize ChannelRegistry for multi-channel messaging
    let channel_registry = Arc::new(ChannelRegistry::new());

    // Register iMessage channel on macOS
    #[cfg(target_os = "macos")]
    {
        // Create iMessage config with enabled = true
        let imessage_config = IMessageConfig {
            enabled: true,
            ..Default::default()
        };

        let imessage_channel = IMessageChannel::new(imessage_config);
        let channel_id = channel_registry.register(Box::new(imessage_channel)).await;
        if !args.daemon {
            println!("Registered channel: {} (iMessage)", channel_id);
        }
    }

    // Register channel handlers
    register_channel_handlers(&mut server, &channel_registry);

    if !args.daemon {
        println!("Channel methods:");
        println!("  - channels.list   : List all channels");
        println!("  - channels.status : Get channel status");
        println!("  - channel.start   : Start a channel");
        println!("  - channel.stop    : Stop a channel");
        println!("  - channel.send    : Send message via channel");
        println!();
    }

    // Initialize PairingStore for InboundMessageRouter
    let pairing_store_path = dirs::home_dir()
        .map(|h| h.join(".aleph/pairing.db"))
        .unwrap_or_else(|| PathBuf::from("/tmp/aleph_pairing.db"));

    // Ensure parent directory exists
    if let Some(parent) = pairing_store_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let pairing_store: Arc<dyn alephcore::gateway::pairing_store::PairingStore> = Arc::new(
        SqlitePairingStore::new(&pairing_store_path)
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to create pairing store: {}. Using in-memory.", e);
                SqlitePairingStore::in_memory().expect("Failed to create in-memory pairing store")
            })
    );

    // Create InboundMessageRouter with unified routing (uses AgentRouter bindings)
    let routing_config = RoutingConfig::default();
    let inbound_router = Arc::new(
        InboundMessageRouter::new(
            channel_registry.clone(),
            pairing_store.clone(),
            routing_config,
        )
        .with_agent_router(router.clone())
    );

    // Start the inbound message router
    let _inbound_router_handle = inbound_router.clone().start().await;
    if !args.daemon {
        println!("Inbound message router started");
        println!();
    }

    // Initialize ConfigWatcher for hot configuration reload
    let config_path = args.config.clone()
        .map(|p| expand_path(&p.to_string_lossy()))
        .or_else(|| {
            dirs::home_dir().map(|h| h.join(".aleph/config.toml"))
        });

    let _config_watcher = setup_config_watcher(&mut server, config_path, &event_bus, args.daemon).await;

    // Start WebChat HTTP server if configured
    start_webchat_server(args, &final_bind, final_port).await;

    // Start ControlPlane embedded UI server
    #[cfg(feature = "control-plane")]
    start_control_plane_server(&final_bind, final_port, args.daemon).await;

    // Set up graceful shutdown
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

    // Also handle SIGTERM for daemon mode
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

    server.run_until_shutdown(shutdown_rx).await?;

    // Clean shutdown: unregister mDNS service
    if let Some(broadcaster) = mdns_broadcaster {
        broadcaster.shutdown();
    }

    Ok(())
}

#[cfg(feature = "gateway")]
fn register_auth_handlers(
    server: &mut GatewayServer,
    auth_ctx: &Arc<auth_handlers::AuthContext>,
) {
    // connect
    let auth_ctx_connect = auth_ctx.clone();
    server.handlers_mut().register("connect", move |req| {
        let ctx = auth_ctx_connect.clone();
        async move { auth_handlers::handle_connect(req, ctx).await }
    });

    // pairing.approve
    let auth_ctx_pairing_approve = auth_ctx.clone();
    server.handlers_mut().register("pairing.approve", move |req| {
        let ctx = auth_ctx_pairing_approve.clone();
        async move { auth_handlers::handle_pairing_approve(req, ctx).await }
    });

    // pairing.reject
    let auth_ctx_pairing_reject = auth_ctx.clone();
    server.handlers_mut().register("pairing.reject", move |req| {
        let ctx = auth_ctx_pairing_reject.clone();
        async move { auth_handlers::handle_pairing_reject(req, ctx).await }
    });

    // pairing.list
    let auth_ctx_pairing_list = auth_ctx.clone();
    server.handlers_mut().register("pairing.list", move |req| {
        let ctx = auth_ctx_pairing_list.clone();
        async move { auth_handlers::handle_pairing_list(req, ctx).await }
    });

    // devices.list
    let auth_ctx_devices_list = auth_ctx.clone();
    server.handlers_mut().register("devices.list", move |req| {
        let ctx = auth_ctx_devices_list.clone();
        async move { auth_handlers::handle_devices_list(req, ctx).await }
    });

    // devices.revoke
    let auth_ctx_devices_revoke = auth_ctx.clone();
    server.handlers_mut().register("devices.revoke", move |req| {
        let ctx = auth_ctx_devices_revoke.clone();
        async move { auth_handlers::handle_devices_revoke(req, ctx).await }
    });
}

#[cfg(feature = "gateway")]
fn register_guest_handlers(
    server: &mut GatewayServer,
    invitation_manager: &Arc<alephcore::gateway::security::InvitationManager>,
    session_manager: &Arc<alephcore::gateway::security::GuestSessionManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::guests;

    // guests.createInvitation
    let mgr_create = invitation_manager.clone();
    let bus_create = event_bus.clone();
    server.handlers_mut().register("guests.createInvitation", move |req| {
        let mgr = mgr_create.clone();
        let bus = bus_create.clone();
        async move { guests::handle_create_invitation(req, mgr, bus).await }
    });

    // guests.listPending
    let mgr_list = invitation_manager.clone();
    server.handlers_mut().register("guests.listPending", move |req| {
        let mgr = mgr_list.clone();
        async move { guests::handle_list_guests(req, mgr).await }
    });

    // guests.revokeInvitation
    let mgr_revoke = invitation_manager.clone();
    let bus_revoke = event_bus.clone();
    server.handlers_mut().register("guests.revokeInvitation", move |req| {
        let mgr = mgr_revoke.clone();
        let bus = bus_revoke.clone();
        async move { guests::handle_revoke_invitation(req, mgr, bus).await }
    });

    // guests.listSessions
    let sess_list = session_manager.clone();
    server.handlers_mut().register("guests.listSessions", move |req| {
        let sess = sess_list.clone();
        async move { guests::handle_list_sessions(req, sess).await }
    });

    // guests.terminateSession
    let sess_terminate = session_manager.clone();
    let bus_terminate = event_bus.clone();
    server.handlers_mut().register("guests.terminateSession", move |req| {
        let sess = sess_terminate.clone();
        let bus = bus_terminate.clone();
        async move { guests::handle_terminate_session(req, sess, bus).await }
    });

    // guests.getActivityLogs
    let sess_logs = session_manager.clone();
    server.handlers_mut().register("guests.getActivityLogs", move |req| {
        let sess = sess_logs.clone();
        async move { guests::handle_get_activity_logs(req, sess).await }
    });
}

#[cfg(feature = "gateway")]
fn register_session_handlers(
    server: &mut GatewayServer,
    session_manager: &Arc<SessionManager>,
) {
    // sessions.list
    let sm_list = session_manager.clone();
    server.handlers_mut().register("sessions.list", move |req| {
        let sm = sm_list.clone();
        async move { session_handlers::handle_list_db(req, sm).await }
    });

    // sessions.history
    let sm_history = session_manager.clone();
    server.handlers_mut().register("sessions.history", move |req| {
        let sm = sm_history.clone();
        async move { session_handlers::handle_history_db(req, sm).await }
    });

    // sessions.reset
    let sm_reset = session_manager.clone();
    server.handlers_mut().register("sessions.reset", move |req| {
        let sm = sm_reset.clone();
        async move { session_handlers::handle_reset_db(req, sm).await }
    });

    // sessions.delete
    let sm_delete = session_manager.clone();
    server.handlers_mut().register("sessions.delete", move |req| {
        let sm = sm_delete.clone();
        async move { session_handlers::handle_delete_db(req, sm).await }
    });
}

#[cfg(feature = "gateway")]
fn register_channel_handlers(
    server: &mut GatewayServer,
    channel_registry: &Arc<ChannelRegistry>,
) {
    // channels.list
    let cr_list = channel_registry.clone();
    server.handlers_mut().register("channels.list", move |req| {
        let cr = cr_list.clone();
        async move { channel_handlers::handle_list(req, cr).await }
    });

    // channels.status
    let cr_status = channel_registry.clone();
    server.handlers_mut().register("channels.status", move |req| {
        let cr = cr_status.clone();
        async move { channel_handlers::handle_status(req, cr).await }
    });

    // channel.start
    let cr_start = channel_registry.clone();
    server.handlers_mut().register("channel.start", move |req| {
        let cr = cr_start.clone();
        async move { channel_handlers::handle_start(req, cr).await }
    });

    // channel.stop
    let cr_stop = channel_registry.clone();
    server.handlers_mut().register("channel.stop", move |req| {
        let cr = cr_stop.clone();
        async move { channel_handlers::handle_stop(req, cr).await }
    });

    // channel.send
    let cr_send = channel_registry.clone();
    server.handlers_mut().register("channel.send", move |req| {
        let cr = cr_send.clone();
        async move { channel_handlers::handle_send(req, cr).await }
    });
}

#[cfg(feature = "gateway")]
async fn setup_config_watcher(
    server: &mut GatewayServer,
    config_path: Option<PathBuf>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    daemon_mode: bool,
) -> Option<Arc<ConfigWatcher>> {
    let path = config_path?;

    if !path.exists() {
        if !daemon_mode {
            println!("No config file found at {}, hot reload disabled", path.display());
            println!();
        }
        return None;
    }

    let watcher_config = ConfigWatcherConfig {
        config_path: path.clone(),
        debounce_duration: std::time::Duration::from_millis(500),
        channel_capacity: 16,
    };

    match ConfigWatcher::new(watcher_config) {
        Ok(watcher) => {
            let watcher = Arc::new(watcher);

            // Register config handlers
            // config.reload
            let cw_reload = watcher.clone();
            server.handlers_mut().register("config.reload", move |req| {
                let cw = cw_reload.clone();
                async move { config_handlers::handle_reload(req, cw).await }
            });

            // config.get
            let cw_get = watcher.clone();
            server.handlers_mut().register("config.get", move |req| {
                let cw = cw_get.clone();
                async move { config_handlers::handle_get(req, cw).await }
            });

            // config.validate
            let cw_validate = watcher.clone();
            server.handlers_mut().register("config.validate", move |req| {
                let cw = cw_validate.clone();
                async move { config_handlers::handle_validate(req, cw).await }
            });

            // config.path
            let cw_path = watcher.clone();
            server.handlers_mut().register("config.path", move |req| {
                let cw = cw_path.clone();
                async move { config_handlers::handle_path(req, cw).await }
            });

            if !daemon_mode {
                println!("Config methods:");
                println!("  - config.reload   : Force reload configuration");
                println!("  - config.get      : Get current configuration");
                println!("  - config.validate : Validate config file");
                println!("  - config.path     : Get config file path");
                println!();
            }

            // Start watching for config changes
            let watcher_for_watch = watcher.clone();
            let event_bus_for_config = event_bus.clone();
            tokio::spawn(async move {
                let mut config_rx = watcher_for_watch.subscribe();

                // Start the file watcher
                let watcher_handle = watcher_for_watch.clone().start_watching();

                // Process config events
                while let Ok(event) = config_rx.recv().await {
                    match event {
                        ConfigEvent::Reloaded(new_config) => {
                            if !daemon_mode {
                                println!("Configuration reloaded: {} agents", new_config.agents.len());
                            }
                            // Emit event to connected clients
                            use alephcore::gateway::TopicEvent;
                            let event = TopicEvent::new(
                                "config.reloaded",
                                serde_json::json!({
                                    "agents": new_config.agents.keys().collect::<Vec<_>>(),
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                }),
                            );
                            let _ = event_bus_for_config.publish_json(&event);
                        }
                        ConfigEvent::ValidationFailed(err) => {
                            if !daemon_mode {
                                eprintln!("Config validation failed: {}", err);
                            }
                            use alephcore::gateway::TopicEvent;
                            let event = TopicEvent::new(
                                "config.error",
                                serde_json::json!({
                                    "error": err,
                                    "timestamp": chrono::Utc::now().to_rfc3339(),
                                }),
                            );
                            let _ = event_bus_for_config.publish_json(&event);
                        }
                        ConfigEvent::FileError(err) => {
                            if !daemon_mode {
                                eprintln!("Config file error: {}", err);
                            }
                        }
                    }
                }

                // Wait for watcher to finish (it won't unless there's an error)
                let _ = watcher_handle.await;
            });

            if !daemon_mode {
                println!("Hot config reload enabled: {}", path.display());
                println!();
            }

            Some(watcher)
        }
        Err(e) => {
            if !daemon_mode {
                eprintln!("Warning: Failed to initialize config watcher: {}", e);
            }
            None
        }
    }
}

#[cfg(feature = "gateway")]
async fn start_webchat_server(args: &Args, final_bind: &str, final_port: u16) {
    let webchat_dir = args.webchat_dir.clone().or_else(|| {
        // Try default locations: ./ui/webchat/dist or ../ui/webchat/dist or ~/.aleph/webchat
        let mut candidates = vec![
            PathBuf::from("ui/webchat/dist"),
            PathBuf::from("../ui/webchat/dist"),
        ];
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".aleph/webchat"));
        }
        candidates.into_iter().find(|p| p.exists())
    });

    if let Some(webchat_path) = webchat_dir {
        if webchat_path.exists() {
            let webchat_port = args.webchat_port.unwrap_or(final_port);
            let webchat_addr: SocketAddr = format!("{}:{}", final_bind, webchat_port)
                .parse()
                .expect("Invalid webchat address");

            // Only start separate HTTP server if port is different from WS port
            if webchat_port != final_port {
                let webchat_path_clone = webchat_path.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_webchat(webchat_addr, webchat_path_clone).await {
                        tracing::error!("WebChat server error: {}", e);
                    }
                });

                if !args.daemon {
                    println!("WebChat UI:");
                    println!("  - URL: http://{}", webchat_addr);
                    println!("  - Static: {}", webchat_path.display());
                    println!();
                }
            } else if !args.daemon {
                println!("WebChat UI directory found: {}", webchat_path.display());
                println!("  Note: WebChat requires a separate HTTP port (use --webchat-port)");
                println!();
            }
        } else if !args.daemon {
            println!("WebChat directory not found: {}", webchat_path.display());
            println!();
        }
    }
}

/// Start ControlPlane embedded web UI server
#[cfg(all(feature = "gateway", feature = "control-plane"))]
async fn start_control_plane_server(final_bind: &str, final_port: u16, daemon_mode: bool) {
    use axum::Router;
    use alephcore::gateway::control_plane::create_control_plane_router;

    // Use a different port for ControlPlane (default: 8081)
    let cp_port = final_port + 1;
    let control_plane_addr: SocketAddr = format!("{}:{}", final_bind, cp_port)
        .parse()
        .expect("Invalid control plane address");

    // Create ControlPlane router
    let cp_router = create_control_plane_router();

    // Mount under /cp path
    let app = Router::new().nest("/cp", cp_router);

    // Spawn ControlPlane server
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(control_plane_addr).await {
            Ok(listener) => {
                tracing::info!("ControlPlane UI available at http://{}/cp", control_plane_addr);
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::error!("ControlPlane server error: {}", e);
                }
            }
            Err(e) => {
                tracing::error!("Failed to bind ControlPlane server: {}", e);
            }
        }
    });

    if !daemon_mode {
        println!("ControlPlane UI:");
        println!("  - URL: http://{}/cp", control_plane_addr);
        println!("  - Embedded: rust-embed (WASM)");
        println!();
    }
}

#[cfg(feature = "gateway")]
fn register_config_handlers(
    server: &mut GatewayServer,
    config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    device_store: Arc<alephcore::gateway::device_store::DeviceStore>,
) {
    use alephcore::gateway::handlers::config::{handle_get_full_config, handle_patch_config};
    use alephcore::gateway::handlers::providers;
    use alephcore::gateway::handlers::routing_rules;
    use alephcore::gateway::handlers::mcp_config;
    use alephcore::gateway::handlers::memory_config;
    use alephcore::gateway::handlers::security_config;
    use alephcore::gateway::handlers::generation_providers;
    use alephcore::gateway::handlers::agent_config;
    use alephcore::gateway::handlers::general_config;
    use alephcore::gateway::handlers::shortcuts_config;
    use alephcore::gateway::handlers::behavior_config;

    // config.get
    let config_get = config.clone();
    server.handlers_mut().register("config.get", move |req| {
        let cfg = config_get.clone();
        async move { handle_get_full_config(req, cfg).await }
    });

    // config.patch
    let config_patch = config.clone();
    let event_bus_patch = event_bus.clone();
    server.handlers_mut().register("config.patch", move |req| {
        let cfg = config_patch.clone();
        let bus = event_bus_patch.clone();
        async move { handle_patch_config(req, cfg, bus).await }
    });

    // providers.list
    let config_list = config.clone();
    server.handlers_mut().register("providers.list", move |req| {
        let cfg = config_list.clone();
        async move { providers::handle_list(req, cfg).await }
    });

    // providers.get
    let config_get_provider = config.clone();
    server.handlers_mut().register("providers.get", move |req| {
        let cfg = config_get_provider.clone();
        async move { providers::handle_get(req, cfg).await }
    });

    // providers.create
    let config_create = config.clone();
    let event_bus_create = event_bus.clone();
    server.handlers_mut().register("providers.create", move |req| {
        let cfg = config_create.clone();
        let bus = event_bus_create.clone();
        async move { providers::handle_create(req, cfg, bus).await }
    });

    // providers.update
    let config_update = config.clone();
    let event_bus_update = event_bus.clone();
    server.handlers_mut().register("providers.update", move |req| {
        let cfg = config_update.clone();
        let bus = event_bus_update.clone();
        async move { providers::handle_update(req, cfg, bus).await }
    });

    // providers.delete
    let config_delete = config.clone();
    let event_bus_delete = event_bus.clone();
    server.handlers_mut().register("providers.delete", move |req| {
        let cfg = config_delete.clone();
        let bus = event_bus_delete.clone();
        async move { providers::handle_delete(req, cfg, bus).await }
    });

    // providers.setDefault
    let config_set_default = config.clone();
    let event_bus_set_default = event_bus.clone();
    server.handlers_mut().register("providers.setDefault", move |req| {
        let cfg = config_set_default.clone();
        let bus = event_bus_set_default.clone();
        async move { providers::handle_set_default(req, cfg, bus).await }
    });

    // providers.test
    server.handlers_mut().register("providers.test", move |req| {
        async move { providers::handle_test(req).await }
    });

    // routing_rules.list
    let config_rules_list = config.clone();
    server.handlers_mut().register("routing_rules.list", move |req| {
        let cfg = config_rules_list.clone();
        async move { routing_rules::handle_list(req, cfg).await }
    });

    // routing_rules.get
    let config_rules_get = config.clone();
    server.handlers_mut().register("routing_rules.get", move |req| {
        let cfg = config_rules_get.clone();
        async move { routing_rules::handle_get(req, cfg).await }
    });

    // routing_rules.create
    let config_rules_create = config.clone();
    let event_bus_rules_create = event_bus.clone();
    server.handlers_mut().register("routing_rules.create", move |req| {
        let cfg = config_rules_create.clone();
        let bus = event_bus_rules_create.clone();
        async move { routing_rules::handle_create(req, cfg, bus).await }
    });

    // routing_rules.update
    let config_rules_update = config.clone();
    let event_bus_rules_update = event_bus.clone();
    server.handlers_mut().register("routing_rules.update", move |req| {
        let cfg = config_rules_update.clone();
        let bus = event_bus_rules_update.clone();
        async move { routing_rules::handle_update(req, cfg, bus).await }
    });

    // routing_rules.delete
    let config_rules_delete = config.clone();
    let event_bus_rules_delete = event_bus.clone();
    server.handlers_mut().register("routing_rules.delete", move |req| {
        let cfg = config_rules_delete.clone();
        let bus = event_bus_rules_delete.clone();
        async move { routing_rules::handle_delete(req, cfg, bus).await }
    });

    // routing_rules.move
    let config_rules_move = config.clone();
    let event_bus_rules_move = event_bus.clone();
    server.handlers_mut().register("routing_rules.move", move |req| {
        let cfg = config_rules_move.clone();
        let bus = event_bus_rules_move.clone();
        async move { routing_rules::handle_move(req, cfg, bus).await }
    });

    // mcp_config.list
    let config_mcp_list = config.clone();
    server.handlers_mut().register("mcp_config.list", move |req| {
        let cfg = config_mcp_list.clone();
        async move { mcp_config::handle_list(req, cfg).await }
    });

    // mcp_config.get
    let config_mcp_get = config.clone();
    server.handlers_mut().register("mcp_config.get", move |req| {
        let cfg = config_mcp_get.clone();
        async move { mcp_config::handle_get(req, cfg).await }
    });

    // mcp_config.create
    let config_mcp_create = config.clone();
    let event_bus_mcp_create = event_bus.clone();
    server.handlers_mut().register("mcp_config.create", move |req| {
        let cfg = config_mcp_create.clone();
        let bus = event_bus_mcp_create.clone();
        async move { mcp_config::handle_create(req, cfg, bus).await }
    });

    // mcp_config.update
    let config_mcp_update = config.clone();
    let event_bus_mcp_update = event_bus.clone();
    server.handlers_mut().register("mcp_config.update", move |req| {
        let cfg = config_mcp_update.clone();
        let bus = event_bus_mcp_update.clone();
        async move { mcp_config::handle_update(req, cfg, bus).await }
    });

    // mcp_config.delete
    let config_mcp_delete = config.clone();
    let event_bus_mcp_delete = event_bus.clone();
    server.handlers_mut().register("mcp_config.delete", move |req| {
        let cfg = config_mcp_delete.clone();
        let bus = event_bus_mcp_delete.clone();
        async move { mcp_config::handle_delete(req, cfg, bus).await }
    });

    // memory_config.get
    let config_memory_get = config.clone();
    server.handlers_mut().register("memory_config.get", move |req| {
        let cfg = config_memory_get.clone();
        async move { memory_config::handle_get(req, cfg).await }
    });

    // memory_config.update
    let config_memory_update = config.clone();
    let event_bus_memory_update = event_bus.clone();
    server.handlers_mut().register("memory_config.update", move |req| {
        let cfg = config_memory_update.clone();
        let bus = event_bus_memory_update.clone();
        async move { memory_config::handle_update(req, cfg, bus).await }
    });

    // security_config.get
    server.handlers_mut().register("security_config.get", move |req| {
        async move { security_config::handle_get(req).await }
    });

    // security_config.update
    let event_bus_security_update = event_bus.clone();
    server.handlers_mut().register("security_config.update", move |req| {
        let bus = event_bus_security_update.clone();
        async move { security_config::handle_update(req, bus).await }
    });

    // security_config.list_devices
    let device_store_list = device_store.clone();
    server.handlers_mut().register("security_config.list_devices", move |req| {
        let store = device_store_list.clone();
        async move { security_config::handle_list_devices(req, store).await }
    });

    // security_config.revoke_device
    let device_store_revoke = device_store.clone();
    let event_bus_security_revoke = event_bus.clone();
    server.handlers_mut().register("security_config.revoke_device", move |req| {
        let store = device_store_revoke.clone();
        let bus = event_bus_security_revoke.clone();
        async move { security_config::handle_revoke_device(req, store, bus).await }
    });

    // generation_providers.list
    let config_gen_list = config.clone();
    server.handlers_mut().register("generation_providers.list", move |req| {
        let cfg = config_gen_list.clone();
        async move { generation_providers::handle_list(req, cfg).await }
    });

    // generation_providers.get
    let config_gen_get = config.clone();
    server.handlers_mut().register("generation_providers.get", move |req| {
        let cfg = config_gen_get.clone();
        async move { generation_providers::handle_get(req, cfg).await }
    });

    // generation_providers.create
    let config_gen_create = config.clone();
    let event_bus_gen_create = event_bus.clone();
    server.handlers_mut().register("generation_providers.create", move |req| {
        let cfg = config_gen_create.clone();
        let bus = event_bus_gen_create.clone();
        async move { generation_providers::handle_create(req, cfg, bus).await }
    });

    // generation_providers.update
    let config_gen_update = config.clone();
    let event_bus_gen_update = event_bus.clone();
    server.handlers_mut().register("generation_providers.update", move |req| {
        let cfg = config_gen_update.clone();
        let bus = event_bus_gen_update.clone();
        async move { generation_providers::handle_update(req, cfg, bus).await }
    });

    // generation_providers.delete
    let config_gen_delete = config.clone();
    let event_bus_gen_delete = event_bus.clone();
    server.handlers_mut().register("generation_providers.delete", move |req| {
        let cfg = config_gen_delete.clone();
        let bus = event_bus_gen_delete.clone();
        async move { generation_providers::handle_delete(req, cfg, bus).await }
    });

    // generation_providers.setDefault
    let config_gen_set_default = config.clone();
    let event_bus_gen_set_default = event_bus.clone();
    server.handlers_mut().register("generation_providers.setDefault", move |req| {
        let cfg = config_gen_set_default.clone();
        let bus = event_bus_gen_set_default.clone();
        async move { generation_providers::handle_set_default(req, cfg, bus).await }
    });

    // generation_providers.test
    let config_gen_test = config.clone();
    server.handlers_mut().register("generation_providers.test", move |req| {
        let cfg = config_gen_test.clone();
        async move { generation_providers::handle_test_connection(req, cfg).await }
    });

    // agent_config.get
    let config_agent_get = config.clone();
    server.handlers_mut().register("agent_config.get", move |req| {
        let cfg = config_agent_get.clone();
        async move { agent_config::handle_get(req, cfg).await }
    });

    // agent_config.update
    let config_agent_update = config.clone();
    let event_bus_agent_update = event_bus.clone();
    server.handlers_mut().register("agent_config.update", move |req| {
        let cfg = config_agent_update.clone();
        let bus = event_bus_agent_update.clone();
        async move { agent_config::handle_update(req, cfg, bus).await }
    });

    // agent_config.get_file_ops
    let config_agent_file_ops_get = config.clone();
    server.handlers_mut().register("agent_config.get_file_ops", move |req| {
        let cfg = config_agent_file_ops_get.clone();
        async move { agent_config::handle_get_file_ops(req, cfg).await }
    });

    // agent_config.update_file_ops
    let config_agent_file_ops_update = config.clone();
    let event_bus_agent_file_ops_update = event_bus.clone();
    server.handlers_mut().register("agent_config.update_file_ops", move |req| {
        let cfg = config_agent_file_ops_update.clone();
        let bus = event_bus_agent_file_ops_update.clone();
        async move { agent_config::handle_update_file_ops(req, cfg, bus).await }
    });

    // agent_config.get_code_exec
    let config_agent_code_exec_get = config.clone();
    server.handlers_mut().register("agent_config.get_code_exec", move |req| {
        let cfg = config_agent_code_exec_get.clone();
        async move { agent_config::handle_get_code_exec(req, cfg).await }
    });

    // agent_config.update_code_exec
    let config_agent_code_exec_update = config.clone();
    let event_bus_agent_code_exec_update = event_bus.clone();
    server.handlers_mut().register("agent_config.update_code_exec", move |req| {
        let cfg = config_agent_code_exec_update.clone();
        let bus = event_bus_agent_code_exec_update.clone();
        async move { agent_config::handle_update_code_exec(req, cfg, bus).await }
    });

    // general_config.get
    let config_general_get = config.clone();
    server.handlers_mut().register("general_config.get", move |req| {
        let cfg = config_general_get.clone();
        async move { general_config::handle_get(req, cfg).await }
    });

    // general_config.update
    let config_general_update = config.clone();
    let event_bus_general_update = event_bus.clone();
    server.handlers_mut().register("general_config.update", move |req| {
        let cfg = config_general_update.clone();
        let bus = event_bus_general_update.clone();
        async move { general_config::handle_update(req, cfg, bus).await }
    });

    // shortcuts_config.get
    let config_shortcuts_get = config.clone();
    server.handlers_mut().register("shortcuts_config.get", move |req| {
        let cfg = config_shortcuts_get.clone();
        async move { shortcuts_config::handle_get(req, cfg).await }
    });

    // shortcuts_config.update
    let config_shortcuts_update = config.clone();
    let event_bus_shortcuts_update = event_bus.clone();
    server.handlers_mut().register("shortcuts_config.update", move |req| {
        let cfg = config_shortcuts_update.clone();
        let bus = event_bus_shortcuts_update.clone();
        async move { shortcuts_config::handle_update(req, cfg, bus).await }
    });

    // behavior_config.get
    let config_behavior_get = config.clone();
    server.handlers_mut().register("behavior_config.get", move |req| {
        let cfg = config_behavior_get.clone();
        async move { behavior_config::handle_get(req, cfg).await }
    });

    // behavior_config.update
    let config_behavior_update = config.clone();
    let event_bus_behavior_update = event_bus.clone();
    server.handlers_mut().register("behavior_config.update", move |req| {
        let cfg = config_behavior_update.clone();
        let bus = event_bus_behavior_update.clone();
        async move { behavior_config::handle_update(req, cfg, bus).await }
    });
}
