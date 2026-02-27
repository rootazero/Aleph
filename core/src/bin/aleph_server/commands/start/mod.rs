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
use alephcore::gateway::bridge::DesktopBridgeManager;
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
};
#[cfg(feature = "gateway")]
use alephcore::gateway::pairing_store::SqlitePairingStore;
#[cfg(all(feature = "gateway", feature = "discord"))]
use alephcore::gateway::handlers::discord_panel as discord_panel_handlers;
#[cfg(feature = "gateway")]
use alephcore::gateway::handlers::auth as auth_handlers;
#[cfg(feature = "gateway")]
use alephcore::gateway::security::{TokenManager, PairingManager};
#[cfg(feature = "gateway")]
use alephcore::gateway::device_store::DeviceStore;
#[cfg(all(feature = "gateway", target_os = "macos"))]
use alephcore::gateway::interfaces::{IMessageChannel, IMessageConfig};
#[cfg(feature = "telegram")]
use alephcore::gateway::interfaces::{TelegramChannel, TelegramConfig};
#[cfg(feature = "discord")]
use alephcore::gateway::interfaces::{DiscordChannel, DiscordConfig};
#[cfg(feature = "whatsapp")]
use alephcore::gateway::interfaces::{WhatsAppChannel, WhatsAppConfig};
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
use crate::server_init::handle_run_with_engine;

#[cfg(feature = "gateway")]
mod builder;
#[cfg(feature = "gateway")]
use builder::*;

// ── Subsystem initializer functions ──────────────────────────────────────────
// Each function handles one cohesive initialization concern, extracted from
// start_server() to keep the orchestrator function under 100 lines.

/// Validate that the bind address is available, or exit if not.
#[cfg(feature = "gateway")]
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
#[cfg(feature = "gateway")]
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

/// Initialize the tracing subscriber with log level from CLI args.
#[cfg(feature = "gateway")]
fn initialize_tracing(args: &Args) {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
    let filter = format!("aleph_server={},alephcore::gateway={}", args.log_level, args.log_level);
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

/// Load gateway configuration, apply CLI overrides, and return resolved values.
/// Returns (full_config, final_bind, final_port, final_max_connections).
#[cfg(feature = "gateway")]
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

    (full_config, final_bind, final_port, final_max_connections)
}

/// Initialize the SessionManager with SQLite persistence, falling back to a
/// temporary path on error.
#[cfg(feature = "gateway")]
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
#[cfg(feature = "gateway")]
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

/// Register agent.run / agent.status / agent.cancel handlers.
/// Selects real ExecutionEngine when an API key is available, otherwise uses
/// the simulated AgentRunManager.
/// Returns the shared AgentRunManager (needed for status/cancel regardless of mode).
#[cfg(feature = "gateway")]
async fn register_agent_handlers(
    server: &mut GatewayServer,
    session_manager: Arc<SessionManager>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    router: Arc<AgentRouter>,
    full_config: &FullGatewayConfig,
    daemon: bool,
) -> Arc<AgentRunManager> {
    let run_manager = Arc::new(AgentRunManager::new(router.clone(), event_bus.clone()));

    if can_create_provider_from_env() {
        match create_provider_registry_from_env() {
            Ok(provider_registry) => {
                let tool_registry = Arc::new(BuiltinToolRegistry::new());

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

                let engine = Arc::new(ExecutionEngine::new(
                    ExecutionEngineConfig::default(),
                    provider_registry,
                    tool_registry,
                    tools,
                    session_manager.clone(),
                ));

                let agent_registry = Arc::new(AgentRegistry::new());
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

                if !daemon {
                    let provider_name = available_provider_from_env().unwrap_or("unknown");
                    println!("  Mode: Real AgentLoop ({} API)", provider_name);
                    println!();
                }

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
                if !daemon {
                    eprintln!("Warning: Failed to create provider: {}. Falling back to simulated mode.", e);
                }
                let run_manager_clone = run_manager.clone();
                server.handlers_mut().register("agent.run", move |req| {
                    let manager = run_manager_clone.clone();
                    async move { handle_run(req, manager).await }
                });
            }
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

    if !daemon {
        println!("Agent control methods:");
        println!("  - agent.run     : Execute agent request with streaming");
        println!("  - agent.status  : Query run status by run_id");
        println!("  - agent.cancel  : Cancel an active run");
        println!();
    }

    run_manager
}

/// Register POE (Principle-Operation-Evaluation) handlers when an Anthropic
/// API key is available. Skips silently (with a note) if the key is absent.
#[cfg(feature = "gateway")]
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
#[cfg(feature = "gateway")]
struct AuthBundle {
    device_store: Arc<DeviceStore>,
    auth_ctx: Arc<auth_handlers::AuthContext>,
    mdns_broadcaster: Option<alephcore::gateway::MdnsBroadcaster>,
    invitation_manager: Arc<alephcore::gateway::security::InvitationManager>,
    guest_session_manager: Arc<alephcore::gateway::security::GuestSessionManager>,
}

/// Initialize authentication and security subsystems (construction only).
/// Does NOT register handlers — the orchestrator layer is responsible for that.
#[cfg(feature = "gateway")]
fn initialize_auth(
    port: u16,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    require_auth: bool,
    daemon: bool,
) -> AuthBundle {
    use tracing::{info, warn};

    let device_store_path = dirs::home_dir()
        .map(|h| h.join(".aleph/devices.db"))
        .unwrap_or_else(|| PathBuf::from("/tmp/aleph_devices.db"));

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
#[cfg(feature = "gateway")]
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
#[cfg(feature = "gateway")]
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
#[cfg(feature = "gateway")]
async fn initialize_channels(server: &mut GatewayServer, daemon: bool) -> Arc<ChannelRegistry> {
    let channel_registry = Arc::new(ChannelRegistry::new());

    #[cfg(target_os = "macos")]
    {
        let imessage_config = IMessageConfig {
            enabled: true,
            ..Default::default()
        };
        let imessage_channel = IMessageChannel::new(imessage_config);
        let channel_id = channel_registry.register(Box::new(imessage_channel)).await;
        if !daemon {
            println!("Registered channel: {} (iMessage)", channel_id);
        }
    }

    #[cfg(feature = "telegram")]
    {
        let telegram_config = TelegramConfig::default();
        let telegram_channel = TelegramChannel::new("telegram", telegram_config);
        let channel_id = channel_registry.register(Box::new(telegram_channel)).await;
        if !daemon {
            println!("Registered channel: {} (Telegram)", channel_id);
        }
    }

    #[cfg(feature = "discord")]
    {
        let discord_config = DiscordConfig::default();
        let discord_channel = DiscordChannel::new("discord", discord_config);
        let channel_id = channel_registry.register(Box::new(discord_channel)).await;
        if !daemon {
            println!("Registered channel: {} (Discord)", channel_id);
        }
    }

    #[cfg(feature = "whatsapp")]
    {
        let whatsapp_config = WhatsAppConfig::default();
        let whatsapp_channel = WhatsAppChannel::new("whatsapp", whatsapp_config);
        let channel_id = channel_registry.register(Box::new(whatsapp_channel)).await;
        if !daemon {
            println!("Registered channel: {} (WhatsApp)", channel_id);
        }
    }

    register_channel_handlers(server, &channel_registry);

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
#[cfg(feature = "gateway")]
async fn initialize_inbound_router(
    channel_registry: Arc<ChannelRegistry>,
    router: Arc<AgentRouter>,
    daemon: bool,
) {
    let pairing_store_path = dirs::home_dir()
        .map(|h| h.join(".aleph/pairing.db"))
        .unwrap_or_else(|| PathBuf::from("/tmp/aleph_pairing.db"));

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

    let routing_config = RoutingConfig::default();
    let inbound_router = Arc::new(
        InboundMessageRouter::new(
            channel_registry.clone(),
            pairing_store.clone(),
            routing_config,
        )
        .with_agent_router(router.clone())
    );

    let _inbound_router_handle = inbound_router.clone().start().await;
    if !daemon {
        println!("Inbound message router started");
        println!();
    }
}

// ─────────────────────────────────────────────────────────────────────────────

/// Start the gateway server
#[cfg(feature = "gateway")]
pub async fn start_server(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::server::GatewayConfig as ServerConfig;

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

    let server_config = ServerConfig {
        max_connections: final_max_connections,
        require_auth: full_config.gateway.require_auth,
        timeout_secs: 300,
    };
    let mut server = GatewayServer::with_config(addr, server_config);

    let session_manager = initialize_session_manager(args.daemon).await;
    initialize_extension_manager(args.daemon).await;

    let event_bus = server.event_bus().clone();
    let router = Arc::new(AgentRouter::new());

    register_agent_handlers(
        &mut server, session_manager.clone(), event_bus.clone(),
        router.clone(), &full_config, args.daemon,
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

    // App config loading and handler registration
    let app_config = Arc::new(tokio::sync::RwLock::new(load_app_config().await));
    register_config_handlers(&mut server, app_config, event_bus.clone(), auth_bundle.device_store.clone());

    register_session_handlers(&mut server, &session_manager, args.daemon);

    let channel_registry = initialize_channels(&mut server, args.daemon).await;
    initialize_inbound_router(channel_registry, router, args.daemon).await;

    let config_path = args.config.clone()
        .map(|p| expand_path(&p.to_string_lossy()))
        .or_else(|| dirs::home_dir().map(|h| h.join(".aleph/config.toml")));
    let _config_watcher = setup_config_watcher(&mut server, config_path, &event_bus, args.daemon).await;

    start_webchat_server(args, &final_bind, final_port).await;

    #[cfg(feature = "control-plane")]
    start_control_plane_server(&final_bind, final_port, args.daemon).await;

    // Start desktop bridge (non-blocking — server runs headless if bridge not found)
    let run_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("run");
    let mut bridge_manager = DesktopBridgeManager::new(run_dir, final_port);
    if let Err(e) = bridge_manager.start().await {
        tracing::warn!("Desktop bridge not started: {e} — running headless");
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
